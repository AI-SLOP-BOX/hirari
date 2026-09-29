use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

const SIDECHAIN_SLOTS: usize = 256;
const SIDECHAIN_BLOCK: usize = 4096;
const EMPTY_SIDECHAIN_KEY: u64 = u64::MAX;

struct SidechainSlot {
    key: AtomicU64,
    source_track: AtomicU32,
    level_bits: AtomicU32,
    tap_point: AtomicU8,
    source_frames: AtomicU32,
    source_sample_rate: AtomicU32,
    source_generation: AtomicU64,
    generation: AtomicU64,
    published_buffer: AtomicU32,
    owned: AtomicBool,
    owned_frames: [AtomicU32; 2],
    owned_sample_rates: [AtomicU32; 2],
    owned_source_generations: [AtomicU64; 2],
    owned_generations: [AtomicU64; 2],
    buffers: [UnsafeCell<Option<Box<[f32]>>>; 4],
}

impl SidechainSlot {
    fn new() -> Self {
        Self {
            key: AtomicU64::new(EMPTY_SIDECHAIN_KEY),
            source_track: AtomicU32::new(0),
            level_bits: AtomicU32::new(1.0f32.to_bits()),
            tap_point: AtomicU8::new(1),
            source_frames: AtomicU32::new(0),
            source_sample_rate: AtomicU32::new(0),
            source_generation: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            published_buffer: AtomicU32::new(0),
            owned: AtomicBool::new(false),
            owned_frames: std::array::from_fn(|_| AtomicU32::new(0)),
            owned_sample_rates: std::array::from_fn(|_| AtomicU32::new(0)),
            owned_source_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            owned_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            buffers: std::array::from_fn(|_| UnsafeCell::new(None)),
        }
    }

    fn reset(&self) {
        self.key.store(EMPTY_SIDECHAIN_KEY, Ordering::Release);
        self.source_track.store(0, Ordering::Relaxed);
        self.level_bits.store(1.0f32.to_bits(), Ordering::Relaxed);
        self.tap_point.store(1, Ordering::Relaxed);
        self.source_frames.store(0, Ordering::Relaxed);
        self.source_sample_rate.store(0, Ordering::Relaxed);
        self.source_generation.store(0, Ordering::Relaxed);
        self.generation.store(0, Ordering::Relaxed);
        self.published_buffer.store(0, Ordering::Relaxed);
        self.owned.store(false, Ordering::Relaxed);
        for index in 0..2 {
            self.owned_frames[index].store(0, Ordering::Relaxed);
            self.owned_sample_rates[index].store(0, Ordering::Relaxed);
            self.owned_source_generations[index].store(0, Ordering::Relaxed);
            self.owned_generations[index].store(0, Ordering::Relaxed);
        }
    }

    fn ensure_buffers(&self) -> bool {
        // SAFETY: callers hold the control mutation gate, excluding all readers
        // and the audio publisher while lazy buffer allocation occurs.
        for buffer in &self.buffers {
            // SAFETY: callers hold the control mutation gate.
            let buffer = unsafe { &mut *buffer.get() };
            if buffer.is_none() {
                let mut storage = Vec::new();
                if storage.try_reserve_exact(SIDECHAIN_BLOCK).is_err() {
                    return false;
                }
                storage.resize(SIDECHAIN_BLOCK, 0.0);
                *buffer = Some(storage.into_boxed_slice());
            }
        }
        true
    }
}

// The publication gate grants exclusive mutation access or immutable readers.
unsafe impl Sync for SidechainSlot {}

struct PublicationGate {
    readers: AtomicU32,
    control_mutation: AtomicBool,
    audio_publishing: AtomicBool,
    slots: [SidechainSlot; SIDECHAIN_SLOTS],
}

