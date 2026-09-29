//! Allocation-free sample operations for the native AudioBuffer adapter.

use std::alloc::{alloc, dealloc, Layout};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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

struct AlignedAudioStorage {
    samples: *mut f32,
    capacity: usize,
}

impl Drop for AlignedAudioStorage {
    fn drop(&mut self) {
        self.release();
    }
}

impl AlignedAudioStorage {
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
    Box::into_raw(Box::new(AlignedAudioStorage {
        samples: std::ptr::null_mut(),
        capacity: 0,
    }))
    .cast()
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

/// Copies and sanitizes a selected stereo input pair for the input monitor.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_copy_monitor_input(
    inputs: *const *const f32,
    channel_count: u32,
    left_channel: u32,
    right_channel: u32,
    frames: u32,
    output_left: *mut f32,
    output_right: *mut f32,
) -> bool {
    if inputs.is_null()
        || output_left.is_null()
        || output_right.is_null()
        || left_channel >= channel_count
        || right_channel >= channel_count
    {
        return false;
    }
    let channels = unsafe { std::slice::from_raw_parts(inputs, channel_count as usize) };
    let (Some(&source_left), Some(&source_right)) = (
        channels.get(left_channel as usize),
        channels.get(right_channel as usize),
    ) else {
        return false;
    };
    if source_left.is_null() || source_right.is_null() {
        return false;
    }
    let (output_left, output_right, source_left, source_right) = unsafe {
        (
            std::slice::from_raw_parts_mut(output_left, frames as usize),
            std::slice::from_raw_parts_mut(output_right, frames as usize),
            std::slice::from_raw_parts(source_left, frames as usize),
            std::slice::from_raw_parts(source_right, frames as usize),
        )
    };
    for (((out_left, out_right), in_left), in_right) in output_left
        .iter_mut()
        .zip(output_right)
        .zip(source_left)
        .zip(source_right)
    {
        *out_left = if in_left.is_finite() { *in_left } else { 0.0 };
        *out_right = if in_right.is_finite() { *in_right } else { 0.0 };
    }
    true
}

/// Copies and clamps a talkback input channel into the monitor staging block.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_copy_talkback_input(
    inputs: *const *const f32,
    channel_count: u32,
    input_channel: u32,
    frames: u32,
    output: *mut f32,
) -> bool {
    if inputs.is_null() || output.is_null() || input_channel >= channel_count {
        return false;
    }
    let channels = unsafe { std::slice::from_raw_parts(inputs, channel_count as usize) };
    let Some(&source) = channels.get(input_channel as usize) else {
        return false;
    };
    if source.is_null() {
        return false;
    }
    let (output, source) = unsafe {
        (
            std::slice::from_raw_parts_mut(output, frames as usize),
            std::slice::from_raw_parts(source, frames as usize),
        )
    };
    for (out, input) in output.iter_mut().zip(source) {
        *out = if input.is_finite() {
            input.clamp(-16.0, 16.0)
        } else {
            0.0
        };
    }
    true
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
