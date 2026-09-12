/// Returns an interleaved frame range for selection/loop exports without
/// mutating the source render buffer.
pub fn select_frame_range(
    samples: &[f32],
    channels: u16,
    start_frame: usize,
    end_frame: usize,
) -> Result<Vec<f32>, WavExportError> {
    if channels == 0 || !samples.len().is_multiple_of(channels as usize) {
        return Err(WavExportError::IncompleteFrame);
    }
    let total_frames = samples.len() / channels as usize;
    if start_frame > end_frame || end_frame > total_frames {
        return Err(WavExportError::InvalidTask);
    }
    Ok(samples[start_frame * channels as usize..end_frame * channels as usize].to_vec())
}

/// Applies deterministic peak normalization and optional triangular dither in
/// the offline export path. The audio callback never calls this function.
pub fn prepare_export_buffer(
    samples: &[f32],
    bit_depth: u32,
    normalize_peak: bool,
    dither: bool,
) -> Result<Vec<f32>, WavExportError> {
    if samples.is_empty() {
        return Err(WavExportError::EmptyBuffer);
    }
    if !matches!(bit_depth, 16 | 24 | 32) {
        return Err(WavExportError::UnsupportedFormat);
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err(WavExportError::NonFiniteSample);
    }
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let gain = if normalize_peak && peak > 0.0 {
        1.0 / peak
    } else {
        1.0
    };
    let step = 1.0 / (1u64 << (bit_depth - 1)) as f32;
    let mut state = 0x0A0A_2026_u64;
    let mut output = Vec::with_capacity(samples.len());
    for sample in samples {
        let noise = if dither && bit_depth < 32 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let a = ((state >> 32) as u32) as f32 / u32::MAX as f32;
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let b = ((state >> 32) as u32) as f32 / u32::MAX as f32;
            (a - b) * step
        } else {
            0.0
        };
        output.push((sample * gain + noise).clamp(-1.0, 1.0));
    }
    Ok(output)
}

/// Writes one validated file per stem. A stem is published only after its
/// own atomic WAV write succeeds; a missing or invalid stem aborts the batch.
pub fn export_stems_to_wav(
    output_dir: &Path,
    stems: &[(String, Vec<f32>)],
    sample_rate: u32,
    channels: u16,
    bit_depth: u32,
    normalize_peak: bool,
    dither: bool,
) -> Result<Vec<std::path::PathBuf>, WavExportError> {
    if !output_dir.is_dir() || stems.is_empty() {
        return Err(WavExportError::InvalidPath);
    }
    let mut paths = Vec::with_capacity(stems.len());
    let mut used_names = std::collections::HashSet::with_capacity(stems.len());
    for (name, samples) in stems {
        let base_name: String = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        if base_name.is_empty() {
            return Err(WavExportError::InvalidPath);
        }
        let mut safe_name = base_name.clone();
        let mut suffix = 2usize;
        while !used_names.insert(safe_name.clone())
            || output_dir.join(format!("{safe_name}.wav")).exists()
        {
            safe_name = format!("{base_name}_{suffix}");
            suffix += 1;
        }
        let prepared = match prepare_export_buffer(samples, bit_depth, normalize_peak, dither) {
            Ok(prepared) => prepared,
            Err(error) => {
                for path in &paths {
                    let _ = std::fs::remove_file(path);
                }
                return Err(error);
            }
        };
        let path = output_dir.join(format!("{safe_name}.wav"));
        if let Err(error) = write_wav_pcm(&path, &prepared, sample_rate, channels, bit_depth) {
            for prior in &paths {
                let _ = std::fs::remove_file(prior);
            }
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        paths.push(path);
    }
    Ok(paths)
}

/// Exports a completed interleaved render buffer through the existing WAV writer.
///
/// `renderer_connected` is intentionally explicit: an unconnected renderer must
/// never be reported as a successful export merely because a destination path is
/// available. This function is for an offline/export worker, not an audio callback.
pub fn export_interleaved_buffer_to_wav(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    renderer_connected: bool,
) -> Result<(), WavExportError> {
    export_interleaved_buffer_to_wav_with_format(
        path,
        samples,
        sample_rate,
        channels,
        16,
        false,
        renderer_connected,
    )
}

/// Publishes a completed render as PCM16 AIFF from an export worker.
pub fn export_interleaved_buffer_to_aiff(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    renderer_connected: bool,
) -> Result<(), WavExportError> {
    if !renderer_connected { return Err(WavExportError::RendererNotConnected); }
    write_aiff_pcm16(path, samples, sample_rate, channels)
}

/// Encodes a completed render as FLAC through the installed FFmpeg tool.
/// The intermediate WAV is private and removed on every exit path; the FLAC
/// destination is considered published only after FFmpeg exits successfully.
pub fn export_interleaved_buffer_to_flac(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    renderer_connected: bool,
) -> Result<(), WavExportError> {
    if !renderer_connected { return Err(WavExportError::RendererNotConnected); }
    if path.as_os_str().is_empty() { return Err(WavExportError::InvalidPath); }
    let temporary = path.with_extension(format!("flac-input-{}.wav", std::process::id()));
    write_wav_pcm16(&temporary, samples, sample_rate, channels)?;
    let _output_lock = match OutputPublicationLock::acquire(path) {
        Ok(lock) => lock,
        Err(error) => { let _ = fs::remove_file(&temporary); return Err(error); }
    };
    let result = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&temporary)
        .args(["-codec:a", "flac", "-blocksize", "4096"])
        .arg(path)
        .status()
        .map_err(|error| WavExportError::Io(error.to_string()))
        .and_then(|status| if status.success() { Ok(()) } else { Err(WavExportError::Io("ffmpeg FLAC encoding failed".into())) });
    let _ = fs::remove_file(&temporary);
    if result.is_err() { let _ = fs::remove_file(path); }
    result
}