#[no_mangle]
pub extern "C" fn hirari_sidechain_publication_gate_create() -> *mut c_void {
    Box::into_raw(Box::new(PublicationGate {
        readers: AtomicU32::new(0),
        control_mutation: AtomicBool::new(false),
        audio_publishing: AtomicBool::new(false),
        slots: std::array::from_fn(|_| SidechainSlot::new()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_destroy(gate: *mut c_void) {
    if !gate.is_null() {
        // Prevent new entrants and drain active calls before reclaiming storage.
        let runtime = unsafe { &*gate.cast::<PublicationGate>() };
        runtime.control_mutation.store(true, Ordering::SeqCst);
        while runtime.audio_publishing.load(Ordering::SeqCst)
            || runtime.readers.load(Ordering::SeqCst) != 0
        {
            std::thread::yield_now();
        }
        // SAFETY: admission is closed and all active calls have drained.
        drop(unsafe { Box::from_raw(gate.cast::<PublicationGate>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_enter_reader(
    gate: *const c_void,
) -> bool {
    let Some(gate) = (unsafe { gate.cast::<PublicationGate>().as_ref() }) else {
        return false;
    };
    if gate.control_mutation.load(Ordering::SeqCst) {
        return false;
    }
    gate.readers.fetch_add(1, Ordering::SeqCst);
    if gate.control_mutation.load(Ordering::SeqCst) {
        gate.readers.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_leave_reader(gate: *const c_void) {
    if let Some(gate) = unsafe { gate.cast::<PublicationGate>().as_ref() } {
        gate.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_begin_control(gate: *const c_void) {
    let Some(gate) = (unsafe { gate.cast::<PublicationGate>().as_ref() }) else {
        return;
    };
    gate.control_mutation.store(true, Ordering::SeqCst);
    while gate.audio_publishing.load(Ordering::SeqCst)
        || gate.readers.load(Ordering::SeqCst) != 0
    {
        std::thread::yield_now();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_end_control(gate: *const c_void) {
    if let Some(gate) = unsafe { gate.cast::<PublicationGate>().as_ref() } {
        gate.control_mutation.store(false, Ordering::SeqCst);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_try_begin_audio(
    gate: *const c_void,
) -> bool {
    let Some(gate) = (unsafe { gate.cast::<PublicationGate>().as_ref() }) else {
        return false;
    };
    if gate.control_mutation.load(Ordering::SeqCst)
        || gate.readers.load(Ordering::SeqCst) != 0
        || gate
            .audio_publishing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return false;
    }
    if gate.control_mutation.load(Ordering::SeqCst)
        || gate.readers.load(Ordering::SeqCst) != 0
    {
        gate.audio_publishing.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publication_gate_end_audio(gate: *const c_void) {
    if let Some(gate) = unsafe { gate.cast::<PublicationGate>().as_ref() } {
        gate.audio_publishing.store(false, Ordering::SeqCst);
    }
}

fn sidechain_key(destination: u32, plugin: u32) -> u64 {
    (u64::from(destination) << 32) | u64::from(plugin)
}

unsafe fn gate_ref<'a>(state: *const c_void) -> Option<&'a PublicationGate> {
    // SAFETY: forwarded from each public FFI function's live runtime handle.
    unsafe { state.cast::<PublicationGate>().as_ref() }
}

fn clear_tail(destination: &mut [f32], copied: usize) {
    destination[copied..].fill(0.0);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_register_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    source_track: u32,
    source_left: *const f32,
    source_right: *const f32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
    level: f32,
    tap_point: u8,
) -> bool {
    if plugin_index >= 65_535
        || source_left.is_null()
        || source_right.is_null()
        || frames == 0
        || frames as usize > SIDECHAIN_BLOCK
        || sample_rate == 0
        || source_generation == 0
        || !level.is_finite()
        || tap_point > 2
    {
        return false;
    }
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return false;
    };
    let key = sidechain_key(destination_track, plugin_index);
    let Some(slot) = runtime
        .slots
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == key)
        .or_else(|| {
            runtime
                .slots
                .iter()
                .find(|slot| slot.key.load(Ordering::Acquire) == EMPTY_SIDECHAIN_KEY)
        })
    else {
        return false;
    };
    if !slot.ensure_buffers() {
        return false;
    }
    let active = slot.published_buffer.load(Ordering::Relaxed) as usize;
    let next = 1 - active.min(1);
    // SAFETY: Control mutation excludes audio publishers/readers; source slices
    // are supplied by the caller for the declared frame count.
    let (Some(left), Some(right)) = (
        unsafe { &mut *slot.buffers[next * 2].get() },
        unsafe { &mut *slot.buffers[next * 2 + 1].get() },
    ) else {
        return false;
    };
    let frames = frames as usize;
    unsafe {
        std::ptr::copy_nonoverlapping(source_left, left.as_mut_ptr(), frames);
        std::ptr::copy_nonoverlapping(source_right, right.as_mut_ptr(), frames);
    }
    clear_tail(left, frames);
    clear_tail(right, frames);
    let generation = slot.generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    slot.source_track.store(source_track, Ordering::Relaxed);
    slot.level_bits.store(level.max(0.0).to_bits(), Ordering::Relaxed);
    slot.tap_point.store(tap_point, Ordering::Relaxed);
    slot.source_frames.store(frames as u32, Ordering::Relaxed);
    slot.source_sample_rate.store(sample_rate, Ordering::Relaxed);
    slot.source_generation.store(source_generation, Ordering::Relaxed);
    slot.owned_frames[next].store(frames as u32, Ordering::Relaxed);
    slot.owned_sample_rates[next].store(sample_rate, Ordering::Relaxed);
    slot.owned_source_generations[next].store(source_generation, Ordering::Relaxed);
    slot.owned_generations[next].store(generation, Ordering::Relaxed);
    slot.owned.store(true, Ordering::Relaxed);
    slot.published_buffer.store(next as u32, Ordering::Release);
    slot.key.store(key, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_register_silent_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    source_track: u32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
    level: f32,
    tap_point: u8,
) -> bool {
    if plugin_index >= 65_535
        || frames == 0
        || frames as usize > SIDECHAIN_BLOCK
        || sample_rate == 0
        || source_generation == 0
        || !level.is_finite()
        || tap_point > 2
    {
        return false;
    }
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return false;
    };
    let key = sidechain_key(destination_track, plugin_index);
    let Some(slot) = runtime
        .slots
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == key)
        .or_else(|| {
            runtime
                .slots
                .iter()
                .find(|slot| slot.key.load(Ordering::Acquire) == EMPTY_SIDECHAIN_KEY)
        })
    else {
        return false;
    };
    if !slot.ensure_buffers() {
        return false;
    }
    let active = slot.published_buffer.load(Ordering::Relaxed) as usize;
    let next = 1 - active.min(1);
    // SAFETY: Control mutation excludes all buffer readers and writers.
    let (Some(left), Some(right)) = (
        unsafe { &mut *slot.buffers[next * 2].get() },
        unsafe { &mut *slot.buffers[next * 2 + 1].get() },
    ) else {
        return false;
    };
    left.fill(0.0);
    right.fill(0.0);
    let generation = slot.generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    slot.source_track.store(source_track, Ordering::Relaxed);
    slot.level_bits.store(level.max(0.0).to_bits(), Ordering::Relaxed);
    slot.tap_point.store(tap_point, Ordering::Relaxed);
    slot.source_frames.store(frames, Ordering::Relaxed);
    slot.source_sample_rate.store(sample_rate, Ordering::Relaxed);
    slot.source_generation.store(source_generation, Ordering::Relaxed);
    slot.owned_frames[next].store(frames, Ordering::Relaxed);
    slot.owned_sample_rates[next].store(sample_rate, Ordering::Relaxed);
    slot.owned_source_generations[next].store(source_generation, Ordering::Relaxed);
    slot.owned_generations[next].store(generation, Ordering::Relaxed);
    slot.owned.store(true, Ordering::Relaxed);
    slot.published_buffer.store(next as u32, Ordering::Release);
    slot.key.store(key, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_remove_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
) -> bool {
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return false;
    };
    let key = sidechain_key(destination_track, plugin_index);
    let Some(slot) = runtime
        .slots
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == key)
    else {
        return false;
    };
    slot.reset();
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_has_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    source_track: u32,
) -> bool {
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return false;
    };
    let key = sidechain_key(destination_track, plugin_index);
    runtime.slots.iter().any(|slot| {
        slot.key.load(Ordering::Acquire) == key
            && slot.source_track.load(Ordering::Relaxed) == source_track
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_level(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
) -> f32 {
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return 0.0;
    };
    let key = sidechain_key(destination_track, plugin_index);
    runtime
        .slots
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == key)
        .map_or(0.0, |slot| f32::from_bits(slot.level_bits.load(Ordering::Relaxed)))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_remove_track(state: *const c_void, track: u32) {
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return;
    };
    for slot in &runtime.slots {
        let key = slot.key.load(Ordering::Acquire);
        if key != EMPTY_SIDECHAIN_KEY
            && ((key >> 32) as u32 == track || slot.source_track.load(Ordering::Relaxed) == track)
        {
            slot.reset();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_reset(state: *const c_void) {
    if let Some(runtime) = unsafe { gate_ref(state) } {
        for slot in &runtime.slots {
            slot.reset();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_copy_link(
    state: *const c_void,
    destination_track: u32,
    plugin_index: u32,
    output_left: *mut f32,
    output_right: *mut f32,
    capacity: u32,
    metadata: *mut u64,
) -> u32 {
    if output_left.is_null() || output_right.is_null() || metadata.is_null() || capacity == 0 {
        return 0;
    }
    let Some(runtime) = (unsafe { gate_ref(state) }) else {
        return 0;
    };
    let key = sidechain_key(destination_track, plugin_index);
    let Some(slot) = runtime.slots.iter().find(|slot| slot.key.load(Ordering::Acquire) == key) else {
        return 0;
    };
    let active = slot.published_buffer.load(Ordering::Acquire) as usize;
    if active > 1 || !slot.owned.load(Ordering::Relaxed) {
        return 0;
    }
    // SAFETY: Reader gate prevents a writer from rotating/replacing storage
    // until both channel copies and metadata reads have completed.
    let (Some(left), Some(right)) = (
        unsafe { &*slot.buffers[active * 2].get() },
        unsafe { &*slot.buffers[active * 2 + 1].get() },
    ) else {
        return 0;
    };
    let frames = slot.owned_frames[active].load(Ordering::Relaxed) as usize;
    if frames == 0 || frames > SIDECHAIN_BLOCK {
        return 0;
    }
    let copied = frames.min(capacity as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(left.as_ptr(), output_left, copied);
        std::ptr::copy_nonoverlapping(right.as_ptr(), output_right, copied);
        let output = std::slice::from_raw_parts_mut(metadata, 6);
        output[0] = u64::from(slot.source_track.load(Ordering::Relaxed));
        output[1] = u64::from(slot.level_bits.load(Ordering::Relaxed));
        output[2] = u64::from(slot.tap_point.load(Ordering::Relaxed));
        output[3] = u64::from(slot.owned_sample_rates[active].load(Ordering::Relaxed));
        output[4] = slot.owned_source_generations[active].load(Ordering::Relaxed);
        output[5] = slot.owned_generations[active].load(Ordering::Relaxed);
    }
    copied as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_refresh_source(
    state: *const c_void,
    source_track: u32,
    left: *const f32,
    right: *const f32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
) -> bool {
    if left.is_null() || right.is_null() || frames == 0 || frames as usize > SIDECHAIN_BLOCK
        || sample_rate == 0 || source_generation == 0
    {
        return false;
    }
    let Some(runtime) = (unsafe { gate_ref(state) }) else { return false };
    let mut refreshed = false;
    for slot in &runtime.slots {
        if slot.key.load(Ordering::Acquire) == EMPTY_SIDECHAIN_KEY
            || slot.source_track.load(Ordering::Relaxed) != source_track
            || slot.source_generation.load(Ordering::Relaxed) != source_generation
            || slot.source_sample_rate.load(Ordering::Relaxed) != sample_rate
            || !slot.owned.load(Ordering::Relaxed)
        {
            continue;
        }
        if !slot.ensure_buffers() { continue }
        let active = slot.published_buffer.load(Ordering::Relaxed) as usize;
        let next = 1 - active.min(1);
        // SAFETY: Caller holds the control mutation gate.
        let (Some(target_left), Some(target_right)) = (
            unsafe { &mut *slot.buffers[next * 2].get() },
            unsafe { &mut *slot.buffers[next * 2 + 1].get() },
        ) else {
            continue;
        };
        let frames = frames as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(left, target_left.as_mut_ptr(), frames);
            std::ptr::copy_nonoverlapping(right, target_right.as_mut_ptr(), frames);
        }
        clear_tail(target_left, frames);
        clear_tail(target_right, frames);
        let generation = slot.generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        slot.owned_frames[next].store(frames as u32, Ordering::Relaxed);
        slot.owned_sample_rates[next].store(sample_rate, Ordering::Relaxed);
        slot.owned_source_generations[next].store(source_generation, Ordering::Relaxed);
        slot.owned_generations[next].store(generation, Ordering::Relaxed);
        slot.source_frames.store(frames as u32, Ordering::Relaxed);
        slot.published_buffer.store(next as u32, Ordering::Release);
        refreshed = true;
    }
    refreshed
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_publish_source(
    state: *const c_void,
    source_track: u32,
    pre_left: *const f32,
    pre_right: *const f32,
    post_left: *const f32,
    post_right: *const f32,
    fader_left: *const f32,
    fader_right: *const f32,
    frames: u32,
    sample_rate: u32,
    source_generation: u64,
) {
    if [pre_left, pre_right, post_left, post_right, fader_left, fader_right]
        .iter()
        .any(|pointer| pointer.is_null())
        || frames == 0
        || frames as usize > SIDECHAIN_BLOCK
        || sample_rate == 0
        || source_generation == 0
    {
        return;
    }
    let Some(runtime) = (unsafe { gate_ref(state) }) else { return };
    for slot in &runtime.slots {
        if slot.key.load(Ordering::Acquire) == EMPTY_SIDECHAIN_KEY
            || slot.source_track.load(Ordering::Relaxed) != source_track
            || !slot.owned.load(Ordering::Relaxed)
        {
            continue;
        }
        let active = slot.published_buffer.load(Ordering::Relaxed) as usize;
        let next = 1 - active.min(1);
        // Audio publication never allocates; registration provisions all buffers.
        let (Some(target_left), Some(target_right)) = (
            unsafe { &mut *slot.buffers[next * 2].get() },
            unsafe { &mut *slot.buffers[next * 2 + 1].get() },
        ) else {
            continue;
        };
        let (source_left, source_right) = match slot.tap_point.load(Ordering::Relaxed) {
            0 => (pre_left, pre_right),
            1 => (post_left, post_right),
            2 => (fader_left, fader_right),
            _ => continue,
        };
        let frames = frames as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(source_left, target_left.as_mut_ptr(), frames);
            std::ptr::copy_nonoverlapping(source_right, target_right.as_mut_ptr(), frames);
        }
        clear_tail(target_left, frames);
        clear_tail(target_right, frames);
        let generation = slot.generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        slot.owned_frames[next].store(frames as u32, Ordering::Relaxed);
        slot.owned_sample_rates[next].store(sample_rate, Ordering::Relaxed);
        slot.owned_source_generations[next].store(source_generation, Ordering::Relaxed);
        slot.owned_generations[next].store(generation, Ordering::Relaxed);
        slot.source_frames.store(frames as u32, Ordering::Relaxed);
        slot.source_sample_rate.store(sample_rate, Ordering::Relaxed);
        slot.source_generation.store(source_generation, Ordering::Relaxed);
        slot.published_buffer.store(next as u32, Ordering::Release);
    }
}

pub type SidechainCaptureCallback = unsafe extern "C" fn(
    *mut c_void,
    u32,
    u32,
    u32,
    f32,
    u8,
    u32,
    u64,
    *const f32,
    *const f32,
    u32,
) -> bool;

#[no_mangle]
pub unsafe extern "C" fn hirari_sidechain_capture_track(
    state: *const c_void,
    track: u32,
    context: *mut c_void,
    callback: Option<SidechainCaptureCallback>,
) -> bool {
    let (Some(runtime), Some(callback)) = (unsafe { gate_ref(state) }, callback) else {
        return false;
    };
    for slot in &runtime.slots {
        let key = slot.key.load(Ordering::Acquire);
        let destination = (key >> 32) as u32;
        let source = slot.source_track.load(Ordering::Relaxed);
        if key == EMPTY_SIDECHAIN_KEY || (destination != track && source != track)
            || !slot.owned.load(Ordering::Relaxed)
        {
            continue;
        }
        let active = slot.published_buffer.load(Ordering::Relaxed) as usize;
        if active > 1 { continue }
        // SAFETY: Caller holds the control mutation gate throughout capture.
        let (Some(left), Some(right)) = (
            unsafe { &*slot.buffers[active * 2].get() },
            unsafe { &*slot.buffers[active * 2 + 1].get() },
        ) else {
            continue;
        };
        let frames = slot.owned_frames[active].load(Ordering::Relaxed);
        if frames == 0 || frames as usize > SIDECHAIN_BLOCK { continue }
        let plugin_index = key as u32;
        let level = f32::from_bits(slot.level_bits.load(Ordering::Relaxed));
        let tap = slot.tap_point.load(Ordering::Relaxed);
        let sample_rate = slot.owned_sample_rates[active].load(Ordering::Relaxed);
        let generation = slot.owned_source_generations[active].load(Ordering::Relaxed);
        if !unsafe {
            callback(
                context,
                destination,
                plugin_index,
                source,
                level,
                tap,
                sample_rate,
                generation,
                left.as_ptr(),
                right.as_ptr(),
                frames,
            )
        } {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidechainTapPointRust {
    PreFX,
    PostFX,
    PostFader,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(source: u32, dest: u32, plugin_idx: u32) -> SidechainLinkRust {
        SidechainLinkRust {
            source_track_id: source,
            dest_track_id: dest,
            plugin_idx,
            input_bus: 0,
            enabled: true,
            level: 1.0,
            tap_point: SidechainTapPointRust::PostFX,
        }
    }

    #[test]
    fn audit_rejects_duplicate_routes_and_invalid_levels() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(1, link(1, 2, 0)).unwrap();
        graph.active_links.insert(
            2,
            SidechainLinkRust {
                level: f32::NAN,
                ..link(3, 4, 0)
            },
        );
        assert!(!graph.audit_sidechain_manager());
    }

    #[test]
    fn audit_rejects_imported_cycles() {
        let mut graph = SidechainOrchestrator::new();
        graph.active_links.insert(1, link(1, 2, 0));
        graph.active_links.insert(2, link(2, 1, 0));
        assert!(!graph.audit_sidechain_manager());
    }

    #[test]
    fn dynamic_plugin_ports_are_enumerated_and_removed() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(20, link(3, 9, 1)).unwrap();
        graph.register_link(10, link(4, 9, 1)).unwrap();
        assert_eq!(
            graph
                .links_for_destination(9, 1)
                .iter()
                .map(|entry| entry.0)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(graph.remove_plugin_links(9, 1), 2);
        assert!(graph.links_for_destination(9, 1).is_empty());
        assert!(graph.audit_sidechain_manager());
    }

    #[test]
    fn supports_multiple_sources_and_multiple_plugin_inputs() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(30, link(3, 9, 1)).unwrap();
        graph.register_link(10, link(4, 9, 1)).unwrap();
        let mut second_input = link(5, 9, 1);
        second_input.input_bus = 1;
        graph.register_link(20, second_input).unwrap();
        assert_eq!(
            graph.sources_for_input(9, 1, 0),
            vec![(10, 4, 1.0), (30, 3, 1.0)]
        );
        assert_eq!(graph.sources_for_input(9, 1, 1), vec![(20, 5, 1.0)]);
        assert_eq!(graph.resolve_source_for(9, 1), Some(4));
        assert!(graph.audit_sidechain_manager());
    }

    #[test]
    fn rejects_an_exact_duplicate_route_but_allows_shared_destination() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(1, link(3, 9, 1)).unwrap();
        assert!(graph.register_link(2, link(4, 9, 1)).is_ok());
        assert_eq!(
            graph.register_link(3, link(3, 9, 1)),
            Err(SidechainLinkError::DuplicateRoute)
        );
    }
}

#[derive(Clone, Debug)]
pub struct SidechainLinkRust {
    pub source_track_id: u32,
    pub dest_track_id: u32,
    pub plugin_idx: u32,
    /// Zero-based side-chain input exposed by the plug-in.
    pub input_bus: u16,
    pub enabled: bool,
    pub level: f32,
    pub tap_point: SidechainTapPointRust,
}

pub struct SidechainOrchestrator {
    pub active_links: HashMap<u64, SidechainLinkRust>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SidechainLinkError {
    EmptyId,
    SelfReference,
    DuplicateId,
    DuplicateRoute,
    InvalidParameters,
    CircularReference,
}

impl Default for SidechainOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SidechainOrchestrator {
    pub fn new() -> Self {
        Self {
            active_links: HashMap::new(),
        }
    }

    /// Registers a link after checking that it can be added to the routing graph.
    pub fn register_link(
        &mut self,
        link_id: u64,
        link: SidechainLinkRust,
    ) -> Result<(), SidechainLinkError> {
        if link_id == 0 || link.source_track_id == 0 || link.dest_track_id == 0 {
            return Err(SidechainLinkError::EmptyId);
        }
        if link.source_track_id == link.dest_track_id {
            return Err(SidechainLinkError::SelfReference);
        }
        if self.active_links.contains_key(&link_id) {
            return Err(SidechainLinkError::DuplicateId);
        }
        if link.input_bus >= 256 || !link.level.is_finite() || !(0.0..=1.0).contains(&link.level) {
            return Err(SidechainLinkError::InvalidParameters);
        }
        if self.active_links.values().any(|existing| {
            existing.source_track_id == link.source_track_id
                && existing.dest_track_id == link.dest_track_id
                && existing.plugin_idx == link.plugin_idx
                && existing.input_bus == link.input_bus
        }) {
            return Err(SidechainLinkError::DuplicateRoute);
        }
        if self.would_create_cycle(link.source_track_id, link.dest_track_id) {
            return Err(SidechainLinkError::CircularReference);
        }

        self.active_links.insert(link_id, link);
        Ok(())
    }

    /// Compatibility convenience for callers that only need success/failure.
    pub fn add_sidechain_link(&mut self, link_id: u64, link: SidechainLinkRust) -> bool {
        self.register_link(link_id, link).is_ok()
    }

    /// Resolves a link ID without panicking for an unknown ID.
    pub fn resolve_link(&self, link_id: u64) -> Option<&SidechainLinkRust> {
        self.active_links.get(&link_id)
    }

    pub fn remove_link(&mut self, link_id: u64) -> bool {
        self.active_links.remove(&link_id).is_some()
    }

    /// Removes every side-chain endpoint owned by a plugin instance. This is
    /// used when a plugin is deleted or bypassed so stale dynamic ports cannot
    /// remain in the routing graph.
    pub fn remove_plugin_links(&mut self, dest_track_id: u32, plugin_idx: u32) -> usize {
        let before = self.active_links.len();
        self.active_links.retain(|_, link| {
            !(link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx)
        });
        before - self.active_links.len()
    }

    /// Returns all sources feeding a plugin in deterministic link-ID order.
    pub fn links_for_destination(
        &self,
        dest_track_id: u32,
        plugin_idx: u32,
    ) -> Vec<(u64, u32, u16, bool, f32, SidechainTapPointRust)> {
        let mut links: Vec<_> = self
            .active_links
            .iter()
            .filter(|(_, link)| {
                link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx
            })
            .map(|(id, link)| {
                (
                    *id,
                    link.source_track_id,
                    link.input_bus,
                    link.enabled,
                    link.level,
                    link.tap_point,
                )
            })
            .collect();
        links.sort_by_key(|entry| entry.0);
        links
    }

    pub fn sources_for_input(
        &self,
        dest_track_id: u32,
        plugin_idx: u32,
        input_bus: u16,
    ) -> Vec<(u64, u32, f32)> {
        let mut sources: Vec<_> = self
            .active_links
            .iter()
            .filter(|(_, link)| {
                link.enabled
                    && link.dest_track_id == dest_track_id
                    && link.plugin_idx == plugin_idx
                    && link.input_bus == input_bus
            })
            .map(|(id, link)| (*id, link.source_track_id, link.level))
            .collect();
        sources.sort_by_key(|entry| entry.0);
        sources
    }

    pub fn set_enabled(&mut self, link_id: u64, enabled: bool) -> bool {
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.enabled = enabled;
        true
    }

    pub fn update_level(&mut self, link_id: u64, level: f32) -> bool {
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return false;
        }
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.level = level;
        true
    }

    pub fn set_tap_point(&mut self, link_id: u64, tap_point: SidechainTapPointRust) -> bool {
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.tap_point = tap_point;
        true
    }

    /// Resolves the source track for a destination/plugin pair.
    pub fn resolve_source_for(&self, dest_track_id: u32, plugin_idx: u32) -> Option<u32> {
        if dest_track_id == 0 {
            return None;
        }
        self.active_links
            .iter()
            .filter(|(_, link)| {
                link.enabled && link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx
            })
            .min_by_key(|(id, _)| *id)
            .map(|(_, link)| link.source_track_id)
    }

    /// Validates the already registered graph. Registration keeps this graph valid;
    /// this method remains the orchestration entry point for existing callers.
    pub fn resolve_sidechain_links(&mut self) {
        self.active_links.retain(|_, link| {
            link.source_track_id != 0
                && link.dest_track_id != 0
                && link.source_track_id != link.dest_track_id
        });
    }

    fn would_create_cycle(&self, source: u32, destination: u32) -> bool {
        let mut current = destination;
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current) {
            if current == source {
                return true;
            }
            let Some(next) = self
                .active_links
                .values()
                .find(|link| link.source_track_id == current)
                .map(|link| link.dest_track_id)
            else {
                return false;
            };
            current = next;
        }
        false
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide routing state.
    pub fn audit_sidechain_manager(&self) -> bool {
        if self.active_links.len() > 65_536 {
            return false;
        }
        let mut routes = std::collections::HashSet::new();
        for (link_id, link) in &self.active_links {
            if *link_id == 0
                || link.source_track_id == 0
                || link.dest_track_id == 0
                || link.source_track_id == link.dest_track_id
                || !link.level.is_finite()
                || !(0.0..=1.0).contains(&link.level)
                || link.input_bus >= 256
                || !matches!(
                    link.tap_point,
                    SidechainTapPointRust::PreFX
                        | SidechainTapPointRust::PostFX
                        | SidechainTapPointRust::PostFader
                )
            {
                return false;
            }
            if !routes.insert((
                link.source_track_id,
                link.dest_track_id,
                link.plugin_idx,
                link.input_bus,
            )) {
                return false;
            }
        }

        // Validate the complete graph, not only the edge most recently added.
        // This catches imported/legacy snapshots that bypass register_link().
        for link in self.active_links.values() {
            if self.would_create_cycle_from_snapshot(link.source_track_id, link.dest_track_id) {
                return false;
            }
        }
        true
    }

    fn would_create_cycle_from_snapshot(&self, source: u32, destination: u32) -> bool {
        let mut current = destination;
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current) {
            if current == source {
                return true;
            }
            let Some(next) = self
                .active_links
                .values()
                .filter(|link| link.source_track_id == current)
                .map(|link| link.dest_track_id)
                .next()
            else {
                return false;
            };
            current = next;
        }
        true
    }
}
