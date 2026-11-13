//! Probe/decode machinery and FFI exports for the preview audio runtime,
//! split out of preview_audio_runtime.rs.
use super::*;

pub(crate) fn probe_audio(path: &Path) -> Result<(f64, u16, u64), String> {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let result = (|| {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let source = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|error| format!("unsupported or invalid audio file: {error}"))?;
        let track = probed
            .format
            .default_track()
            .ok_or_else(|| "audio file has no default track".to_owned())?;
        symphonia::default::get_codecs()
            .make(
                &track.codec_params,
                &symphonia::core::codecs::DecoderOptions::default(),
            )
            .map_err(|error| format!("unsupported audio codec: {error}"))?;
        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| "audio file has no sample rate".to_owned())?;
        let channels = track
            .codec_params
            .channels
            .ok_or_else(|| "audio file has no channel layout".to_owned())?
            .count();
        if !(1..=384_000).contains(&sample_rate) || !(1..=32).contains(&channels) {
            return Err("audio file sample rate or channel count is unsupported".into());
        }
        Ok((
            f64::from(sample_rate),
            channels as u16,
            track.codec_params.n_frames.unwrap_or_default(),
        ))
    })();

    match result {
        Ok(metadata) => Ok(metadata),
        Err(error) if is_wav_path(path) => probe_wav(path).map_err(|fallback_error| {
            format!("{error}; WAV metadata fallback failed: {fallback_error}")
        }),
        Err(error) => Err(error),
    }
}

pub(crate) fn is_wav_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("wav") || extension.eq_ignore_ascii_case("wave")
        })
}

pub(crate) fn decode_audio(path: &Path) -> Result<(Vec<f32>, f64), String> {
    let (interleaved, sample_rate, channels, _) = decode_audio_channels(path)?;
    let mut mono = Vec::with_capacity(interleaved.len() / channels as usize);
    for frame in interleaved.chunks_exact(channels as usize) {
        mono.push(frame.iter().copied().sum::<f32>() / channels as f32);
    }
    Ok((mono, sample_rate))
}

pub(crate) fn decode_audio_channels(path: &Path) -> Result<(Vec<f32>, f64, u32, u32), String> {
    match decode_with_symphonia(path) {
        Ok(audio) => Ok(audio),
        Err(error)
            if is_wav_path(path)
                || path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("rf64")) =>
        {
            decode_wav(path).map_err(|fallback_error| {
                format!("{error}; WAV decoder fallback failed: {fallback_error}")
            })
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn decode_with_symphonia(path: &Path) -> Result<(Vec<f32>, f64, u32, u32), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = File::open(path).map_err(|error| format!("unable to open audio file: {error}"))?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions {
                enable_gapless: true,
                ..FormatOptions::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("unsupported or invalid audio file: {error}"))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "audio file has no default track".to_owned())?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let bits_per_sample = codec_params.bits_per_sample.unwrap_or(32);
    let sample_rate = codec_params
        .sample_rate
        .filter(|rate| (1..=384_000).contains(rate))
        .ok_or_else(|| "audio file has no supported sample rate".to_owned())?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|error| format!("unsupported audio codec: {error}"))?;
    let mut interleaved_samples = Vec::new();
    let mut decoded_channels = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => {
                return Err("chained audio streams are not supported in browser previews".into());
            }
            Err(error) => return Err(format!("audio packet read failed: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|error| format!("audio packet decode failed: {error}"))?;
        let channel_count = decoded.spec().channels.count();
        if channel_count == 0 || channel_count > 32 {
            return Err("audio file channel count is unsupported".into());
        }
        if decoded_channels.is_some_and(|channels| channels != channel_count) {
            return Err("audio stream changed channel count while decoding".into());
        }
        decoded_channels = Some(channel_count);
        let mut interleaved = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        interleaved.copy_interleaved_ref(decoded);
        let samples = interleaved.samples();
        if samples.len() % channel_count != 0 {
            return Err("decoded audio packet has incomplete frames".into());
        }
        if interleaved_samples.len().saturating_add(samples.len()) > 134_217_728 {
            return Err("decoded audio exceeds the 512 MiB PCM limit".into());
        }
        for sample in samples {
            if !sample.is_finite() {
                return Err("audio file contains a non-finite sample".into());
            }
            interleaved_samples.push(*sample);
        }
    }

    if interleaved_samples.is_empty() {
        return Err("audio file contains no decoded samples".into());
    }
    Ok((
        interleaved_samples,
        f64::from(sample_rate),
        decoded_channels.unwrap_or(1) as u32,
        bits_per_sample,
    ))
}

