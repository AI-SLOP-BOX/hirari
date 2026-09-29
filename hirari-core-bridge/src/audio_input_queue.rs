//! Fixed-size SPSC queue used between native device callbacks and the capture
//! worker. All storage is allocated before callbacks start; push/poll only copy
//! bounded sample blocks and update atomics.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const CAPACITY: usize = 8;
const MAX_CHANNELS: usize = 32;
const MAX_FRAMES: usize = 8192;
const MAX_SAMPLES_PER_SLOT: usize = MAX_CHANNELS * MAX_FRAMES;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlockInfo {
    channels: u32,
    frames: u32,
}

struct Slot {
    samples: UnsafeCell<Box<[f32]>>,
    info: UnsafeCell<BlockInfo>,
}

impl Slot {
    fn new() -> Self {
        Self {
            samples: UnsafeCell::new(vec![0.0; MAX_SAMPLES_PER_SLOT].into_boxed_slice()),
            info: UnsafeCell::new(BlockInfo::default()),
        }
    }
}

#[repr(align(64))]
struct CachePadded<T>(T);

pub struct AudioInputBlockQueue {
    slots: Box<[Slot]>,
    write_index: CachePadded<AtomicU64>,
    read_index: CachePadded<AtomicU64>,
    dropped_blocks: AtomicU64,
}

// Queue slot access is exclusive by index: only the producer writes the
// current write slot and only the consumer reads the published read slot.
unsafe impl Send for AudioInputBlockQueue {}
unsafe impl Sync for AudioInputBlockQueue {}

impl AudioInputBlockQueue {
    fn new() -> Self {
        let slots = (0..CAPACITY)
            .map(|_| Slot::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            write_index: CachePadded(AtomicU64::new(0)),
            read_index: CachePadded(AtomicU64::new(0)),
            dropped_blocks: AtomicU64::new(0),
        }
    }

    unsafe fn push_planar(
        &self,
        channels: *const *const f32,
        channel_count: u32,
        frame_count: u32,
    ) -> bool {
        let write = self.write_index.0.load(Ordering::Relaxed);
        let read = self.read_index.0.load(Ordering::Acquire);
        if channels.is_null()
            || channel_count == 0
            || channel_count as usize > MAX_CHANNELS
            || frame_count == 0
            || frame_count as usize > MAX_FRAMES
            || write.wrapping_sub(read) >= CAPACITY as u64
        {
            self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        let slot = &self.slots[write as usize & (CAPACITY - 1)];
        let samples = &mut *slot.samples.get();
        for channel in 0..channel_count as usize {
            let input = *channels.add(channel);
            if input.is_null() {
                self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            ptr::copy_nonoverlapping(
                input,
                samples.as_mut_ptr().add(channel * MAX_FRAMES),
                frame_count as usize,
            );
        }
        *slot.info.get() = BlockInfo {
            channels: channel_count,
            frames: frame_count,
        };
        self.write_index
            .0
            .store(write.wrapping_add(1), Ordering::Release);
        true
    }

    unsafe fn poll(
        &self,
        destination: *const *mut f32,
        destination_channels: u32,
        destination_frames: u32,
        channel_count_out: *mut u32,
        frame_count_out: *mut u32,
        dropped_out: *mut u64,
    ) -> bool {
        if dropped_out.is_null() || channel_count_out.is_null() || frame_count_out.is_null() {
            return false;
        }
        let mut dropped = self.dropped_blocks.swap(0, Ordering::AcqRel);
        *dropped_out = dropped;
        let read = self.read_index.0.load(Ordering::Relaxed);
        let write = self.write_index.0.load(Ordering::Acquire);
        if read == write {
            return false;
        }

        let slot = &self.slots[read as usize & (CAPACITY - 1)];
        let info = *slot.info.get();
        *channel_count_out = info.channels;
        *frame_count_out = info.frames;
        if destination.is_null()
            || info.channels > destination_channels
            || info.frames > destination_frames
        {
            self.drop_read_slot(read, &mut dropped, dropped_out);
            return false;
        }

        let samples = &*slot.samples.get();
        for channel in 0..info.channels as usize {
            let output = *destination.add(channel);
            if output.is_null() {
                self.drop_read_slot(read, &mut dropped, dropped_out);
                return false;
            }
            ptr::copy_nonoverlapping(
                samples.as_ptr().add(channel * MAX_FRAMES),
                output,
                info.frames as usize,
            );
        }
        self.read_index
            .0
            .store(read.wrapping_add(1), Ordering::Release);
        true
    }

    unsafe fn drop_read_slot(&self, read: u64, dropped: &mut u64, dropped_out: *mut u64) {
        self.read_index
            .0
            .store(read.wrapping_add(1), Ordering::Release);
        self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
        if *dropped != u64::MAX {
            *dropped += 1;
        }
        *dropped_out = *dropped;
    }

    fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks.load(Ordering::Acquire)
    }

    fn discard_pending(&self) {
        let write = self.write_index.0.load(Ordering::Acquire);
        self.read_index.0.store(write, Ordering::Release);
        self.dropped_blocks.swap(0, Ordering::AcqRel);
    }

    fn reset(&self) {
        let write = self.write_index.0.load(Ordering::Relaxed);
        self.read_index.0.store(write, Ordering::Relaxed);
        self.dropped_blocks.store(0, Ordering::Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn hirari_audio_input_queue_create() -> *mut c_void {
    Box::into_raw(Box::new(AudioInputBlockQueue::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_input_queue_free(queue: *mut c_void) {
    if !queue.is_null() {
        drop(Box::from_raw(queue.cast::<AudioInputBlockQueue>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_input_queue_push(
    queue: *const c_void,
    channels: *const *const f32,
    channel_count: u32,
    frame_count: u32,
) -> bool {
    queue
        .cast::<AudioInputBlockQueue>()
        .as_ref()
        .is_some_and(|queue| queue.push_planar(channels, channel_count, frame_count))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_input_queue_poll(
    queue: *const c_void,
    destination: *const *mut f32,
    destination_channels: u32,
    destination_frames: u32,
    channel_count_out: *mut u32,
    frame_count_out: *mut u32,
    dropped_out: *mut u64,
) -> bool {
    queue
        .cast::<AudioInputBlockQueue>()
        .as_ref()
        .is_some_and(|queue| {
            queue.poll(
                destination,
                destination_channels,
                destination_frames,
                channel_count_out,
                frame_count_out,
                dropped_out,
            )
        })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_input_queue_dropped(queue: *const c_void) -> u64 {
    queue
        .cast::<AudioInputBlockQueue>()
        .as_ref()
        .map_or(0, AudioInputBlockQueue::dropped_blocks)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_input_queue_discard(queue: *const c_void) {
    if let Some(queue) = queue.cast::<AudioInputBlockQueue>().as_ref() {
        queue.discard_pending();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_input_queue_reset(queue: *const c_void) {
    if let Some(queue) = queue.cast::<AudioInputBlockQueue>().as_ref() {
        queue.reset();
    }
}
