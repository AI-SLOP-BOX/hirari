//! Live input staging for the realtime callback.
//!
//! The driver callback copies the monitored input pair and the Control Room
//! talkback source into fixed-capacity planar buffers; the render callback then
//! reads a window of them. This module owns that storage, the channel selection,
//! the frame bound and the publication ordering, so C++ keeps only the track
//! lookup and the `addInput` handoff it cannot express across the FFI.
//!
//! The planes are raw pointers rather than `Vec` fields on purpose: exactly one
//! thread (the driver callback) writes them while the render thread reads a
//! published window, so no `&mut` to `InputBlocks` is ever formed and the copy
//! cannot be claimed to alias the reader's `&`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The largest block the driver may hand over, mirroring the engine's own cap.
const MAX_BLOCK: u32 = 16_384;
/// Focus sentinel: no track is being monitored. Matches kNoInputMonitorTrack.
pub const NO_TRACK: u32 = u32::MAX;

/// A monitorable window into the staged input, resolved for one render chunk.
/// Both planes are null unless a focus is set and `frames` samples remain at
/// `offset`, so the caller never has to re-check the publication itself.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HirariInputMonitorBlock {
    pub track_id: u32,
    pub left: *mut f32,
    pub right: *mut f32,
}

struct InputBlocks {
    capacity: usize,
    monitor_left: *mut f32,
    monitor_right: *mut f32,
    talkback: *mut f32,
    /// Packed as `(right << 32) | left`, the encoding the routing host uses.
    focus_pair: AtomicU64,
    focus_track: AtomicU32,
    monitor_frames: AtomicU32,
    monitor_published: AtomicBool,
    talkback_frames: AtomicU32,
    talkback_published: AtomicBool,
}

impl InputBlocks {
    fn new(capacity: usize) -> Self {
        fn plane(capacity: usize) -> *mut f32 {
            Box::into_raw(vec![0.0f32; capacity].into_boxed_slice()).cast()
        }
        InputBlocks {
            capacity,
            monitor_left: plane(capacity),
            monitor_right: plane(capacity),
            talkback: plane(capacity),
            focus_pair: AtomicU64::new(1u64 << 32),
            focus_track: AtomicU32::new(NO_TRACK),
            monitor_frames: AtomicU32::new(0),
            monitor_published: AtomicBool::new(false),
            talkback_frames: AtomicU32::new(0),
            talkback_published: AtomicBool::new(false),
        }
    }

    fn focus(&self) -> (u32, u32, u32) {
        let pair = self.focus_pair.load(Ordering::Acquire);
        (
            self.focus_track.load(Ordering::Acquire),
            pair as u32,
            (pair >> 32) as u32,
        )
    }

    fn window(&self, offset: u32, frames: u32, published: &AtomicU32) -> bool {
        published.load(Ordering::Acquire) > offset
            && published.load(Ordering::Acquire) - offset >= frames
    }
}

impl Drop for InputBlocks {
    fn drop(&mut self) {
        for plane in [self.monitor_left, self.monitor_right, self.talkback] {
            unsafe {
                drop(Box::from_raw(std::slice::from_raw_parts_mut(
                    plane, self.capacity,
                )))
            };
        }
    }
}

fn channels<'a>(inputs: *const *const f32, channel_count: u32) -> &'a [*const f32] {
    if inputs.is_null() {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(inputs, channel_count as usize) }
}

fn as_blocks(handle: *const c_void) -> Option<&'static InputBlocks> {
    unsafe { handle.cast::<InputBlocks>().as_ref() }
}

/// # Safety
/// `capacity_frames` must be the engine's block cap; the planes are sized once.
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_create(capacity_frames: usize) -> *mut c_void {
    if capacity_frames == 0 || capacity_frames > MAX_BLOCK as usize {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(InputBlocks::new(capacity_frames))).cast()
}

/// # Safety
/// `handle` must be null or a pointer from [`hirari_input_blocks_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<InputBlocks>())) };
    }
}