pub(crate) fn probe_wav(path: &Path) -> Result<(f64, u16, u64), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return Err("unsupported WAV".into());
    }
    let mut pos = 12usize;
    let mut audio_format = 0u16;
    let mut sample_rate = 0.0;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut block_align = 0u16;
    let mut data_bytes = 0u64;
    let rf64 = &bytes[0..4] == b"RF64";
    let mut rf64_data_size = None;
    while pos.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[pos..pos + 4];
        let encoded_size = u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV chunk size")?);
        pos += 8;
        if id == b"ds64" && encoded_size >= 28 {
            rf64_data_size = Some(u64::from_le_bytes(read_array::<8>(
                &bytes,
                pos + 8,
                "RF64 data size",
            )?));
        }
        let size_u64 = if rf64 && id == b"data" && encoded_size == u32::MAX {
            rf64_data_size.ok_or("RF64 data size is missing")?
        } else {
            u64::from(encoded_size)
        };
        let size = usize::try_from(size_u64).map_err(|_| "WAV chunk is too large")?;
        let end = pos.checked_add(size).ok_or("WAV chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated WAV".into());
        }
        if id == b"fmt " && size >= 16 {
            audio_format = u16::from_le_bytes(read_array::<2>(&bytes, pos, "WAV format")?);
            channels = u16::from_le_bytes(read_array::<2>(&bytes, pos + 2, "WAV channels")?);
            sample_rate =
                u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV sample rate")?) as f64;
            block_align =
                u16::from_le_bytes(read_array::<2>(&bytes, pos + 12, "WAV block alignment")?);
            bits = u16::from_le_bytes(read_array::<2>(&bytes, pos + 14, "WAV bits")?);
        } else if id == b"data" {
            data_bytes = size_u64;
        }
        pos = end.checked_add(size & 1).ok_or("WAV alignment overflow")?;
    }
    let bytes_per_frame = u64::from(block_align);
    if channels == 0
        || !matches!(bits, 16 | 24 | 32)
        || !matches!(audio_format, 1 | 3)
        || (audio_format == 3 && bits != 32)
        || bytes_per_frame == 0
        || bytes_per_frame != u64::from(channels) * u64::from(bits / 8)
        || data_bytes == 0
        || data_bytes % bytes_per_frame != 0
    {
        return Err("unsupported or incomplete WAV metadata".into());
    }
    Ok((sample_rate, channels, data_bytes / bytes_per_frame))
}

