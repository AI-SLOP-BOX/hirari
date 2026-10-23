//! Source-compatible WAV decoder state for the native AudioBuffer facade.

use crate::preview_audio_runtime::{
    hirari_decode_audio_file, hirari_free_decoded_audio, NativeDecodedAudio,
};
use std::ffi::{c_void, CString};

struct WavDecoderState {
    samples: *mut f32,
    sample_count: usize,
    channels: u32,
    sample_rate: f64,
    bits_per_sample: u32,
}

impl WavDecoderState {
    fn new() -> Self {
        Self { samples: std::ptr::null_mut(), sample_count: 0, channels: 0,
            sample_rate: 0.0, bits_per_sample: 0 }
    }

    fn clear(&mut self) {
        if !self.samples.is_null() {
            unsafe { hirari_free_decoded_audio(self.samples, self.sample_count) };
        }
        *self = Self::new();
    }
}

impl Drop for WavDecoderState {
    fn drop(&mut self) { self.clear(); }
}

#[no_mangle]
pub extern "C" fn hirari_wav_decoder_create() -> *mut c_void {
    Box::into_raw(Box::new(WavDecoderState::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<WavDecoderState>()) });
    }
}

fn supported_wav_path(path: &[u8]) -> bool {
    let Some(dot) = path.iter().rposition(|byte| *byte == b'.') else { return false };
    if path.iter().rposition(|byte| *byte == b'/' || *byte == b'\\')
        .is_some_and(|separator| dot < separator) {
        return false;
    }
    let extension = &path[dot + 1..];
    extension.eq_ignore_ascii_case(b"wav")
        || extension.eq_ignore_ascii_case(b"wave")
        || extension.eq_ignore_ascii_case(b"rf64")
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_frames(state: *const c_void) -> u32 {
    unsafe { state.cast::<WavDecoderState>().as_ref() }.map_or(0, |state| {
        if state.channels == 0 { 0 } else { (state.sample_count / state.channels as usize) as u32 }
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_open(
    state: *mut c_void,
    path: *const u8,
    path_size: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<WavDecoderState>().as_mut() }) else { return false };
    state.clear();
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 {
        return false;
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    if !supported_wav_path(path_bytes) {
        return false;
    }
    let c_path = match CString::new(path_bytes.split(|byte| *byte == 0).next().unwrap_or_default()) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let decoded: NativeDecodedAudio = unsafe { hirari_decode_audio_file(c_path.as_ptr()) };
    if decoded.status != 0 || decoded.samples.is_null() || decoded.channels == 0
        || decoded.channels > 32 || decoded.sample_count == 0
        || decoded.sample_count % decoded.channels as usize != 0
        || decoded.sample_count > (512_u64 * 1024 * 1024 / 4) as usize
        || !decoded.sample_rate.is_finite()
        || !(8_000.0..=384_000.0).contains(&decoded.sample_rate)
        || decoded.sample_count / decoded.channels as usize > u32::MAX as usize
    {
        if !decoded.samples.is_null() {
            unsafe { hirari_free_decoded_audio(decoded.samples, decoded.sample_count) };
        }
        return false;
    }
    state.samples = decoded.samples;
    state.sample_count = decoded.sample_count;
    state.channels = decoded.channels;
    state.sample_rate = decoded.sample_rate;
    state.bits_per_sample = decoded.bits_per_sample;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_copy_to_channels(
    state: *const c_void,
    output: *const *mut f32,
    channels: u32,
    frames: u32,
) -> bool {
    let Some(state) = (unsafe { state.cast::<WavDecoderState>().as_ref() }) else { return false };
    if state.samples.is_null() || output.is_null() || channels != state.channels
        || frames as usize != state.sample_count / state.channels as usize
    {
        return false;
    }
    for channel in 0..channels as usize {
        let destination = unsafe { *output.add(channel) };
        if destination.is_null() { return false; }
        for frame in 0..frames as usize {
            unsafe {
                destination.add(frame).write(*state.samples.add(frame * channels as usize + channel));
            }
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_sample_rate(state: *const c_void) -> f64 {
    unsafe { state.cast::<WavDecoderState>().as_ref() }.map_or(0.0, |state| state.sample_rate)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_channels(state: *const c_void) -> u32 {
    unsafe { state.cast::<WavDecoderState>().as_ref() }.map_or(0, |state| state.channels)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_decoder_bits_per_sample(state: *const c_void) -> u32 {
    unsafe { state.cast::<WavDecoderState>().as_ref() }.map_or(0, |state| state.bits_per_sample)
}