/// Points input monitoring at `track_id` and its physical input pair. One input
/// is monitored at a time, so this atomically moves the focus; record-arm state
/// stays a track decision in Rust.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_set_focus(
    handle: *mut c_void,
    track_id: u32,
    left_channel: u32,
    right_channel: u32,
) -> bool {
    let Some(blocks) = as_blocks(handle) else {
        return false;
    };
    if track_id == NO_TRACK || left_channel == right_channel {
        return false;
    }
    blocks
        .focus_pair
        .store((u64::from(right_channel) << 32) | u64::from(left_channel), Ordering::Release);
    blocks.focus_track.store(track_id, Ordering::Release);
    true
}

/// Drops the focus, but only if `track_id` still holds it, so disabling one
/// track cannot clear a monitor another track has since taken over.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_clear_focus(handle: *mut c_void, track_id: u32) {
    if let Some(blocks) = as_blocks(handle) {
        // A failing compare means another track already owns the focus.
        let _ = blocks
            .focus_track
            .compare_exchange(track_id, NO_TRACK, Ordering::Release, Ordering::Relaxed);
    }
}

/// Forgets the focus and both publications, as starting a new project does.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_reset(handle: *mut c_void) {
    if let Some(blocks) = as_blocks(handle) {
        blocks.focus_track.store(NO_TRACK, Ordering::Release);
        hirari_input_blocks_end_monitor(handle);
        hirari_input_blocks_end_talkback(handle);
    }
}

/// Validates the focus and the incoming block, copies the selected pair and
/// publishes it for the render callback. A rejected copy withdraws the block.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`], `inputs` must have
/// `channel_count` readable planes of at least `frames` samples each, and only
/// the driver callback may call this.
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_publish_monitor(
    handle: *mut c_void,
    inputs: *const *const f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(blocks) = as_blocks(handle) else {
        return;
    };
    let (focus, left_channel, right_channel) = blocks.focus();
    let selected = channels(inputs, channel_count);
    let valid = focus != NO_TRACK
        && frames != 0
        && frames as usize <= blocks.capacity
        && (left_channel as usize) < selected.len()
        && (right_channel as usize) < selected.len();
    let copied = valid && {
        let (source_left, source_right) = (
            selected[left_channel as usize],
            selected[right_channel as usize],
        );
        !source_left.is_null()
            && !source_right.is_null()
            && {
                let (out_left, out_right, in_left, in_right) = unsafe {
                    (
                        std::slice::from_raw_parts_mut(blocks.monitor_left, frames as usize),
                        std::slice::from_raw_parts_mut(blocks.monitor_right, frames as usize),
                        std::slice::from_raw_parts(source_left, frames as usize),
                        std::slice::from_raw_parts(source_right, frames as usize),
                    )
                };
                for (((out_left, out_right), in_left), in_right) in out_left
                    .iter_mut()
                    .zip(out_right)
                    .zip(in_left)
                    .zip(in_right)
                {
                    *out_left = if in_left.is_finite() { *in_left } else { 0.0 };
                    *out_right = if in_right.is_finite() { *in_right } else { 0.0 };
                }
                true
            }
    };
    if !copied {
        hirari_input_blocks_end_monitor(handle);
        blocks.monitor_frames.store(0, Ordering::Release);
        return;
    }
    blocks.monitor_frames.store(frames, Ordering::Release);
    blocks.monitor_published.store(true, Ordering::Release);
}

/// Resolves the published window for a chunk starting at `offset`. A silent
/// block answers when nothing is monitorable at that offset.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_take_monitor(
    handle: *const c_void,
    offset: u32,
    frames: u32,
) -> HirariInputMonitorBlock {
    let Some(blocks) = as_blocks(handle) else {
        return HirariInputMonitorBlock::default();
    };
    if !blocks.monitor_published.load(Ordering::Acquire)
        || !blocks.window(offset, frames, &blocks.monitor_frames)
    {
        return HirariInputMonitorBlock {
            track_id: NO_TRACK,
            ..HirariInputMonitorBlock::default()
        };
    }
    HirariInputMonitorBlock {
        track_id: blocks.focus_track.load(Ordering::Acquire),
        left: unsafe { blocks.monitor_left.add(offset as usize) },
        right: unsafe { blocks.monitor_right.add(offset as usize) },
    }
}

