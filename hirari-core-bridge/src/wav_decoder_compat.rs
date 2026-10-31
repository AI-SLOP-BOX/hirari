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

pub(crate) struct DecodedWav {
    pub samples: *mut f32,
    pub sample_count: usize,
    pub channels: u32,
    pub sample_rate: f64,
    pub bits_per_sample: u32,
}

/// Shared RIFF/RF64 decode boundary used by the AudioBuffer adapter and the
/// path-based diagnostic. A `None` result owns nothing.
pub(crate) fn decode_wav_file(path_bytes: &[u8]) -> Option<DecodedWav> {
    if path_bytes.is_empty() || path_bytes.len() > 1024 * 1024 || !supported_wav_path(path_bytes) {
        return None;
    }
    let c_path = CString::new(path_bytes.split(|byte| *byte == 0).next().unwrap_or_default()).ok()?;
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
        return None;
    }
    Some(DecodedWav {
        samples: decoded.samples,
        sample_count: decoded.sample_count,
        channels: decoded.channels,
        sample_rate: decoded.sample_rate,
        bits_per_sample: decoded.bits_per_sample,
    })
}

impl WavDecoderState {
    fn new() -> Self {
        Self { samples: std::ptr::null_mut(), sample_count: 0, channels: 0,
            sample_rate: 0.0, bits_per_sample: 0 }
    }

    fn clear(&mut self) {
        if !self.samples.is_null() {
            unsafe { hirari_free_decoded_audio(self.samples, self.sample_count) };
            self.samples = std::ptr::null_mut();
        }
        self.sample_count = 0;
        self.channels = 0;
        self.sample_rate = 0.0;
        self.bits_per_sample = 0;
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
    if path.is_null() || path_size == 0 {
        return false;
    }
    let Some(decoded) = decode_wav_file(unsafe { std::slice::from_raw_parts(path, path_size) })
    else {
        return false;
    };
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

// WAV write/decode contract: both sides are Rust-owned, so the assertions
// live next to the implementation instead of behind a C++ facade.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{hirari_wav_planar_shapes_valid, hirari_wav_write_planar};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hirari-wav-contract-{name}-{}.wav",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn path_bytes(path: &std::path::Path) -> Vec<u8> {
        path.to_string_lossy().into_owned().into_bytes()
    }

    fn write_planes(path: &std::path::Path, channels: &[Vec<f32>], rate: u32, format: u8) -> u8 {
        let sizes: Vec<usize> = channels.iter().map(Vec::len).collect();
        let frames = channels.first().map_or(0, Vec::len) as u64;
        let planes: Vec<*const f32> = channels.iter().map(|channel| channel.as_ptr()).collect();
        let bytes = path_bytes(path);
        unsafe {
            hirari_wav_write_planar(
                bytes.as_ptr(),
                bytes.len(),
                planes.as_ptr(),
                planes.len(),
                frames,
                rate,
                format,
            )
        }
    }

    /// Decoded interleaved samples, channel count and sample rate.
    fn decode(path: &std::path::Path) -> Option<(Vec<f32>, u32, u32)> {
        let bytes = path_bytes(path);
        let decoded = decode_wav_file(&bytes)?;
        let samples = unsafe {
            std::slice::from_raw_parts(decoded.samples, decoded.sample_count).to_vec()
        };
        let channels = decoded.channels;
        let sample_rate = decoded.sample_rate as u32;
        unsafe { hirari_free_decoded_audio(decoded.samples, decoded.sample_count) };
        Some((samples, channels, sample_rate))
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from(bytes[offset]) | (u16::from(bytes[offset + 1]) << 8)
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from(bytes[offset])
            | (u32::from(bytes[offset + 1]) << 8)
            | (u32::from(bytes[offset + 2]) << 16)
            | (u32::from(bytes[offset + 3]) << 24)
    }