pub(crate) fn decode_wav(path: &Path) -> Result<(Vec<f32>, f64, u32, u32), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 512 * 1024 * 1024 {
        return Err("WAV file exceeds the 512 MiB decode limit".into());
    }
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return Err("unsupported WAV".into());
    }
    let rf64 = &bytes[0..4] == b"RF64";
    let riff_size = u32::from_le_bytes(read_array::<4>(&bytes, 4, "RIFF size")?) as usize;
    if !rf64 && (riff_size < 4 || riff_size.checked_add(8).is_none_or(|end| end > bytes.len())) {
        return Err("invalid RIFF size".into());
    }
    let mut pos = 12;
    let mut audio_format = 1u16;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut sample_rate = 44100.0f64;
    let mut data = None;
    let mut fmt_found = false;
    let mut rf64_data_size: Option<u64> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let encoded_size = u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV chunk size")?);
        pos += 8;
        if id == b"ds64" && encoded_size >= 28 {
            let ds64_end = pos.checked_add(28).ok_or("RF64 ds64 overflow")?;
            if ds64_end > bytes.len() {
                return Err("truncated RF64 ds64 chunk".into());
            }
            rf64_data_size = Some(u64::from_le_bytes(read_array::<8>(
                &bytes,
                pos + 8,
                "RF64 data size",
            )?));
        }
        let size = if rf64 && id == b"data" && encoded_size == u32::MAX {
            usize::try_from(rf64_data_size.ok_or("RF64 data size is missing")?)
                .map_err(|_| "RF64 data size is too large")?
        } else {
            encoded_size as usize
        };
        let end = pos.checked_add(size).ok_or("WAV chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated WAV".into());
        }
        if id == b"fmt " && size >= 16 {
            fmt_found = true;
            audio_format = u16::from_le_bytes(read_array::<2>(&bytes, pos, "WAV format")?);
            channels = u16::from_le_bytes(read_array::<2>(&bytes, pos + 2, "WAV channel count")?);
            sample_rate =
                u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV sample rate")?) as f64;
            bits = u16::from_le_bytes(read_array::<2>(&bytes, pos + 14, "WAV bit depth")?);
        }
        if id == b"data" {
            data = Some((pos, end));
        }
        let next = end
            .checked_add(size & 1)
            .ok_or("WAV chunk alignment overflow")?;
        if next > bytes.len() {
            return Err("truncated WAV chunk padding".into());
        }
        pos = next;
    }
    let (start, end) = data.ok_or("WAV data missing")?;
    if start == end {
        return Err("WAV data is empty".into());
    }
    if !fmt_found
        || channels == 0
        || channels > 32
        || !matches!(bits, 16 | 24 | 32)
        || !matches!(audio_format, 1 | 3)
        || (audio_format == 3 && bits != 32)
    {
        return Err("only PCM16/24/32 or float32 WAV is supported".into());
    }
    let bytes_per = (bits / 8) as usize;
    let frame_bytes = bytes_per * channels as usize;
    if frame_bytes == 0 {
        return Err("invalid WAV channels".into());
    }
    if (end - start) % frame_bytes != 0 {
        return Err("WAV data is not aligned to complete frames".into());
    }
    let output_samples = (end - start) / bytes_per;
    if output_samples > 134_217_728 {
        return Err("decoded WAV exceeds the 512 MiB PCM limit".into());
    }
    let mut result = Vec::with_capacity(output_samples);
    for frame in (start..end).step_by(frame_bytes) {
        for ch in 0..channels as usize {
            let p = frame + ch * bytes_per;
            let v = if bits == 16 {
                i16::from_le_bytes(read_array::<2>(&bytes, p, "PCM16 sample")?) as f32 / 32768.0
            } else if bits == 24 {
                let raw = (bytes[p] as i32)
                    | ((bytes[p + 1] as i32) << 8)
                    | ((bytes[p + 2] as i32) << 16);
                let signed = if raw & 0x0080_0000 != 0 {
                    raw | !0x00FF_FFFF
                } else {
                    raw
                };
                signed as f32 / 8_388_608.0
            } else if audio_format == 3 {
                f32::from_le_bytes(read_array::<4>(&bytes, p, "float32 sample")?)
            } else {
                i32::from_le_bytes(read_array::<4>(&bytes, p, "PCM32 sample")?) as f32
                    / 2147483648.0
            };
            if !v.is_finite() {
                return Err("WAV contains a non-finite sample".into());
            }
            result.push(v);
        }
    }
    if !sample_rate.is_finite() || !(1.0..=384_000.0).contains(&sample_rate) {
        return Err("invalid WAV sample rate".into());
    }
    Ok((result, sample_rate, channels as u32, u32::from(bits)))
}

/// C ABI used by the native engine's compatibility adapter. The media decode
/// implementation stays in Rust; the returned boxed slice is released by
/// `hirari_free_decoded_audio` after it has been copied into AudioBuffer.
#[repr(C)]
pub struct NativeDecodedAudio {
    pub(crate) samples: *mut f32,
    pub(crate) sample_count: usize,
    pub(crate) channels: u32,
    pub(crate) sample_rate: f64,
    pub(crate) bits_per_sample: u32,
    pub(crate) status: u32,
}