/// Withdraws the monitor block once a chunked callback has consumed it. The
/// focus itself survives: it is a device assignment, not a per-callback one.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`] and only the render
/// callback may call this.
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_end_monitor(handle: *mut c_void) {
    if let Some(blocks) = as_blocks(handle) {
        blocks.monitor_published.store(false, Ordering::Release);
    }
}

/// Copies and clamps the selected Control Room talkback channel, then publishes
/// it with the same release ordering the render callback acquires.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`], `control_room` must
/// be null or a live Control Room state, `inputs` must have `channel_count`
/// readable planes of at least `frames` samples each, and only the driver
/// callback may call this.
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_publish_talkback(
    handle: *mut c_void,
    inputs: *const *const f32,
    channel_count: u32,
    frames: u32,
    control_room: *const c_void,
) {
    let Some(blocks) = as_blocks(handle) else {
        return;
    };
    let enabled = !control_room.is_null()
        && unsafe { crate::control_room_runtime::hirari_control_room_talkback_enabled(control_room) };
    let input_channel = if enabled {
        unsafe { crate::control_room_runtime::hirari_control_room_talkback_channel(control_room) }
    } else {
        u32::MAX
    };
    let source = channels(inputs, channel_count).get(input_channel as usize).copied();
    let valid = enabled
        && frames != 0
        && frames as usize <= blocks.capacity
        && source.is_some_and(|plane| !plane.is_null());
    if !valid {
        hirari_input_blocks_end_talkback(handle);
        return;
    }
    let (output, input) = unsafe {
        (
            std::slice::from_raw_parts_mut(blocks.talkback, frames as usize),
            std::slice::from_raw_parts(source.unwrap_or(std::ptr::null()), frames as usize),
        )
    };
    for (out, sample) in output.iter_mut().zip(input) {
        *out = if sample.is_finite() {
            sample.clamp(-16.0, 16.0)
        } else {
            0.0
        };
    }
    blocks.talkback_frames.store(frames, Ordering::Release);
    blocks.talkback_published.store(true, Ordering::Release);
}

/// Resolves the talkback window for a chunk, or null when it cannot be served.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`].
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_take_talkback(
    handle: *const c_void,
    offset: u32,
    frames: u32,
) -> *const f32 {
    let Some(blocks) = as_blocks(handle) else {
        return std::ptr::null();
    };
    if !blocks.talkback_published.load(Ordering::Acquire)
        || !blocks.window(offset, frames, &blocks.talkback_frames)
    {
        return std::ptr::null();
    }
    unsafe { blocks.talkback.add(offset as usize) }
}

