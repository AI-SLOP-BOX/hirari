//! Allocation-free sample operations for the engine's audio blocks.

use std::alloc::{alloc, dealloc, Layout};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

thread_local! {
    static AUDIO_BUFFER_REALTIME_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static AUDIO_BUFFER_RESIZE_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static AUDIO_BUFFER_LAST_REQUESTED_CAPACITY: AtomicUsize = AtomicUsize::new(0);
static AUDIO_BUFFER_LAST_CAPACITY: AtomicUsize = AtomicUsize::new(0);

struct BuiltinGainState {
    gain: AtomicU32,
    prepared: AtomicBool,
}

#[no_mangle]
pub extern "C" fn hirari_builtin_gain_create() -> *mut c_void {
    Box::into_raw(Box::new(BuiltinGainState {
        gain: AtomicU32::new(1.0f32.to_bits()),
        prepared: AtomicBool::new(false),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<BuiltinGainState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_prepare(
    state: *const c_void,
    sample_rate: f64,
    block_size: u32,
) {
    if let Some(state) = unsafe { state.cast::<BuiltinGainState>().as_ref() } {
        state.prepared.store(
            sample_rate.is_finite() && sample_rate > 0.0 && block_size > 0,
            Ordering::Release,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if id != 0 || !value.is_finite() {
        return;
    }
    if let Some(state) = unsafe { state.cast::<BuiltinGainState>().as_ref() } {
        state
            .gain
            .store(value.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_get_parameter(state: *const c_void, id: u32) -> f32 {
    if id != 0 {
        return 0.0;
    }
    unsafe { state.cast::<BuiltinGainState>().as_ref() }.map_or(0.0, |state| {
        f32::from_bits(state.gain.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_process_state(
    state: *const c_void,
    channels: *const *mut f32,
    channel_count: u32,
    sample_count: u32,
) -> u32 {
    let Some(state) = (unsafe { state.cast::<BuiltinGainState>().as_ref() }) else {
        return 0;
    };
    if !state.prepared.load(Ordering::Acquire) {
        return 0;
    }
    unsafe {
        hirari_builtin_gain_process(
            channels,
            channel_count,
            sample_count,
            f32::from_bits(state.gain.load(Ordering::Relaxed)),
        )
    }
}

const AUDIO_BUFFER_INLINE_CHANNELS: usize = 16;

struct AlignedAudioStorage {
    samples: *mut f32,
    capacity: usize,
    inline_channel_pointers: [*mut f32; AUDIO_BUFFER_INLINE_CHANNELS],
    channel_pointers: Vec<*mut f32>,
    pointer_count: usize,
    channels: u32,
    frame_count: u32,
    external: bool,
    external_data: *mut *mut f32,
}

fn new_aligned_audio_storage() -> AlignedAudioStorage {
    AlignedAudioStorage {
        samples: std::ptr::null_mut(),
        capacity: 0,
        inline_channel_pointers: [std::ptr::null_mut(); AUDIO_BUFFER_INLINE_CHANNELS],
        channel_pointers: Vec::new(),
        pointer_count: 0,
        channels: 0,
        frame_count: 0,
        external: false,
        external_data: std::ptr::null_mut(),
    }
}

impl Drop for AlignedAudioStorage {
    fn drop(&mut self) {
        self.release();
    }
}

impl AlignedAudioStorage {
    fn channel_pointers(&self) -> *mut *mut f32 {
        if self.pointer_count <= AUDIO_BUFFER_INLINE_CHANNELS {
            self.inline_channel_pointers.as_ptr() as *mut *mut f32
        } else {
            self.channel_pointers.as_ptr() as *mut *mut f32
        }
    }

    fn channel_pointers_mut(&mut self) -> &mut [*mut f32] {
        if self.pointer_count <= AUDIO_BUFFER_INLINE_CHANNELS {
            &mut self.inline_channel_pointers[..self.pointer_count]
        } else {
            &mut self.channel_pointers[..self.pointer_count]
        }
    }

    fn release(&mut self) {
        if !self.samples.is_null() {
            if let Some(layout) = audio_storage_layout(self.capacity) {
                unsafe { dealloc(self.samples.cast(), layout) };
            }
        }
        self.samples = std::ptr::null_mut();
        self.capacity = 0;
    }
}

fn audio_storage_layout(capacity: usize) -> Option<Layout> {
    capacity
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|bytes| Layout::from_size_align(bytes, 4096).ok())
}

/// Copies decoded interleaved PCM into the planar channel buffers owned by the
/// native AudioBuffer adapter. The caller must provide distinct writable
/// channel planes, as returned by AudioBuffer::getArrayOfWritePointers().
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_deinterleave_interleaved(
    source: *const f32,
    source_sample_count: usize,
    channel_pointers: *const *mut f32,
    channel_count: u32,
    frames: u32,
) -> bool {
    let expected_samples = match (channel_count as usize).checked_mul(frames as usize) {
        Some(count) => count,
        None => return false,
    };
    if source.is_null()
        || channel_pointers.is_null()
        || channel_count == 0
        || frames == 0
        || source_sample_count != expected_samples
    {
        return false;
    }

    // SAFETY: the caller supplies arrays sized by the validated counts.
    let source = unsafe { std::slice::from_raw_parts(source, source_sample_count) };
    let channel_pointers =
        unsafe { std::slice::from_raw_parts(channel_pointers, channel_count as usize) };
    if channel_pointers.iter().any(|channel| channel.is_null()) {
        return false;
    }
    let Some(channel_bytes) = (frames as usize).checked_mul(std::mem::size_of::<f32>()) else {
        return false;
    };
    for left_index in 0..channel_pointers.len() {
        let left_start = channel_pointers[left_index] as usize;
        let Some(left_end) = left_start.checked_add(channel_bytes) else {
            return false;
        };
        for right_pointer in &channel_pointers[left_index + 1..] {
            let right_start = *right_pointer as usize;
            let Some(right_end) = right_start.checked_add(channel_bytes) else {
                return false;
            };
            if left_start < right_end && right_start < left_end {
                return false;
            }
        }
    }
    for (channel_index, channel_pointer) in channel_pointers.iter().enumerate() {
        // SAFETY: the AudioBuffer adapter provides `frames` writable samples
        // for each distinct channel pointer.
        let output = unsafe { std::slice::from_raw_parts_mut(*channel_pointer, frames as usize) };
        for frame in 0..frames as usize {
            output[frame] = source[frame * channel_count as usize + channel_index];
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn hirari_audio_buffer_storage_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(new_aligned_audio_storage())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_destroy(storage: *mut std::ffi::c_void) {
    if !storage.is_null() {
        drop(Box::from_raw(storage.cast::<AlignedAudioStorage>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_reserve(
    storage: *mut std::ffi::c_void,
    capacity: usize,
) -> bool {
    let Some(storage) = storage.cast::<AlignedAudioStorage>().as_mut() else {
        return false;
    };
    if capacity <= storage.capacity {
        return true;
    }
    let Some(layout) = audio_storage_layout(capacity) else {
        return false;
    };
    let samples = alloc(layout).cast::<f32>();
    if samples.is_null() {
        return false;
    }
    if !storage.samples.is_null() {
        if let Some(old_layout) = audio_storage_layout(storage.capacity) {
            dealloc(storage.samples.cast(), old_layout);
        }
    }
    storage.samples = samples;
    storage.capacity = capacity;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_release(storage: *mut std::ffi::c_void) {
    if let Some(storage) = storage.cast::<AlignedAudioStorage>().as_mut() {
        storage.release();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_data(
    storage: *const std::ffi::c_void,
) -> *mut f32 {
    storage
        .cast::<AlignedAudioStorage>()
        .as_ref()
        .map_or(std::ptr::null_mut(), |storage| storage.samples)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_capacity(
    storage: *const std::ffi::c_void,
) -> usize {
    storage
        .cast::<AlignedAudioStorage>()
        .as_ref()
        .map_or(0, |storage| storage.capacity)
}

/// Reserves the planar channel pointer table used by the C++ AudioBuffer
/// compatibility facade. Allocation is only requested from its control path.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_prepare_channels(
    storage: *mut c_void,
    channel_count: usize,
) -> bool {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_mut() }) else {
        return false;
    };
    if storage.pointer_count == channel_count {
        return true;
    }
    if channel_count <= AUDIO_BUFFER_INLINE_CHANNELS {
        if storage.pointer_count <= AUDIO_BUFFER_INLINE_CHANNELS {
            storage.inline_channel_pointers.fill(std::ptr::null_mut());
            storage.pointer_count = AUDIO_BUFFER_INLINE_CHANNELS;
        }
        return true;
    }
    if storage.pointer_count >= channel_count {
        return true;
    }
    let mut pointers = Vec::new();
    if pointers.try_reserve_exact(channel_count).is_err() {
        return false;
    }
    pointers.resize(channel_count, std::ptr::null_mut());
    storage.channel_pointers = pointers;
    storage.pointer_count = channel_count;
    true
}

/// Refreshes the planar channel view from Rust-owned sample storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_update_channel_pointers(
    storage: *mut c_void,
    data: *mut f32,
    channels: u32,
    samples: u32,
) -> bool {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_mut() }) else {
        return false;
    };
    if storage.pointer_count < channels as usize {
        return false;
    }
    let required = match (channels as usize).checked_mul(samples as usize) {
        Some(required) => required,
        None => return false,
    };
    if required > storage.capacity {
        return false;
    }
    let pointers = storage.channel_pointers_mut();
    for (channel, pointer) in pointers.iter_mut().take(channels as usize).enumerate() {
        let Some(offset) = channel.checked_mul(samples as usize) else {
            return false;
        };
        *pointer = unsafe { data.add(offset) };
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_channel_pointers(
    storage: *const c_void,
) -> *mut *mut f32 {
    unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }
        .map_or(std::ptr::null_mut(), |storage| storage.channel_pointers())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_channel_count(
    storage: *const c_void,
) -> usize {
    unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }
        .map_or(0, |storage| storage.pointer_count)
}

/// Applies the AudioBuffer growth plan on the Rust side. Status is 0 for
/// success, 1 for a rejected realtime sample resize, 5 for a rejected realtime
/// channel-table resize, 2 for an invalid size, 3 for sample allocation
/// failure, and 4 for pointer allocation failure.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_reserve_buffer(
    storage_handle: *mut c_void,
    channels: u32,
    samples: u32,
) -> u8 {
    let Some(storage) = (unsafe { storage_handle.cast::<AlignedAudioStorage>().as_mut() }) else {
        return 3;
    };
    if samples > u32::MAX - 15 {
        return 2;
    }
    let padded_samples = (samples + 15) & !15;
    let Some(required) = (channels as usize).checked_mul(padded_samples as usize) else {
        return 2;
    };
    if required > usize::MAX / std::mem::size_of::<f32>() {
        return 2;
    }
    let needs_data = required > storage.capacity || (storage.external && required != 0);
    let required_pointers = channels.max(storage.channels) as usize;
    let target_pointer_count = required_pointers.max(AUDIO_BUFFER_INLINE_CHANNELS);
    let needs_pointer_table = storage.pointer_count < target_pointer_count;
    let needs_pointer_allocation = needs_pointer_table
        && required_pointers > AUDIO_BUFFER_INLINE_CHANNELS;
    if !needs_data && !needs_pointer_table {
        return 0;
    }
    let realtime = AUDIO_BUFFER_REALTIME_GUARD.with(std::cell::Cell::get);
    if realtime && (needs_data || needs_pointer_allocation) {
        AUDIO_BUFFER_RESIZE_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        AUDIO_BUFFER_LAST_REQUESTED_CAPACITY.store(required, Ordering::Relaxed);
        AUDIO_BUFFER_LAST_CAPACITY.store(storage.capacity, Ordering::Relaxed);
        // A realtime growth is reported where it is rejected: the caller holds
        // only this status code, so the notification has no C++ wrapper left.
        let text: &[u8] = if needs_data {
            b"AUDIO_BUFFER_RESIZE_ATTEMPTED"
        } else {
            b"AUDIO_BUFFER_POINTER_RESIZE_ATTEMPTED"
        };
        unsafe { crate::status_queue::hirari_status_queue_push_default(3, text.as_ptr(), text.len()) };
        return if needs_data { 1 } else { 5 };
    }
    if needs_data
        && !unsafe { hirari_audio_buffer_storage_reserve(storage_handle, required) }
    {
        return 3;
    }
    if needs_pointer_table
        && !unsafe { hirari_audio_buffer_storage_prepare_channels(storage_handle, target_pointer_count) }
    {
        return 4;
    }
    0
}

/// Commits a new owned buffer size after a successful reserve. Ownership of
/// the dimension fields and the channel view lives here, not in C++.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_resize(
    storage_handle: *mut c_void,
    channels: u32,
    samples: u32,
) -> bool {
    let Some(storage) = (unsafe { storage_handle.cast::<AlignedAudioStorage>().as_mut() }) else {
        return false;
    };
    if storage.pointer_count < channels as usize {
        return false;
    }
    let Some(required) = (channels as usize).checked_mul(samples as usize) else {
        return false;
    };
    if required > storage.capacity {
        return false;
    }
    storage.external = false;
    storage.external_data = std::ptr::null_mut();
    storage.channels = channels;
    storage.frame_count = samples;
    if !unsafe {
        hirari_audio_buffer_storage_update_channel_pointers(
            storage_handle, storage.samples, channels, samples,
        )
    } {
        return false;
    }
    unsafe { hirari_audio_buffer_storage_clear(storage_handle, 0, samples) };
    true
}

/// Switches a native AudioBuffer between owned and externally wrapped planes.
/// Invalid wraps release the current owned block and reset the view, matching
/// the compatibility API's failure behavior.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_wrap_channels(
    storage_handle: *mut c_void,
    channels: *mut *mut f32,
    channel_count: u32,
    sample_count: u32,
) -> bool {
    let Some(storage) = (unsafe { storage_handle.cast::<AlignedAudioStorage>().as_mut() }) else {
        return false;
    };
    storage.release();
    storage.channels = 0;
    storage.frame_count = 0;
    storage.external = false;
    storage.external_data = std::ptr::null_mut();
    if channels.is_null() || channel_count == 0 || sample_count == 0 {
        return false;
    }
    if (0..channel_count as usize).any(|index| unsafe { (*channels.add(index)).is_null() }) {
        return false;
    }
    storage.channels = channel_count;
    storage.frame_count = sample_count;
    storage.external = true;
    storage.external_data = channels;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_channels(
    storage: *const c_void,
) -> u32 {
    unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }.map_or(0, |storage| storage.channels)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_samples(
    storage: *const c_void,
) -> u32 {
    unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }
        .map_or(0, |storage| storage.frame_count)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_array_read(
    storage: *const c_void,
) -> *const *mut f32 {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }) else {
        return std::ptr::null();
    };
    if storage.external {
        storage.external_data as *const *mut f32
    } else {
        storage.channel_pointers() as *const *mut f32
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_array_write(
    storage: *const c_void,
) -> *mut *mut f32 {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    if storage.external {
        storage.external_data
    } else {
        storage.channel_pointers()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_write_pointer(
    storage: *const c_void,
    channel: u32,
) -> *mut f32 {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    if channel >= storage.channels {
        return std::ptr::null_mut();
    }
    if storage.external {
        return unsafe { *storage.external_data.add(channel as usize) };
    }
    unsafe { storage.samples.add(channel as usize * storage.frame_count as usize) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_read_pointer(
    storage: *const c_void,
    channel: u32,
    offset: u32,
) -> *const f32 {
    let pointer = unsafe { hirari_audio_buffer_storage_write_pointer(storage, channel) };
    if pointer.is_null() {
        return std::ptr::null();
    }
    let samples = unsafe { hirari_audio_buffer_storage_samples(storage) };
    unsafe { pointer.add(offset.min(samples) as usize) as *const f32 }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_write_pointer_at(
    storage: *const c_void,
    channel: u32,
    offset: u32,
) -> *mut f32 {
    let pointer = unsafe { hirari_audio_buffer_storage_write_pointer(storage, channel) };
    if pointer.is_null() {
        return std::ptr::null_mut();
    }
    let samples = unsafe { hirari_audio_buffer_storage_samples(storage) };
    unsafe { pointer.add(offset.min(samples) as usize) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_clear(
    storage_handle: *const c_void,
    offset: u32,
    samples: u32,
) {
    let Some(storage) = (unsafe { storage_handle.cast::<AlignedAudioStorage>().as_ref() }) else {
        return;
    };
    if offset >= storage.frame_count || storage.channels == 0 {
        return;
    }
    let count = samples.min(storage.frame_count - offset);
    let pointers = if storage.external {
        storage.external_data
    } else {
        storage.channel_pointers()
    };
    unsafe {
        hirari_audio_buffer_clear(pointers, storage.channels, offset, count);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_sanitize(
    storage: *const c_void,
) -> u32 {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }) else {
        return 0;
    };
    let pointers = if storage.external {
        storage.external_data
    } else {
        storage.channel_pointers()
    };
    unsafe {
        hirari_audio_buffer_sanitize_non_finite(pointers, storage.channels, storage.frame_count)
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_apply_gain(
    storage: *const c_void,
    gain: f32,
) {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }) else {
        return;
    };
    if !gain.is_finite() || gain == 1.0 {
        return;
    }
    let pointers = if storage.external {
        storage.external_data
    } else {
        storage.channel_pointers()
    };
    unsafe {
        hirari_audio_buffer_apply_gain(pointers, storage.channels, storage.frame_count, gain);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_add_from(
    storage_handle: *const c_void,
    source_handle: *const c_void,
    sample_count: u32,
) {
    let Some(storage) = (unsafe { storage_handle.cast::<AlignedAudioStorage>().as_ref() }) else {
        return;
    };
    let Some(source) = (unsafe { source_handle.cast::<AlignedAudioStorage>().as_ref() }) else {
        return;
    };
    let channels = storage.channels.min(source.channels);
    let frames = sample_count.min(storage.frame_count.min(source.frame_count));
    if channels == 0 {
        return;
    }
    let destinations = if storage.external {
        storage.external_data
    } else {
        storage.channel_pointers()
    };
    let sources = if source.external {
        source.external_data as *const *const f32
    } else {
        source.channel_pointers() as *const *const f32
    };
    unsafe {
        hirari_audio_buffer_add_channels(destinations, sources, channels, frames);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_storage_add_channels_from(
    storage: *const c_void,
    source_left: *const f32,
    source_right: *const f32,
    sample_count: u32,
) {
    let Some(storage) = (unsafe { storage.cast::<AlignedAudioStorage>().as_ref() }) else {
        return;
    };
    if source_left.is_null() || (storage.channels > 1 && source_right.is_null()) {
        return;
    }
    if storage.channels < 1 {
        return;
    }
    let frames = sample_count.min(storage.frame_count);
    let handle = storage as *const AlignedAudioStorage as *const c_void;
    let left = unsafe { hirari_audio_buffer_storage_write_pointer(handle, 0) };
    let right = if storage.channels > 1 {
        unsafe { hirari_audio_buffer_storage_write_pointer(handle, 1) }
    } else {
        std::ptr::null_mut()
    };
    let mut destinations: [*mut f32; 2] = [left, right];
    let mut sources: [*const f32; 2] = [source_left, source_right];
    unsafe {
        hirari_audio_buffer_add_channels(
            destinations.as_mut_ptr(),
            sources.as_mut_ptr(),
            if right.is_null() { 1 } else { 2 },
            frames,
        );
    }
}

#[no_mangle]
pub extern "C" fn hirari_audio_buffer_set_realtime_guard(is_realtime: bool) {
    AUDIO_BUFFER_REALTIME_GUARD.with(|guard| guard.set(is_realtime));
}

#[no_mangle]
pub extern "C" fn hirari_audio_buffer_realtime_resize_attempts() -> u64 {
    AUDIO_BUFFER_RESIZE_ATTEMPTS.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn hirari_audio_buffer_last_realtime_requested_capacity() -> usize {
    AUDIO_BUFFER_LAST_REQUESTED_CAPACITY.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn hirari_audio_buffer_last_realtime_capacity() -> usize {
    AUDIO_BUFFER_LAST_CAPACITY.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn hirari_audio_buffer_reset_realtime_resize_stats() {
    AUDIO_BUFFER_RESIZE_ATTEMPTS.store(0, Ordering::Relaxed);
    AUDIO_BUFFER_LAST_REQUESTED_CAPACITY.store(0, Ordering::Relaxed);
    AUDIO_BUFFER_LAST_CAPACITY.store(0, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_clear(
    channels: *const *mut f32,
    channel_count: u32,
    offset: u32,
    sample_count: u32,
) {
    if channels.is_null() || sample_count == 0 {
        return;
    }
    for channel in 0..channel_count as usize {
        let samples = *channels.add(channel);
        if samples.is_null() {
            continue;
        }
        for index in offset as usize..offset as usize + sample_count as usize {
            samples.add(index).write(0.0);
        }
    }
}

/// Clears the requested output block, then copies the available frozen audio.
/// This keeps the frozen-track callback path allocation-free across the FFI.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_render_frozen(
    destinations: *const *mut f32,
    destination_channels: u32,
    destination_samples: u32,
    sources: *const *const f32,
    source_channels: u32,
    source_samples: u32,
    total_samples: u64,
    playhead: u64,
    frame_count: u32,
) -> u32 {
    if destinations.is_null() {
        return 0;
    }
    let clear_count = frame_count.min(destination_samples);
    hirari_audio_buffer_clear(destinations, destination_channels, 0, clear_count);
    if sources.is_null() || playhead >= total_samples || playhead >= u64::from(source_samples) {
        return 0;
    }
    let frames = u64::from(frame_count.min(destination_samples))
        .min(total_samples - playhead)
        .min(u64::from(source_samples) - playhead) as usize;
    if frames == 0 {
        return 0;
    }
    let channels = destination_channels.min(source_channels) as usize;
    for channel in 0..channels {
        let destination = *destinations.add(channel);
        let source = *sources.add(channel);
        if destination.is_null() || source.is_null() {
            continue;
        }
        std::ptr::copy_nonoverlapping(source.add(playhead as usize), destination, frames);
    }
    frames as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_sanitize_non_finite(
    channels: *const *mut f32,
    channel_count: u32,
    sample_count: u32,
) -> u32 {
    if channels.is_null() {
        return 0;
    }
    let mut replaced = 0_u32;
    for channel in 0..channel_count as usize {
        let samples = *channels.add(channel);
        if samples.is_null() {
            continue;
        }
        for index in 0..sample_count as usize {
            let sample = samples.add(index);
            if !(*sample).is_finite() {
                sample.write(0.0);
                replaced = replaced.wrapping_add(1);
            }
        }
    }
    replaced
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_add_channels(
    destinations: *const *mut f32,
    sources: *const *const f32,
    channel_count: u32,
    sample_count: u32,
) {
    if destinations.is_null() || sources.is_null() {
        return;
    }
    for channel in 0..channel_count as usize {
        let destination = *destinations.add(channel);
        let source = *sources.add(channel);
        if destination.is_null() || source.is_null() {
            continue;
        }
        for index in 0..sample_count as usize {
            let destination_sample = destination.add(index);
            destination_sample.write(*destination_sample + *source.add(index));
        }
    }
}

/// Applies the gain correction used after a unity-gain route accumulation.
/// Non-finite sources are ignored in the correction pass, matching the host's
/// audio-route policy while leaving the initial buffer add unchanged.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_route_gain_correction(
    destination_left: *mut f32,
    destination_right: *mut f32,
    source_left: *const f32,
    source_right: *const f32,
    frames: u32,
    gain: f32,
) {
    if destination_left.is_null()
        || destination_right.is_null()
        || source_left.is_null()
        || source_right.is_null()
        || !gain.is_finite()
    {
        return;
    }
    let (destination_left, destination_right, source_left, source_right) = unsafe {
        (
            std::slice::from_raw_parts_mut(destination_left, frames as usize),
            std::slice::from_raw_parts_mut(destination_right, frames as usize),
            std::slice::from_raw_parts(source_left, frames as usize),
            std::slice::from_raw_parts(source_right, frames as usize),
        )
    };
    let correction = gain - 1.0;
    for (((destination_left, destination_right), source_left), source_right) in destination_left
        .iter_mut()
        .zip(destination_right)
        .zip(source_left)
        .zip(source_right)
    {
        if source_left.is_finite() {
            *destination_left += *source_left * correction;
        }
        if source_right.is_finite() {
            *destination_right += *source_right * correction;
        }
    }
}

/// Adds finite stereo source samples to a destination block, replacing
/// malformed source values with silence.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_add_sanitized_stereo(
    destination_left: *mut f32,
    destination_right: *mut f32,
    source_left: *const f32,
    source_right: *const f32,
    frames: u32,
) {
    if destination_left.is_null()
        || destination_right.is_null()
        || source_left.is_null()
        || source_right.is_null()
    {
        return;
    }
    let (destination_left, destination_right, source_left, source_right) = unsafe {
        (
            std::slice::from_raw_parts_mut(destination_left, frames as usize),
            std::slice::from_raw_parts_mut(destination_right, frames as usize),
            std::slice::from_raw_parts(source_left, frames as usize),
            std::slice::from_raw_parts(source_right, frames as usize),
        )
    };
    for (((destination_left, destination_right), source_left), source_right) in destination_left
        .iter_mut()
        .zip(destination_right)
        .zip(source_left)
        .zip(source_right)
    {
        *destination_left += if source_left.is_finite() {
            *source_left
        } else {
            0.0
        };
        *destination_right += if source_right.is_finite() {
            *source_right
        } else {
            0.0
        };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_apply_gain(
    channels: *const *mut f32,
    channel_count: u32,
    sample_count: u32,
    gain: f32,
) {
    if channels.is_null() || !gain.is_finite() {
        return;
    }
    for channel in 0..channel_count as usize {
        let samples = *channels.add(channel);
        if samples.is_null() {
            continue;
        }
        for index in 0..sample_count as usize {
            let sample = samples.add(index);
            sample.write(*sample * gain);
        }
    }
}

/// Realtime DSP used by the registered Built-in Gain processor. Unlike the
/// general AudioBuffer gain primitive, this preserves the processor's
/// non-finite output containment contract.
#[no_mangle]
pub unsafe extern "C" fn hirari_builtin_gain_process(
    channels: *const *mut f32,
    channel_count: u32,
    sample_count: u32,
    gain: f32,
) -> u32 {
    if channels.is_null() || !gain.is_finite() {
        return 0;
    }
    let mut sanitized = 0_u32;
    for channel in 0..channel_count as usize {
        let samples = unsafe { *channels.add(channel) };
        if samples.is_null() {
            continue;
        }
        for index in 0..sample_count as usize {
            let sample = unsafe { samples.add(index) };
            let output = unsafe { *sample } * gain;
            if output.is_finite() {
                unsafe { sample.write(output) };
            } else {
                unsafe { sample.write(0.0) };
                sanitized = sanitized.saturating_add(1);
            }
        }
    }
    sanitized
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_copy(
    destination_left: *mut f32,
    destination_right: *mut f32,
    source_left: *const f32,
    source_right: *const f32,
    sample_count: u32,
) -> bool {
    if sample_count == 0 {
        return !source_left.is_null() && !source_right.is_null();
    }
    if destination_left.is_null()
        || destination_right.is_null()
        || source_left.is_null()
        || source_right.is_null()
    {
        return false;
    }
    std::ptr::copy(source_left, destination_left, sample_count as usize);
    std::ptr::copy(source_right, destination_right, sample_count as usize);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_buffer_magnitude(
    samples: *const f32,
    sample_count: u32,
) -> f32 {
    if samples.is_null() {
        return 0.0;
    }
    let mut maximum = 0.0_f32;
    for index in 0..sample_count as usize {
        let magnitude = (*samples.add(index)).abs();
        if magnitude > maximum {
            maximum = magnitude;
        }
    }
    maximum
}

#[cfg(test)]
mod frozen_render_tests {
    use super::hirari_audio_buffer_render_frozen;

    #[test]
    fn copies_bounded_audio_and_leaves_missing_channels_silent() {
        let source_left = [1.0_f32, 2.0, 3.0, 4.0];
        let source_right = [-1.0_f32, -2.0, -3.0, -4.0];
        let sources = [source_left.as_ptr(), source_right.as_ptr()];
        let mut left = [9.0_f32; 6];
        let mut right = [9.0_f32; 6];
        let mut extra = [9.0_f32; 6];
        let mut destinations = [left.as_mut_ptr(), right.as_mut_ptr(), extra.as_mut_ptr()];
        let copied = unsafe {
            hirari_audio_buffer_render_frozen(
                destinations.as_mut_ptr(),
                3,
                6,
                sources.as_ptr(),
                2,
                4,
                3,
                1,
                4,
            )
        };
        assert_eq!(copied, 2);
        assert_eq!(left, [2.0, 3.0, 0.0, 0.0, 9.0, 9.0]);
        assert_eq!(right, [-2.0, -3.0, 0.0, 0.0, 9.0, 9.0]);
        assert_eq!(extra, [0.0, 0.0, 0.0, 0.0, 9.0, 9.0]);
    }

    #[test]
    fn clears_requested_output_when_source_is_missing_or_exhausted() {
        let mut left = [7.0_f32; 4];
        let mut destinations = [left.as_mut_ptr()];
        let copied = unsafe {
            hirari_audio_buffer_render_frozen(
                destinations.as_mut_ptr(),
                1,
                4,
                std::ptr::null(),
                0,
                0,
                8,
                8,
                4,
            )
        };
        assert_eq!(copied, 0);
        assert_eq!(left, [0.0; 4]);
    }

    #[test]
    fn bounds_copy_by_source_capacity_even_when_timeline_is_longer() {
        let source = [3.0_f32, 4.0];
        let sources = [source.as_ptr()];
        let mut output = [8.0_f32; 4];
        let mut destinations = [output.as_mut_ptr()];
        let copied = unsafe {
            hirari_audio_buffer_render_frozen(
                destinations.as_mut_ptr(),
                1,
                4,
                sources.as_ptr(),
                1,
                2,
                20,
                1,
                4,
            )
        };
        assert_eq!(copied, 1);
        assert_eq!(output, [4.0, 0.0, 0.0, 0.0]);
    }
}

#[cfg(test)]
mod builtin_gain_tests {
    use super::hirari_builtin_gain_process;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_builtin_gain_process_reference(
            channels: *const *mut f32,
            channel_count: u32,
            sample_count: u32,
            gain: f32,
        ) -> u32;
    }

    #[test]
    fn registered_gain_matches_reference_and_contains_non_finite_output() {
        let mut left = [0.25_f32, -0.5, f32::INFINITY, f32::NAN];
        let mut right = [2.0_f32, -3.0, f32::NEG_INFINITY, 0.125];
        let mut expected_left = left;
        let mut expected_right = right;
        let gain = 1.75_f32;
        let mut reference_sanitized = 0;
        for channel in [&mut expected_left, &mut expected_right] {
            for sample in channel {
                let output = *sample * gain;
                if output.is_finite() {
                    *sample = output;
                } else {
                    *sample = 0.0;
                    reference_sanitized += 1;
                }
            }
        }

        let channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let sanitized =
            unsafe { hirari_builtin_gain_process(channels.as_ptr(), 2, left.len() as u32, gain) };
        assert_eq!(sanitized, reference_sanitized);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn registered_gain_matches_legacy_cpp_reference() {
        let original_left = [0.25_f32, -0.5, f32::INFINITY, f32::NAN, 0.0];
        let original_right = [2.0_f32, -3.0, f32::NEG_INFINITY, 0.125, -0.0];
        let mut rust_left = original_left;
        let mut rust_right = original_right;
        let mut cpp_left = original_left;
        let mut cpp_right = original_right;
        let gain = 1.75_f32;
        let rust_channels = [rust_left.as_mut_ptr(), rust_right.as_mut_ptr()];
        let cpp_channels = [cpp_left.as_mut_ptr(), cpp_right.as_mut_ptr()];

        let rust_sanitized = unsafe {
            hirari_builtin_gain_process(rust_channels.as_ptr(), 2, original_left.len() as u32, gain)
        };
        let cpp_sanitized = unsafe {
            hirari_builtin_gain_process_reference(
                cpp_channels.as_ptr(),
                2,
                original_left.len() as u32,
                gain,
            )
        };

        assert_eq!(rust_sanitized, cpp_sanitized);
        assert_eq!(rust_left, cpp_left);
        assert_eq!(rust_right, cpp_right);
    }
}

/// The whole owned/wrapped buffer contract: dimensions, channel views, bounds,
/// non-finite sanitising and the mixing entry points the native audio thread
/// uses. These drive the same C ABI the engine's typed handle forwards to.
#[cfg(test)]
mod storage_contract_tests {
    use std::ffi::c_void;
    use std::ptr;

    use super::{
        hirari_audio_buffer_storage_add_channels_from, hirari_audio_buffer_storage_add_from,
        hirari_audio_buffer_storage_apply_gain, hirari_audio_buffer_storage_array_read,
        hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_channels,
        hirari_audio_buffer_storage_clear, hirari_audio_buffer_storage_create,
        hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_read_pointer,
        hirari_audio_buffer_storage_reserve_buffer, hirari_audio_buffer_storage_resize,
        hirari_audio_buffer_storage_samples, hirari_audio_buffer_storage_sanitize,
        hirari_audio_buffer_storage_wrap_channels, hirari_audio_buffer_storage_write_pointer,
    };

    struct Storage(*mut c_void);

    impl Storage {
        fn empty() -> Self {
            Self(hirari_audio_buffer_storage_create())
        }

        fn owned(channels: u32, samples: u32) -> Self {
            let storage = Self::empty();
            assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(storage.0, channels, samples) }, 0);
            assert!(unsafe { hirari_audio_buffer_storage_resize(storage.0, channels, samples) });
            storage
        }

        fn write(&self, channel: u32) -> *mut f32 {
            unsafe { hirari_audio_buffer_storage_write_pointer(self.0, channel) }
        }

        fn read(&self, channel: u32) -> *const f32 {
            unsafe { hirari_audio_buffer_storage_read_pointer(self.0, channel, 0) }
        }

        fn fill(&self, channel: u32, values: &[f32]) {
            let pointer = self.write(channel);
            assert!(!pointer.is_null());
            for (index, value) in values.iter().enumerate() {
                unsafe { *pointer.add(index) = *value };
            }
        }

        fn sample(&self, channel: u32, index: u32) -> f32 {
            unsafe { *self.read(channel).add(index as usize) }
        }

        fn dimensions(&self) -> (u32, u32) {
            unsafe {
                (
                    hirari_audio_buffer_storage_channels(self.0),
                    hirari_audio_buffer_storage_samples(self.0),
                )
            }
        }
    }

    impl Drop for Storage {
        fn drop(&mut self) {
            unsafe { hirari_audio_buffer_storage_destroy(self.0) };
        }
    }

    #[test]
    fn mixed_planes_ignore_incomplete_sources_and_keep_finite_audio() {
        let stereo = Storage::owned(2, 4);
        stereo.fill(0, &[0.25, 0.0, 0.0, 0.0]);
        stereo.fill(1, &[-0.25, 0.0, 0.0, 0.0]);

        // Invalid raw bridge input must be rejected without touching audio.
        unsafe {
            hirari_audio_buffer_storage_add_channels_from(stereo.0, ptr::null(), ptr::null(), 4);
        }
        assert_eq!(stereo.sample(0, 0), 0.25);
        assert_eq!(stereo.sample(1, 0), -0.25);

        // A stereo destination needs both planes; a missing right is refused.
        let left = [0.5_f32, 0.0, 0.0, 0.0];
        let right = [-0.5_f32, 0.0, 0.0, 0.0];
        unsafe {
            hirari_audio_buffer_storage_add_channels_from(stereo.0, left.as_ptr(), ptr::null(), 4);
        }
        assert_eq!(stereo.sample(0, 0), 0.25);
        assert_eq!(stereo.sample(1, 0), -0.25);

        unsafe {
            hirari_audio_buffer_storage_add_channels_from(
                stereo.0,
                left.as_ptr(),
                right.as_ptr(),
                4,
            );
        }
        assert_eq!(stereo.sample(0, 0), 0.75);
        assert_eq!(stereo.sample(1, 0), -0.75);

        // A non-finite gain is a no-op rather than a way to poison the block.
        unsafe {
            hirari_audio_buffer_storage_apply_gain(stereo.0, f32::NAN);
            hirari_audio_buffer_storage_apply_gain(stereo.0, f32::INFINITY);
        }
        assert!(stereo.sample(0, 0).is_finite());
        assert!(stereo.sample(1, 0).is_finite());
    }

    #[test]
    fn dimensions_and_channel_views_are_bounded() {
        let stereo = Storage::owned(2, 4);
        assert_eq!(stereo.dimensions(), (2, 4));
        assert!(stereo.write(2).is_null());
        assert!(stereo.read(2).is_null());

        // Read offsets clamp to the frame count instead of running past a plane.
        let offset = unsafe { hirari_audio_buffer_storage_read_pointer(stereo.0, 0, 99) };
        assert_eq!(offset, unsafe { stereo.read(0).add(4) });

        let array_write = unsafe { hirari_audio_buffer_storage_array_write(stereo.0) };
        assert_eq!(unsafe { *array_write.add(1) } as usize, stereo.write(1) as usize);
        let array_read = unsafe { hirari_audio_buffer_storage_array_read(stereo.0) };
        assert_eq!(unsafe { *array_read } as usize, stereo.read(0) as usize);

        stereo.fill(0, &[0.0, f32::INFINITY, 0.0, 0.0]);
        assert_eq!(unsafe { hirari_audio_buffer_storage_sanitize(stereo.0) }, 1);
        assert_eq!(stereo.sample(0, 1), 0.0);

        unsafe { hirari_audio_buffer_storage_clear(stereo.0, 0, 1) };
        assert_eq!(stereo.sample(0, 0), 0.0);
    }

    #[test]
    fn another_storage_is_summed_and_an_external_wrap_replaces_the_block() {
        let destination = Storage::owned(2, 4);
        let source = Storage::owned(2, 4);
        source.fill(0, &[2.0, 0.0, 0.0, 0.0]);
        source.fill(1, &[-2.0, 0.0, 0.0, 0.0]);
        unsafe { hirari_audio_buffer_storage_add_from(destination.0, source.0, 4) };
        assert_eq!(destination.sample(0, 0), 2.0);
        assert_eq!(destination.sample(1, 0), -2.0);

        let mut external_left = [1.0_f32, 0.0, 0.0, 0.0];
        let mut external_right = [-1.0_f32, 0.0, 0.0, 0.0];
        let mut planes = [external_left.as_mut_ptr(), external_right.as_mut_ptr()];
        let wrapped = Storage::empty();
        assert!(unsafe { hirari_audio_buffer_storage_wrap_channels(wrapped.0, planes.as_mut_ptr(), 2, 4) });
        assert_eq!(wrapped.dimensions().0, 2);
        assert_eq!(wrapped.write(0), external_left.as_mut_ptr());

        // An invalid wrap releases the owned block and leaves an empty view.
        assert!(!unsafe { hirari_audio_buffer_storage_wrap_channels(wrapped.0, ptr::null_mut(), 0, 0) });
        assert_eq!(wrapped.dimensions(), (0, 0));
    }

    #[test]
    fn the_realtime_guard_declines_growth_and_reports_why() {
        use super::{
            hirari_audio_buffer_last_realtime_requested_capacity,
            hirari_audio_buffer_realtime_resize_attempts, hirari_audio_buffer_set_realtime_guard,
            AUDIO_BUFFER_INLINE_CHANNELS,
        };
        use crate::status_queue::{hirari_status_queue_pop_default, StatusMessage};

        let stereo = Storage::owned(2, 16);
        // A wide channel table inside this sample block is declined by the
        // pointer rule rather than the sample rule.
        let wide = Storage::owned(1, 512);
        let attempts = unsafe { hirari_audio_buffer_realtime_resize_attempts() };
        hirari_audio_buffer_set_realtime_guard(true);
        let sample_status =
            unsafe { hirari_audio_buffer_storage_reserve_buffer(stereo.0, 2, 128) };
        let sample_capacity = unsafe { hirari_audio_buffer_last_realtime_requested_capacity() };
        let pointer_status =
            unsafe { hirari_audio_buffer_storage_reserve_buffer(wide.0, 18, 16) };
        hirari_audio_buffer_set_realtime_guard(false);

        // Both growth plans are declined without touching the block, so the
        // prepared realtime view stays exactly as the callback left it.
        assert_eq!(sample_status, 1);
        assert_eq!(pointer_status, 5);
        assert_eq!(stereo.dimensions(), (2, 16));
        assert_eq!(wide.dimensions(), (1, 512));
        assert_eq!(
            unsafe { hirari_audio_buffer_realtime_resize_attempts() },
            attempts + 2
        );
        assert!(sample_capacity >= 2 * 128);

        // The rejection is published as a Critical notification by the same
        // code that declines it; nothing has to report it on the caller side.
        let mut reasons = [String::new(), String::new()];
        for reason in &mut reasons {
            let mut message = StatusMessage::default();
            assert!(unsafe { hirari_status_queue_pop_default(&mut message) });
            assert_eq!(message.severity, 3);
            let end = message
                .text
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(message.text.len());
            *reason = String::from_utf8_lossy(&message.text[..end]).into_owned();
        }
        assert!(reasons.iter().any(|text| text == "AUDIO_BUFFER_RESIZE_ATTEMPTED"));
        assert!(reasons
            .iter()
            .any(|text| text == "AUDIO_BUFFER_POINTER_RESIZE_ATTEMPTED"));

        // Off the realtime thread the same growth is admitted, and a table
        // beyond the inline channels is allocated then.
        assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(stereo.0, 2, 128) }, 0);
        assert!(unsafe { hirari_audio_buffer_storage_resize(stereo.0, 2, 128) });
        assert_eq!(stereo.dimensions(), (2, 128));
        assert!(unsafe { AUDIO_BUFFER_INLINE_CHANNELS < 18 });
        assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(wide.0, 18, 16) }, 0);
        assert!(unsafe { hirari_audio_buffer_storage_resize(wide.0, 18, 16) });
        assert_eq!(wide.dimensions(), (18, 16));
    }
}