#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_decode_audio_file(
    path: *const std::ffi::c_char,
) -> NativeDecodedAudio {
    use std::ffi::CStr;

    if path.is_null() {
        return NativeDecodedAudio {
            samples: std::ptr::null_mut(),
            sample_count: 0,
            channels: 0,
            sample_rate: 0.0,
            bits_per_sample: 0,
            status: 1,
        };
    }
    let path = match CStr::from_ptr(path).to_str() {
        Ok(path) => Path::new(path),
        Err(_) => {
            return NativeDecodedAudio {
                samples: std::ptr::null_mut(),
                sample_count: 0,
                channels: 0,
                sample_rate: 0.0,
                bits_per_sample: 0,
                status: 2,
            }
        }
    };
    match decode_audio_channels(path) {
        Ok((samples, sample_rate, channels, bits_per_sample)) => {
            let sample_count = samples.len();
            let samples = Box::into_raw(samples.into_boxed_slice()) as *mut f32;
            NativeDecodedAudio {
                samples,
                sample_count,
                channels,
                sample_rate,
                bits_per_sample,
                status: 0,
            }
        }
        Err(_) => NativeDecodedAudio {
            samples: std::ptr::null_mut(),
            sample_count: 0,
            channels: 0,
            sample_rate: 0.0,
            bits_per_sample: 0,
            status: 3,
        },
    }
}

/// Import-only decoder boundary: validates the supported container suffix and
/// metadata before exposing the interleaved allocation to the native buffer
/// adapter. Status 4 means unsupported suffix; status 3 preserves the native
/// decoder's invalid-file result; status 5 means decoded metadata was invalid.
#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_import_audio_file(
    path: *const u8,
    path_size: usize,
) -> NativeDecodedAudio {
    let empty = |status| NativeDecodedAudio {
        samples: std::ptr::null_mut(),
        sample_count: 0,
        channels: 0,
        sample_rate: 0.0,
        bits_per_sample: 0,
        status,
    };
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 {
        return empty(2);
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    let Some(dot) = path_bytes.iter().rposition(|byte| *byte == b'.') else {
        return empty(4);
    };
    if path_bytes.iter().rposition(|byte| *byte == b'/' || *byte == b'\\')
        .is_some_and(|separator| dot < separator) {
        return empty(4);
    }
    let extension = &path_bytes[dot + 1..];
    if extension.is_empty() || ![
        b"wav".as_slice(), b"wave", b"rf64", b"mp3", b"flac",
        b"aif", b"aiff", b"m4a", b"ogg", b"aac",
    ].iter().any(|supported| extension.eq_ignore_ascii_case(supported)) {
        return empty(4);
    }
    let Ok(path) = std::str::from_utf8(path_bytes) else { return empty(2) };
    let (samples, sample_rate, channels, bits_per_sample) =
        match decode_audio_channels(Path::new(path)) {
            Ok(decoded) => decoded,
            Err(_) => return empty(3),
        };
    if channels == 0 || channels > 32 || samples.is_empty()
        || samples.len() % channels as usize != 0
        || samples.len() > (512_u64 * 1024 * 1024 / 4) as usize
        || !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate)
        || samples.len() / channels as usize > u32::MAX as usize
    {
        return empty(5);
    }
    let sample_count = samples.len();
    NativeDecodedAudio {
        samples: Box::into_raw(samples.into_boxed_slice()) as *mut f32,
        sample_count,
        channels,
        sample_rate,
        bits_per_sample,
        status: 0,
    }
}

#[no_mangle]
pub(crate) unsafe extern "C" fn hirari_free_decoded_audio(samples: *mut f32, sample_count: usize) {
    if !samples.is_null() {
        let slice = std::ptr::slice_from_raw_parts_mut(samples, sample_count);
        drop(Box::from_raw(slice));
    }
}

pub(crate) fn read_array<const N: usize>(bytes: &[u8], start: usize, what: &str) -> Result<[u8; N], String> {
    let end = start
        .checked_add(N)
        .ok_or_else(|| format!("{} offset overflow", what))?;
    let slice = bytes
        .get(start..end)
        .ok_or_else(|| format!("truncated {}", what))?;
    slice
        .try_into()
        .map_err(|_| format!("invalid {} length", what))
}