/// Withdraws talkback for the next callback.
///
/// # Safety
/// `handle` must come from [`hirari_input_blocks_create`] and only the render
/// callback may call this.
#[no_mangle]
pub unsafe extern "C" fn hirari_input_blocks_end_talkback(handle: *mut c_void) {
    if let Some(blocks) = as_blocks(handle) {
        blocks.talkback_published.store(false, Ordering::Release);
        blocks.talkback_frames.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPACITY: u32 = 8;

    struct Blocks(*mut c_void);

    impl Blocks {
        fn new() -> Self {
            let handle = unsafe { hirari_input_blocks_create(CAPACITY as usize) };
            assert!(!handle.is_null());
            Blocks(handle)
        }
        fn get(&self) -> *mut c_void {
            self.0
        }
        fn blocks(&self) -> &'static InputBlocks {
            unsafe { self.get().cast::<InputBlocks>().as_ref() }.expect("blocks")
        }
        fn focus(&self, track_id: u32, left: u32, right: u32) -> bool {
            unsafe { hirari_input_blocks_set_focus(self.get(), track_id, left, right) }
        }
        fn publish(&self, planes: &[*const f32], frames: u32) {
            unsafe {
                hirari_input_blocks_publish_monitor(
                    self.get(),
                    planes.as_ptr(),
                    planes.len() as u32,
                    frames,
                )
            };
        }
        fn publish_talkback(&self, planes: &[*const f32], frames: u32) {
            unsafe {
                hirari_input_blocks_publish_talkback(
                    self.get(),
                    planes.as_ptr(),
                    planes.len() as u32,
                    frames,
                    std::ptr::null(),
                )
            };
        }
        fn monitor(&self, offset: u32, frames: u32) -> HirariInputMonitorBlock {
            unsafe { hirari_input_blocks_take_monitor(self.get(), offset, frames) }
        }
        fn talkback(&self, offset: u32, frames: u32) -> *const f32 {
            unsafe { hirari_input_blocks_take_talkback(self.get(), offset, frames) }
        }
    }

    impl Drop for Blocks {
        fn drop(&mut self) {
            unsafe { hirari_input_blocks_destroy(self.get()) };
        }
    }

    fn samples(block: *mut f32, frames: usize) -> Vec<f32> {
        unsafe { std::slice::from_raw_parts(block, frames) }.to_vec()
    }

    #[test]
    fn focus_moves_the_monitor_and_only_its_owner_clears_it() {
        let blocks = Blocks::new();
        assert!(!blocks.focus(NO_TRACK, 0, 1));
        assert!(!blocks.focus(4, 2, 2));
        assert!(blocks.focus(4, 0, 1));
        unsafe { hirari_input_blocks_clear_focus(blocks.get(), 5) };
        assert_eq!(blocks.blocks().focus().0, 4);
        unsafe { hirari_input_blocks_clear_focus(blocks.get(), 4) };
        assert_eq!(blocks.blocks().focus().0, NO_TRACK);
        assert_eq!(blocks.blocks().focus(), (NO_TRACK, 0, 1));
    }

    #[test]
    fn nothing_is_monitorable_until_a_block_is_published() {
        let blocks = Blocks::new();
        assert!(blocks.focus(7, 0, 1));
        let silent = blocks.monitor(0, 4);
        assert_eq!(silent.track_id, NO_TRACK);
        assert!(silent.left.is_null() && silent.right.is_null());
    }

    #[test]
    fn publish_copies_the_selected_pair_and_sanitizes_it() {
        let left = [0.5_f32, f32::NAN, -0.25, 1.0, 7.0, 7.0, 7.0, 7.0];
        let right = [-0.5_f32, 0.25, f32::INFINITY, 2.0, 7.0, 7.0, 7.0, 7.0];
        let planes = [left.as_ptr(), right.as_ptr()];
        let blocks = Blocks::new();
        assert!(blocks.focus(9, 0, 1));
        blocks.publish(&planes, 4);
        let block = blocks.monitor(0, 4);
        assert_eq!(block.track_id, 9);
        assert_eq!(samples(block.left, 4), [0.5, 0.0, -0.25, 1.0]);
        assert_eq!(samples(block.right, 4), [-0.5, 0.25, 0.0, 2.0]);
    }

    #[test]
    fn an_out_of_range_or_unusable_block_withdraws_publication() {
        let plane_data = [0.5_f32; 8];
        let planes = [plane_data.as_ptr(), plane_data.as_ptr()];
        let blocks = Blocks::new();
        assert!(blocks.focus(11, 0, 1));
        blocks.publish(&planes, 4);
        assert!(!blocks.monitor(0, 4).left.is_null());
        // Over the staged capacity, an empty block, and a missing channel.
        blocks.publish(&planes, CAPACITY + 1);
        assert!(blocks.monitor(0, 4).left.is_null());
        blocks.publish(&planes, 0);
        assert!(blocks.monitor(0, 4).left.is_null());
        assert!(blocks.focus(12, 1, 4));
        blocks.publish(&planes, 4);
        assert!(blocks.monitor(0, 4).left.is_null());
        assert!(blocks.focus(13, 0, 1));
        blocks.publish(&[], 4);
        assert!(blocks.monitor(0, 4).left.is_null());
        let holes = [std::ptr::null(), plane_data.as_ptr()];
        blocks.publish(&holes, 4);
        assert!(blocks.monitor(0, 4).left.is_null());
    }

    #[test]
    fn a_window_only_covers_samples_still_available_at_the_offset() {
        let plane_data = [0.5_f32; 8];
        let planes = [plane_data.as_ptr(), plane_data.as_ptr()];
        let blocks = Blocks::new();
        assert!(blocks.focus(13, 0, 1));
        blocks.publish(&planes, 6);
        assert!(!blocks.monitor(2, 4).left.is_null());
        assert_eq!(samples(blocks.monitor(2, 4).left, 4), [0.5; 4]);
        assert!(blocks.monitor(4, 3).left.is_null());
        assert!(blocks.monitor(6, 1).left.is_null());
    }

    #[test]
    fn ending_the_monitor_keeps_the_focus_for_the_next_callback() {
        let plane_data = [0.5_f32; 8];
        let planes = [plane_data.as_ptr(), plane_data.as_ptr()];
        let blocks = Blocks::new();
        assert!(blocks.focus(14, 0, 1));
        blocks.publish(&planes, 4);
        unsafe { hirari_input_blocks_end_monitor(blocks.get()) };
        assert!(blocks.monitor(0, 4).left.is_null());
        assert_eq!(blocks.blocks().focus().0, 14);
        blocks.publish(&planes, 4);
        assert_eq!(blocks.monitor(0, 4).track_id, 14);
    }

    #[test]
    fn talkback_needs_an_enabled_control_room() {
        let plane_data = [0.5_f32; 8];
        let planes = [plane_data.as_ptr(), plane_data.as_ptr()];
        let blocks = Blocks::new();
        blocks.publish_talkback(&planes, 4);
        assert!(blocks.talkback(0, 4).is_null());
        blocks.publish_talkback(&[], 4);
        assert!(blocks.talkback(0, 4).is_null());
    }

    #[test]
    fn reset_forgets_the_focus_and_both_publications() {
        let plane_data = [0.5_f32; 8];
        let planes = [plane_data.as_ptr(), plane_data.as_ptr()];
        let blocks = Blocks::new();
        assert!(blocks.focus(15, 0, 1));
        blocks.publish(&planes, 4);
        unsafe { hirari_input_blocks_reset(blocks.get()) };
        assert!(blocks.monitor(0, 4).left.is_null());
        assert_eq!(blocks.blocks().focus().0, NO_TRACK);
        // A re-published block needs a fresh focus, not the forgotten one.
        blocks.publish(&planes, 4);
        assert!(blocks.monitor(0, 4).left.is_null());
        assert!(!blocks.blocks().talkback_published.load(Ordering::Acquire));
    }

    #[test]
    fn a_null_handle_answers_the_silent_defaults() {
        assert!(unsafe { hirari_input_blocks_create(0) }.is_null());
        assert!(unsafe { hirari_input_blocks_create(MAX_BLOCK as usize + 1) }.is_null());
        assert!(!unsafe { hirari_input_blocks_set_focus(std::ptr::null_mut(), 1, 0, 1) });
        unsafe {
            hirari_input_blocks_clear_focus(std::ptr::null_mut(), 1);
            hirari_input_blocks_reset(std::ptr::null_mut());
            hirari_input_blocks_publish_monitor(std::ptr::null_mut(), std::ptr::null(), 0, 4);
            hirari_input_blocks_publish_talkback(std::ptr::null_mut(), std::ptr::null(), 0, 4, std::ptr::null());
            hirari_input_blocks_end_monitor(std::ptr::null_mut());
            hirari_input_blocks_end_talkback(std::ptr::null_mut());
            hirari_input_blocks_destroy(std::ptr::null_mut());
        }
        assert_eq!(
            unsafe { hirari_input_blocks_take_monitor(std::ptr::null(), 0, 4) },
            HirariInputMonitorBlock::default()
        );
        assert!(unsafe { hirari_input_blocks_take_talkback(std::ptr::null(), 0, 4) }.is_null());
    }
}
