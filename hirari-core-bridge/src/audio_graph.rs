use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

type GraphNodeInfo =
    unsafe extern "C" fn(*mut c_void, u32, *mut bool, *mut f32, *mut u32, *mut *mut c_void) -> bool;
type GraphProcessNode = unsafe extern "C" fn(*mut c_void, u32) -> u8;
type GraphNodeMetrics = unsafe extern "C" fn(*mut c_void, u32, *mut u32, *mut f32) -> bool;
const DELAY_CAPACITY: usize = 65_536;
const DELAY_MASK: usize = DELAY_CAPACITY - 1;
const LATENCY_CROSSFADE_SAMPLES: u32 = 64;

/// Persistent processing faults and aggregate graph diagnostics. Node vector
/// edits happen on the control thread while audio processing is stopped.
pub struct AudioGraphRuntime {
    faulted_nodes: Vec<AtomicBool>,
    sanitized_samples: AtomicU64,
    rejected_blocks: AtomicU64,
    processor_faults: AtomicU64,
    watchdog_trips: AtomicU64,
}

impl Default for AudioGraphRuntime {
    fn default() -> Self {
        Self {
            faulted_nodes: Vec::new(),
            sanitized_samples: AtomicU64::new(0),
            rejected_blocks: AtomicU64::new(0),
            processor_faults: AtomicU64::new(0),
            watchdog_trips: AtomicU64::new(0),
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_audio_graph_runtime_create() -> *mut c_void {
    Box::into_raw(Box::new(AudioGraphRuntime::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<AudioGraphRuntime>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_reset_nodes(state: *mut c_void, count: usize) {
    let Some(runtime) = (unsafe { state.cast::<AudioGraphRuntime>().as_mut() }) else {
        return;
    };
    runtime.faulted_nodes.clear();
    runtime
        .faulted_nodes
        .resize_with(count, || AtomicBool::new(false));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_append_node(state: *mut c_void) -> bool {
    let Some(runtime) = (unsafe { state.cast::<AudioGraphRuntime>().as_mut() }) else {
        return false;
    };
    runtime.faulted_nodes.push(AtomicBool::new(false));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_remove_node(
    state: *mut c_void,
    index: usize,
) -> bool {
    let Some(runtime) = (unsafe { state.cast::<AudioGraphRuntime>().as_mut() }) else {
        return false;
    };
    if index >= runtime.faulted_nodes.len() {
        return false;
    }
    runtime.faulted_nodes.remove(index);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_reset_node_fault(
    state: *const c_void,
    index: usize,
) {
    let Some(runtime) = (unsafe { state.cast::<AudioGraphRuntime>().as_ref() }) else {
        return;
    };
    if let Some(faulted) = runtime.faulted_nodes.get(index) {
        faulted.store(false, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_faulted_node_count(
    state: *const c_void,
) -> u32 {
    let Some(runtime) = (unsafe { state.cast::<AudioGraphRuntime>().as_ref() }) else {
        return 0;
    };
    runtime
        .faulted_nodes
        .iter()
        .filter(|faulted| faulted.load(Ordering::Relaxed))
        .count() as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_metric(
    state: *const c_void,
    metric: u32,
) -> u64 {
    let Some(runtime) = (unsafe { state.cast::<AudioGraphRuntime>().as_ref() }) else {
        return 0;
    };
    match metric {
        0 => runtime.sanitized_samples.load(Ordering::Relaxed),
        1 => runtime.rejected_blocks.load(Ordering::Relaxed),
        2 => runtime.processor_faults.load(Ordering::Relaxed),
        3 => runtime.watchdog_trips.load(Ordering::Relaxed),
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_runtime_note_rejected(state: *const c_void) {
    if let Some(runtime) = unsafe { state.cast::<AudioGraphRuntime>().as_ref() } {
        runtime.rejected_blocks.fetch_add(1, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_total_latency(
    user_data: *mut c_void,
    node_count: u32,
    read_node: Option<GraphNodeMetrics>,
) -> u32 {
    let Some(read_node) = read_node else {
        return 0;
    };
    let mut total = 0_u32;
    for index in 0..node_count {
        let mut latency = 0_u32;
        let mut mix = 1.0_f32;
        if unsafe { read_node(user_data, index, &mut latency, &mut mix) } {
            total = total.saturating_add(latency);
        }
    }
    total
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_validate_nodes(
    user_data: *mut c_void,
    node_count: u32,
    prepared: bool,
    read_node: Option<GraphNodeMetrics>,
) -> bool {
    if !prepared {
        return false;
    }
    let Some(read_node) = read_node else {
        return false;
    };
    for index in 0..node_count {
        let mut latency = 0_u32;
        let mut mix = 0.0_f32;
        if !unsafe { read_node(user_data, index, &mut latency, &mut mix) }
            || latency > 65_535
            || !mix.is_finite()
            || !(0.0..=1.0).contains(&mix)
        {
            return false;
        }
    }
    true
}

struct RingDelay {
    samples: Box<[f32; DELAY_CAPACITY]>,
    write_index: usize,
}

impl RingDelay {
    fn new() -> Self {
        Self {
            samples: Box::new([0.0; DELAY_CAPACITY]),
            write_index: 0,
        }
    }

    fn process(&mut self, sample: f32, delay: u32) -> f32 {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        self.samples[self.write_index] = sample;
        let read_index = self
            .write_index
            .wrapping_sub(delay.min(DELAY_MASK as u32) as usize)
            & DELAY_MASK;
        let output = self.samples[read_index];
        self.write_index = (self.write_index + 1) & DELAY_MASK;
        if output.is_finite() {
            output
        } else {
            0.0
        }
    }
}

pub struct AudioGraphDryState {
    delays: [[RingDelay; 2]; 2],
    active_latency: u32,
    pending_latency: u32,
    transition_remaining: u32,
    active_line: usize,
}

impl Default for AudioGraphDryState {
    fn default() -> Self {
        Self {
            delays: std::array::from_fn(|_| std::array::from_fn(|_| RingDelay::new())),
            active_latency: 0,
            pending_latency: 0,
            transition_remaining: 0,
            active_line: 0,
        }
    }
}

impl AudioGraphDryState {
    fn process(
        &mut self,
        sources: &[*const f32],
        destinations: &[*mut f32],
        frames: usize,
        latency: u32,
    ) -> bool {
        if sources.is_empty()
            || sources.len() > 2
            || sources.len() != destinations.len()
            || frames == 0
            || latency > DELAY_MASK as u32
            || sources.iter().any(|source| source.is_null())
            || destinations.iter().any(|destination| destination.is_null())
        {
            return false;
        }
        if latency != self.active_latency && latency != self.pending_latency {
            self.pending_latency = latency;
            self.transition_remaining = LATENCY_CROSSFADE_SAMPLES;
        }

        for frame in 0..frames {
            let transitioning = self.transition_remaining > 0;
            let progress = if transitioning {
                1.0 - self.transition_remaining as f32 / LATENCY_CROSSFADE_SAMPLES as f32
            } else {
                1.0
            };
            for channel in 0..sources.len() {
                let input = unsafe { *sources[channel].add(frame) };
                let old =
                    self.delays[channel][self.active_line].process(input, self.active_latency);
                let new =
                    self.delays[channel][1 - self.active_line].process(input, self.pending_latency);
                let output = if transitioning {
                    old * (1.0 - progress) + new * progress
                } else {
                    new
                };
                unsafe { *destinations[channel].add(frame) = output };
            }
            if transitioning {
                self.transition_remaining -= 1;
                if self.transition_remaining == 0 {
                    self.active_line = 1 - self.active_line;
                    self.active_latency = self.pending_latency;
                }
            }
        }
        true
    }
}

#[no_mangle]
pub extern "C" fn hirari_audio_graph_dry_state_create() -> *mut c_void {
    Box::into_raw(Box::new(AudioGraphDryState::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_dry_state_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the opaque state was returned by the create function.
        unsafe { drop(Box::from_raw(state.cast::<AudioGraphDryState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_process_dry(
    state: *mut c_void,
    sources: *const *const f32,
    destinations: *const *mut f32,
    channels: u32,
    frames: u32,
    latency: u32,
) -> bool {
    if state.is_null()
        || sources.is_null()
        || destinations.is_null()
        || channels == 0
        || channels > 2
    {
        return false;
    }
    let sources = unsafe { std::slice::from_raw_parts(sources, channels as usize) };
    let destinations = unsafe { std::slice::from_raw_parts(destinations, channels as usize) };
    unsafe { &mut *state.cast::<AudioGraphDryState>() }.process(
        sources,
        destinations,
        frames as usize,
        latency,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_sanitize(
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) -> u64 {
    if channels.is_null() || channel_count == 0 || frames == 0 {
        return 0;
    }
    let channels = unsafe { std::slice::from_raw_parts(channels, channel_count as usize) };
    let mut sanitized = 0u64;
    for channel in channels {
        if channel.is_null() {
            continue;
        }
        for frame in 0..frames as usize {
            let sample = unsafe { &mut *channel.add(frame) };
            if !sample.is_finite() {
                *sample = 0.0;
                sanitized = sanitized.saturating_add(1);
            }
        }
    }
    sanitized
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_blend(
    wet_channels: *const *mut f32,
    dry_channels: *const *const f32,
    channel_count: u32,
    frames: u32,
    mix: f32,
) -> bool {
    if wet_channels.is_null()
        || dry_channels.is_null()
        || channel_count == 0
        || channel_count > 2
        || frames == 0
        || !mix.is_finite()
        || !(0.0..=1.0).contains(&mix)
    {
        return false;
    }
    let wet_channels = unsafe { std::slice::from_raw_parts(wet_channels, channel_count as usize) };
    let dry_channels = unsafe { std::slice::from_raw_parts(dry_channels, channel_count as usize) };
    if wet_channels.iter().any(|channel| channel.is_null())
        || dry_channels.iter().any(|channel| channel.is_null())
    {
        return false;
    }
    let dry_mix = 1.0 - mix;
    for channel in 0..channel_count as usize {
        for frame in 0..frames as usize {
            let wet = unsafe { &mut *wet_channels[channel].add(frame) };
            let dry = unsafe { *dry_channels[channel].add(frame) };
            *wet = *wet * mix + dry * dry_mix;
        }
    }
    true
}

unsafe fn clear_channels(channels: *const *mut f32, channel_count: u32, frames: usize) {
    if channels.is_null() {
        return;
    }
    for channel in 0..channel_count as usize {
        let samples = unsafe { *channels.add(channel) };
        if samples.is_null() {
            continue;
        }
        for frame in 0..frames {
            unsafe { samples.add(frame).write(0.0) };
        }
    }
}

unsafe fn reject_graph_block(
    runtime: &AudioGraphRuntime,
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) -> bool {
    runtime.rejected_blocks.fetch_add(1, Ordering::Relaxed);
    if channel_count <= 2 {
        unsafe { clear_channels(channels, channel_count, frames as usize) };
    }
    false
}

/// Owns the real-time traversal policy for the C++ processor graph while
/// callbacks retain the host's processor objects and their lifecycle.
///
/// # Safety
/// Channel tables, per-node delay handles, callback data, and callbacks must
/// remain valid for the entire call. Callbacks must not unwind across C ABI.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_graph_process_nodes(
    runtime_state: *const c_void,
    user_data: *mut c_void,
    channels: *const *mut f32,
    dry_channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
    node_count: u32,
    node_info: Option<GraphNodeInfo>,
    process_node: Option<GraphProcessNode>,
) -> bool {
    let Some(runtime) = (unsafe { runtime_state.cast::<AudioGraphRuntime>().as_ref() }) else {
        return false;
    };
    if channels.is_null()
        || dry_channels.is_null()
        || channel_count == 0
        || channel_count > 2
        || frames == 0
        || runtime.faulted_nodes.len() != node_count as usize
    {
        return unsafe { reject_graph_block(runtime, channels, channel_count, frames) };
    }
    let (Some(node_info), Some(process_node)) = (node_info, process_node) else {
        return unsafe { reject_graph_block(runtime, channels, channel_count, frames) };
    };
    let channels = unsafe { std::slice::from_raw_parts(channels, channel_count as usize) };
    let dry_channels = unsafe { std::slice::from_raw_parts(dry_channels, channel_count as usize) };
    if channels.iter().any(|channel| channel.is_null())
        || dry_channels.iter().any(|channel| channel.is_null())
    {
        return unsafe { reject_graph_block(runtime, channels.as_ptr(), channel_count, frames) };
    }
    let frames = frames as usize;
    let mut sanitized =
        unsafe { hirari_audio_graph_sanitize(channels.as_ptr(), channel_count, frames as u32) };

    for index in 0..node_count {
        let Some(fault_state) = runtime.faulted_nodes.get(index as usize) else {
            return unsafe {
                reject_graph_block(runtime, channels.as_ptr(), channel_count, frames as u32)
            };
        };
        let mut bypassed = false;
        let faulted = fault_state.load(Ordering::Relaxed);
        let mut mix = 1.0f32;
        let mut latency = 0u32;
        let mut delay_state = std::ptr::null_mut();
        if !unsafe {
            node_info(
                user_data,
                index,
                &mut bypassed,
                &mut mix,
                &mut latency,
                &mut delay_state,
            )
        } {
            return unsafe {
                reject_graph_block(runtime, channels.as_ptr(), channel_count, frames as u32)
            };
        }
        if bypassed || faulted {
            continue;
        }
        if !mix.is_finite() || !(0.0..=1.0).contains(&mix) || latency > 65_535 {
            return unsafe {
                reject_graph_block(runtime, channels.as_ptr(), channel_count, frames as u32)
            };
        }
        if mix < 1.0 {
            if delay_state.is_null() {
                // The C++ graph historically skipped a node when its dry
                // delay state was unavailable during an unprepared update.
                continue;
            }
            let sources = [
                channels[0].cast_const(),
                if channel_count > 1 {
                    channels[1].cast_const()
                } else {
                    std::ptr::null()
                },
            ];
            let destinations = [
                dry_channels[0],
                if channel_count > 1 {
                    dry_channels[1]
                } else {
                    std::ptr::null_mut()
                },
            ];
            if !unsafe {
                hirari_audio_graph_process_dry(
                    delay_state,
                    sources.as_ptr(),
                    destinations.as_ptr(),
                    channel_count,
                    frames as u32,
                    latency,
                )
            } {
                return unsafe {
                    reject_graph_block(runtime, channels.as_ptr(), channel_count, frames as u32)
                };
            }
        }

        let result = unsafe { process_node(user_data, index) };
        if result != 0 {
            fault_state.store(true, Ordering::Relaxed);
            match result {
                1 => runtime.processor_faults.fetch_add(1, Ordering::Relaxed),
                2 => runtime.watchdog_trips.fetch_add(1, Ordering::Relaxed),
                _ => 0,
            };
            unsafe {
                clear_channels(channels.as_ptr(), channel_count, frames);
            }
            continue;
        }
        sanitized = sanitized.saturating_add(unsafe {
            hirari_audio_graph_sanitize(channels.as_ptr(), channel_count, frames as u32)
        });
        if mix < 1.0 {
            let sources = [
                dry_channels[0].cast_const(),
                if channel_count > 1 {
                    dry_channels[1].cast_const()
                } else {
                    std::ptr::null()
                },
            ];
            if !unsafe {
                hirari_audio_graph_blend(
                    channels.as_ptr(),
                    sources.as_ptr(),
                    channel_count,
                    frames as u32,
                    mix,
                )
            } {
                return unsafe {
                    reject_graph_block(runtime, channels.as_ptr(), channel_count, frames as u32)
                };
            }
        }
        sanitized = sanitized.saturating_add(unsafe {
            hirari_audio_graph_sanitize(channels.as_ptr(), channel_count, frames as u32)
        });
    }
    runtime
        .sanitized_samples
        .fetch_add(sanitized, Ordering::Relaxed);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MetricsFixture(Vec<Option<(u32, f32)>>);

    unsafe extern "C" fn read_node_metrics(
        user_data: *mut c_void,
        index: u32,
        latency: *mut u32,
        mix: *mut f32,
    ) -> bool {
        let fixture = unsafe { &*user_data.cast::<MetricsFixture>() };
        let Some(Some((node_latency, node_mix))) = fixture.0.get(index as usize) else {
            return false;
        };
        unsafe {
            latency.write(*node_latency);
            mix.write(*node_mix);
        }
        true
    }

    #[test]
    fn graph_latency_saturates_and_validation_checks_all_node_metadata() {
        let mut fixture = MetricsFixture(vec![Some((100, 0.5)), Some((200, 1.0)), None]);
        let context = (&mut fixture as *mut MetricsFixture).cast();
        assert_eq!(
            unsafe { hirari_audio_graph_total_latency(context, 3, Some(read_node_metrics)) },
            300
        );
        assert!(unsafe {
            hirari_audio_graph_validate_nodes(context, 2, true, Some(read_node_metrics))
        });
        assert!(!unsafe {
            hirari_audio_graph_validate_nodes(context, 2, false, Some(read_node_metrics))
        });
        assert!(!unsafe {
            hirari_audio_graph_validate_nodes(context, 3, true, Some(read_node_metrics))
        });

        fixture.0[0] = Some((u32::MAX - 3, 0.5));
        fixture.0[1] = Some((10, 1.0));
        assert_eq!(
            unsafe { hirari_audio_graph_total_latency(context, 2, Some(read_node_metrics)) },
            u32::MAX
        );
        assert!(!unsafe {
            hirari_audio_graph_validate_nodes(context, 2, true, Some(read_node_metrics))
        });

        fixture.0[0] = Some((100, 0.5));
        fixture.0[1] = Some((65_536, 0.5));
        assert!(!unsafe {
            hirari_audio_graph_validate_nodes(context, 2, true, Some(read_node_metrics))
        });
        fixture.0[1] = Some((0, f32::NAN));
        assert!(!unsafe {
            hirari_audio_graph_validate_nodes(context, 2, true, Some(read_node_metrics))
        });
    }

    #[test]
    fn dry_latency_transition_is_linked_across_stereo_channels() {
        let mut state = AudioGraphDryState::default();
        let left = [1.0, 0.0, 0.0, 0.0];
        let right = left;
        let mut dry_left = [0.0; 4];
        let mut dry_right = [0.0; 4];
        assert!(state.process(
            &[left.as_ptr(), right.as_ptr()],
            &[dry_left.as_mut_ptr(), dry_right.as_mut_ptr()],
            4,
            2,
        ));
        assert_eq!(dry_left, dry_right);
        assert_eq!(state.transition_remaining, LATENCY_CROSSFADE_SAMPLES - 4);
        assert_eq!(state.active_latency, 0);
    }

    #[test]
    fn sanitize_and_wet_dry_blend_operate_on_planar_blocks() {
        let mut left = [f32::NAN, 2.0];
        let mut right = [f32::INFINITY, -2.0];
        let mut wet_l = [1.0, 1.0];
        let mut wet_r = [1.0, 1.0];
        let dry_l = [0.0, 0.0];
        let dry_r = [0.0, 0.0];
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        assert_eq!(
            unsafe { hirari_audio_graph_sanitize(channels.as_mut_ptr(), 2, 2) },
            2
        );
        assert!(unsafe {
            hirari_audio_graph_blend(
                [wet_l.as_mut_ptr(), wet_r.as_mut_ptr()].as_ptr(),
                [dry_l.as_ptr(), dry_r.as_ptr()].as_ptr(),
                2,
                2,
                0.25,
            )
        });
        assert_eq!(wet_l, [0.25, 0.25]);
        assert_eq!(wet_r, [0.25, 0.25]);
    }

    struct GraphFixture {
        dry_state: *mut c_void,
        left: *mut f32,
        right: *mut f32,
        mix: f32,
        next_process_result: u8,
    }

    unsafe extern "C" fn node_info(
        user_data: *mut c_void,
        index: u32,
        bypassed: *mut bool,
        mix: *mut f32,
        latency: *mut u32,
        dry_state: *mut *mut c_void,
    ) -> bool {
        if index != 0 {
            return false;
        }
        let fixture = unsafe { &mut *user_data.cast::<GraphFixture>() };
        unsafe {
            bypassed.write(false);
            mix.write(fixture.mix);
            latency.write(0);
            dry_state.write(fixture.dry_state);
        }
        true
    }

    unsafe extern "C" fn process_node(user_data: *mut c_void, _index: u32) -> u8 {
        let fixture = unsafe { &mut *user_data.cast::<GraphFixture>() };
        if fixture.next_process_result != 0 {
            return std::mem::take(&mut fixture.next_process_result);
        }
        for frame in 0..8 {
            unsafe {
                fixture
                    .left
                    .add(frame)
                    .write(*fixture.left.add(frame) * 2.0);
                fixture
                    .right
                    .add(frame)
                    .write(*fixture.right.add(frame) * 2.0);
            }
        }
        0
    }

    #[test]
    fn rust_graph_traversal_runs_wet_dry_and_accepts_fully_dry_mix() {
        let dry_state = hirari_audio_graph_dry_state_create();
        assert!(!dry_state.is_null());
        let runtime = hirari_audio_graph_runtime_create();
        assert!(!runtime.is_null());
        unsafe { hirari_audio_graph_runtime_reset_nodes(runtime, 1) };
        let mut left = [1.0f32; 8];
        let mut right = [-1.0f32; 8];
        let mut dry_left = [0.0f32; 8];
        let mut dry_right = [0.0f32; 8];
        let channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let dry_channels = [dry_left.as_mut_ptr(), dry_right.as_mut_ptr()];
        let mut fixture = GraphFixture {
            dry_state,
            left: left.as_mut_ptr(),
            right: right.as_mut_ptr(),
            mix: 0.0,
            next_process_result: 0,
        };
        assert!(unsafe {
            hirari_audio_graph_process_nodes(
                runtime,
                (&mut fixture as *mut GraphFixture).cast(),
                channels.as_ptr(),
                dry_channels.as_ptr(),
                2,
                8,
                1,
                Some(node_info),
                Some(process_node),
            )
        });
        assert_eq!(left, [1.0; 8]);
        assert_eq!(right, [-1.0; 8]);
        left.fill(1.0);
        right.fill(-1.0);
        fixture.mix = 0.5;
        assert!(unsafe {
            hirari_audio_graph_process_nodes(
                runtime,
                (&mut fixture as *mut GraphFixture).cast(),
                channels.as_ptr(),
                dry_channels.as_ptr(),
                2,
                8,
                1,
                Some(node_info),
                Some(process_node),
            )
        });
        assert_eq!(left, [1.5; 8]);
        assert_eq!(right, [-1.5; 8]);
        left.fill(1.0);
        right.fill(-1.0);
        fixture.mix = 1.0;
        assert!(unsafe {
            hirari_audio_graph_process_nodes(
                runtime,
                (&mut fixture as *mut GraphFixture).cast(),
                channels.as_ptr(),
                dry_channels.as_ptr(),
                2,
                8,
                1,
                Some(node_info),
                Some(process_node),
            )
        });
        assert_eq!(left, [2.0; 8]);
        assert_eq!(right, [-2.0; 8]);
        fixture.next_process_result = 2;
        left.fill(1.0);
        right.fill(-1.0);
        assert!(unsafe {
            hirari_audio_graph_process_nodes(
                runtime,
                (&mut fixture as *mut GraphFixture).cast(),
                channels.as_ptr(),
                dry_channels.as_ptr(),
                2,
                8,
                1,
                Some(node_info),
                Some(process_node),
            )
        });
        assert_eq!(left, [0.0; 8]);
        assert_eq!(right, [0.0; 8]);
        assert_eq!(
            unsafe { hirari_audio_graph_runtime_faulted_node_count(runtime) },
            1
        );
        assert_eq!(unsafe { hirari_audio_graph_runtime_metric(runtime, 3) }, 1);
        assert_eq!(unsafe { hirari_audio_graph_runtime_metric(runtime, 0) }, 0);
        unsafe { hirari_audio_graph_runtime_reset_node_fault(runtime, 0) };
        fixture.next_process_result = 1;
        left.fill(1.0);
        right.fill(-1.0);
        assert!(unsafe {
            hirari_audio_graph_process_nodes(
                runtime,
                (&mut fixture as *mut GraphFixture).cast(),
                channels.as_ptr(),
                dry_channels.as_ptr(),
                2,
                8,
                1,
                Some(node_info),
                Some(process_node),
            )
        });
        assert_eq!(unsafe { hirari_audio_graph_runtime_metric(runtime, 2) }, 1);
        assert_eq!(
            unsafe { hirari_audio_graph_runtime_faulted_node_count(runtime) },
            1
        );
        unsafe { hirari_audio_graph_runtime_reset_node_fault(runtime, 0) };
        fixture.mix = 2.0;
        left.fill(1.0);
        right.fill(-1.0);
        assert!(!unsafe {
            hirari_audio_graph_process_nodes(
                runtime,
                (&mut fixture as *mut GraphFixture).cast(),
                channels.as_ptr(),
                dry_channels.as_ptr(),
                2,
                8,
                1,
                Some(node_info),
                Some(process_node),
            )
        });
        assert_eq!(left, [0.0; 8]);
        assert_eq!(right, [0.0; 8]);
        assert_eq!(unsafe { hirari_audio_graph_runtime_metric(runtime, 1) }, 1);
        unsafe { hirari_audio_graph_runtime_destroy(runtime) };
        unsafe { hirari_audio_graph_dry_state_destroy(dry_state) };
    }
}