    /// Locates the data chunk without assuming it is the second chunk; the
    /// writer emits a padding JUNK chunk before fmt.
    fn find_data_chunk(bytes: &[u8]) -> Option<(u32, u16, u16, u32)> {
        if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return None;
        }
        let mut offset = 12;
        let mut format = (0u16, 0u16, 0u32);
        while offset + 8 <= bytes.len() {
            let tag = &bytes[offset..offset + 4];
            let chunk_size = read_u32(bytes, offset + 4);
            if tag == b"fmt " {
                format = (
                    read_u16(bytes, offset + 10),
                    read_u16(bytes, offset + 20),
                    read_u32(bytes, offset + 12),
                );
            } else if tag == b"data" {
                return Some((chunk_size, format.0, format.1, format.2));
            }
            offset += 8 + chunk_size as usize + (chunk_size as usize & 1);
        }
        None
    }

    fn check_stereo_round_trip(frames: usize, sample_rate: u32) {
        let left: Vec<f32> = (0..frames).map(|i| i as f32 * 0.25 - 1.0).collect();
        let right: Vec<f32> = left.iter().map(|value| -value).collect();
        let path = temp_path(&format!("roundtrip-{frames}-{sample_rate}"));
        assert_eq!(
            write_planes(&path, &[left.clone(), right.clone()], sample_rate, 0),
            0
        );

        let (samples, channels, rate) = decode(&path).expect("round trip must decode");
        assert_eq!(channels, 2);
        assert_eq!(rate, sample_rate);
        assert_eq!(samples.len(), frames * 2);
        for index in 0..frames {
            assert_eq!(samples[index * 2], left[index]);
            assert_eq!(samples[index * 2 + 1], right[index]);
        }

        // Independently confirm the container so a writer that agreed with its
        // own loader but wrote a malformed header would still be caught.
        let bytes = std::fs::read(&path).expect("written file must exist");
        let (data_size, channels, block_align, rate) =
            find_data_chunk(&bytes).expect("data chunk must exist");
        assert_eq!(channels, 2);
        assert_eq!(block_align, 8); // 2 channels * 4 bytes (IEEE float)
        assert_eq!(rate, sample_rate);
        assert_eq!(data_size as usize, frames * 2 * 4);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn float32_round_trip_preserves_short_buffer_frames() {
        // Short buffers are where a frame-count bug would hide.
        for frames in [1usize, 2, 4, 6, 64, 512] {
            check_stereo_round_trip(frames, 48_000);
        }
        check_stereo_round_trip(64, 44_100);
        check_stereo_round_trip(512, 48_000);
    }

    #[test]
    fn multichannel_pcm16_round_trip_accepts_a_single_frame() {
        let channels = vec![vec![0.25f32], vec![-0.25], vec![0.5], vec![-0.5]];
        let path = temp_path("pcm16");
        assert_eq!(write_planes(&path, &channels, 44_100, 1), 0);
        let (samples, decoded_channels, rate) = decode(&path).expect("pcm16 must decode");
        assert_eq!(decoded_channels, channels.len() as u32);
        assert_eq!(rate, 44_100);
        assert_eq!(samples.len(), channels.len());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn planar_shape_admission_rejects_empty_and_mismatched_buffers() {
        let empty: [usize; 0] = [];
        assert!(!unsafe { hirari_wav_planar_shapes_valid(empty.as_ptr(), 0, 0) });
        let zero_frames = [0usize];
        assert!(!unsafe { hirari_wav_planar_shapes_valid(zero_frames.as_ptr(), 1, 0) });
        let mismatched = [1usize, 2];
        assert!(!unsafe { hirari_wav_planar_shapes_valid(mismatched.as_ptr(), 2, 1) });
        let matched = [4usize, 4];
        assert!(unsafe { hirari_wav_planar_shapes_valid(matched.as_ptr(), 2, 4) });
        let too_many = vec![1usize; usize::from(u16::MAX) + 1];
        assert!(!unsafe { hirari_wav_planar_shapes_valid(too_many.as_ptr(), too_many.len(), 1) });

        // The writer itself rejects the empty layout before touching disk.
        let path = temp_path("empty");
        assert_ne!(write_planes(&path, &[], 48_000, 0), 0);
        assert_ne!(write_planes(&path, &[Vec::new()], 48_000, 0), 0);
    }

    #[test]
    fn float_writer_sanitizes_non_finite_samples() {
        let path = temp_path("non-finite");
        let channels = vec![
            vec![0.0f32, 0.5, f32::NAN, -1.2],
            vec![0.0f32, -0.5, f32::INFINITY, 1.2],
        ];
        assert_eq!(write_planes(&path, &channels, 48_000, 0), 0);
        let (samples, decoded_channels, _) = decode(&path).expect("decode must succeed");
        assert_eq!(decoded_channels, 2);
        assert_eq!(samples.len(), 8);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decoder_skips_odd_junk_chunks() {
        let path = temp_path("odd-chunk");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&52u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"abc\0");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&16_384i16.to_le_bytes());
        std::fs::write(&path, &bytes).expect("probe file must be writable");

        let (samples, channels, _) = decode(&path).expect("odd chunk must decode");
        assert_eq!(channels, 1);
        assert_eq!(samples.len(), 1);
        assert!((samples[0] - 0.5).abs() < 1.0e-5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decoder_reads_twenty_thousand_frames_across_the_chunk_boundary() {
        let path = temp_path("chunked");
        let frames = 20_000;
        let left: Vec<f32> = (0..frames)
            .map(|frame| if frame % 97 == 0 { 0.75 } else { -0.125 })
            .collect();
        let right: Vec<f32> = (0..frames)
            .map(|frame| if frame % 113 == 0 { -0.5 } else { 0.25 })
            .collect();
        assert_eq!(write_planes(&path, &[left, right], 48_000, 0), 0);
        let (samples, channels, rate) = decode(&path).expect("decode");
        assert_eq!((channels, rate), (2, 48_000));
        assert_eq!(samples.len(), frames * 2);
        assert!((samples[16_384 * 2] + 0.125).abs() < 0.01);
        assert!((samples[(frames - 1) * 2 + 1] - 0.25).abs() < 0.01);
    }

    #[test]
    fn decoder_refuses_a_sparse_file_that_would_expand_past_the_buffer_limit() {
        let path = temp_path("oversized");
        const EXPANSION_DATA_BYTES: u32 = 300 * 1024 * 1024;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + EXPANSION_DATA_BYTES).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&48_000u32.to_le_bytes());
        header.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        header.extend_from_slice(&4u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&EXPANSION_DATA_BYTES.to_le_bytes());
        let file = std::fs::File::create(&path).expect("create");
        use std::io::Write;
        let mut file = file;
        file.write_all(&header).expect("header");
        file.set_len(44 + u64::from(EXPANSION_DATA_BYTES))
            .expect("sparse");
        drop(file);
        assert!(decode(&path).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decoder_reads_rf64_data_chunk_sizes() {
        let path = temp_path("rf64");
        let mut bytes = Vec::new();
        let riff_size: u64 = 8 + 28 + 8 + 16 + 8 + 4;
        bytes.extend_from_slice(b"RF64");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"ds64");
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&riff_size.to_le_bytes());
        bytes.extend_from_slice(&4u64.to_le_bytes()); // data size
        bytes.extend_from_slice(&2u64.to_le_bytes()); // sample count
        bytes.extend_from_slice(&0u32.to_le_bytes()); // table length
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&16_384i16.to_le_bytes());
        bytes.extend_from_slice(&(-16_384i16).to_le_bytes());
        std::fs::write(&path, &bytes).expect("probe file must be writable");

        let (samples, channels, _) = decode(&path).expect("rf64 must decode");
        assert_eq!(channels, 1);
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.5).abs() < 1.0e-5);
        assert!((samples[1] + 0.5).abs() < 1.0e-5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decoder_rejects_truncated_and_corrupt_headers() {
        let truncated = temp_path("truncated-riff");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&32u32.to_le_bytes());
        std::fs::write(&truncated, &bytes).expect("probe file must be writable");
        assert!(decode(&truncated).is_none());
        let _ = std::fs::remove_file(&truncated);

        let corrupt = temp_path("corrupt");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36u32 + 6).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&2u16.to_le_bytes()); // stereo
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // bogus byte rate
        bytes.extend_from_slice(&1u16.to_le_bytes()); // bogus block align
        bytes.extend_from_slice(&16u16.to_le_bytes());
        std::fs::write(&corrupt, &bytes).expect("probe file must be writable");
        assert!(decode(&corrupt).is_none());
        let _ = std::fs::remove_file(&corrupt);
    }
}
