//! FFI surface of the audio buffer adapter, split out of audio_buffer_ops.rs.
use super::*;

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
        unsafe {
            crate::status_queue::hirari_status_queue_push_default(
                crate::status_queue::SEVERITY_CRITICAL,
                text.as_ptr(),
                text.len(),
            )
        };
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
