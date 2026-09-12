#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossyAudioCodec { Mp3, Aac }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioTagMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u16>,
    pub track: Option<u16>,
    pub genre: String,
    pub comment: String,
}

impl AudioTagMetadata {
    pub fn validate(&self) -> bool {
        [&self.title, &self.artist, &self.album, &self.genre, &self.comment]
            .into_iter().all(|value| value.len() <= 1024 && !value.contains('\0'))
            && self.year.is_none_or(|year| (1000..=9999).contains(&year))
            && self.track.is_none_or(|track| (1..=999).contains(&track))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LossyExportSettings {
    pub bit_rate_kbps: u16,
    pub high_quality: bool,
    pub metadata: Option<AudioTagMetadata>,
}

impl LossyExportSettings {
    pub fn cubase_quick_export() -> Self {
        Self { bit_rate_kbps: 256, high_quality: true, metadata: None }
    }

    pub fn validate(&self, codec: LossyAudioCodec) -> bool {
        let bitrate_valid = match codec {
            LossyAudioCodec::Mp3 => matches!(self.bit_rate_kbps, 32 | 40 | 48 | 56 | 64 | 80 | 96 | 112
                | 128 | 160 | 192 | 224 | 256 | 320),
            LossyAudioCodec::Aac => (32..=512).contains(&self.bit_rate_kbps),
        };
        bitrate_valid && self.metadata.as_ref().is_none_or(AudioTagMetadata::validate)
    }
}

/// Encodes a rendered buffer through FFmpeg without exposing a partial output.
/// The PCM input is written privately, then the encoded file is atomically
/// renamed only after the encoder exits successfully.
pub fn export_interleaved_buffer_to_lossy(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    codec: LossyAudioCodec,
    settings: &LossyExportSettings,
    renderer_connected: bool,
) -> Result<(), WavExportError> {
    if !renderer_connected { return Err(WavExportError::RendererNotConnected); }
    if path.as_os_str().is_empty() || !settings.validate(codec) { return Err(WavExportError::InvalidTask); }
    let channel_limit = if codec == LossyAudioCodec::Mp3 { 2 } else { 8 };
    if channels == 0 || channels > channel_limit { return Err(WavExportError::InvalidChannelCount); }
    let parent = path.parent().ok_or(WavExportError::InvalidPath)?;
    if !parent.is_dir() { return Err(WavExportError::InvalidPath); }
    let nonce = format!("{}-{}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_err(|error| WavExportError::Io(error.to_string()))?.as_nanos());
    let input = parent.join(format!(".aura-lossy-{nonce}.wav"));
    let encoded = parent.join(format!(".aura-lossy-{nonce}.encoded"));
    write_wav_float32(&input, samples, sample_rate, channels)?;
    let _output_lock = match OutputPublicationLock::acquire(path) {
        Ok(lock) => lock,
        Err(error) => { let _ = fs::remove_file(&input); return Err(error); }
    };
    let mut command = std::process::Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]).arg(&input);
    match codec {
        LossyAudioCodec::Mp3 => { command.args(["-codec:a", "libmp3lame"]); }
        LossyAudioCodec::Aac => { command.args(["-codec:a", "aac"]); }
    }
    command.args(["-b:a", &format!("{}k", settings.bit_rate_kbps)]);
    if settings.high_quality {
        match codec {
            LossyAudioCodec::Mp3 => { command.args(["-compression_level", "0"]); }
            LossyAudioCodec::Aac => { command.args(["-aac_coder", "twoloop"]); }
        }
    }
    if let Some(tags) = &settings.metadata {
        for (key, value) in [("title", tags.title.as_str()), ("artist", tags.artist.as_str()),
            ("album", tags.album.as_str()), ("genre", tags.genre.as_str()), ("comment", tags.comment.as_str())] {
            if !value.is_empty() { command.arg("-metadata").arg(format!("{key}={value}")); }
        }
        if let Some(year) = tags.year { command.arg("-metadata").arg(format!("date={year}")); }
        if let Some(track) = tags.track { command.arg("-metadata").arg(format!("track={track}")); }
    }
    match codec {
        LossyAudioCodec::Mp3 => { command.args(["-f", "mp3"]); }
        LossyAudioCodec::Aac => { command.args(["-f", "ipod"]); }
    }
    let result = command.arg(&encoded).status().map_err(|error| WavExportError::Io(error.to_string()))
        .and_then(|status| if status.success() { fs::rename(&encoded, path).map_err(io_error)
            .and_then(|()| sync_parent_directory(path).map_err(io_error)) }
            else { Err(WavExportError::Io("lossy audio encoding failed".into())) });
    let _ = fs::remove_file(&input);
    if result.is_err() { let _ = fs::remove_file(&encoded); }
    result
}

/// Publishes a rendered WAV with an explicit sample encoding. `ieee_float`
/// may only be used with 32-bit output; this prevents a 32-bit PCM request
/// from silently becoming a float file (or vice versa).
pub fn export_interleaved_buffer_to_wav_with_format(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    bit_depth: u32,
    ieee_float: bool,
    renderer_connected: bool,
) -> Result<(), WavExportError> {
    if !renderer_connected {
        return Err(WavExportError::RendererNotConnected);
    }
    if ieee_float {
        if bit_depth != 32 {
            return Err(WavExportError::UnsupportedFormat);
        }
        write_wav_float32(path, samples, sample_rate, channels)
    } else {
        write_wav_pcm(path, samples, sample_rate, channels, bit_depth)
    }
}

/// Explicit WAVE64 export entry point for large-file delivery. Keeping this
/// separate from the legacy WAV API prevents callers from accidentally
/// changing the container format of existing projects.
pub fn export_interleaved_buffer_to_wave64(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    renderer_connected: bool,
) -> Result<(), WavExportError> {
    if !renderer_connected {
        return Err(WavExportError::RendererNotConnected);
    }
    write_wave64_float32(path, samples, sample_rate, channels)
}

fn io_error(error: std::io::Error) -> WavExportError {
    WavExportError::Io(error.to_string())
}

struct OutputPublicationLock {
    path: std::path::PathBuf,
}

impl OutputPublicationLock {
    fn acquire(output: &Path) -> Result<Self, WavExportError> {
        let path = std::path::PathBuf::from(format!("{}.export.lock", output.display()));
        File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                WavExportError::Io(format!("output is already being rendered: {error}"))
            })?;
        Ok(Self { path })
    }
}

impl Drop for OutputPublicationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
