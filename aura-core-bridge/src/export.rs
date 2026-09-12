use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Codec {
    WAV,
    AIFF,
    FLAC,
    MP3,
    AAC,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExportJob {
    pub name: String,
    pub codec: Codec,
    pub bit_depth: u32,
    pub sample_rate: u32,
    pub normalize: bool,
    pub lufs_target: f32, // INDUSTRIAL: Added loudness target for professional delivery.
}
impl ExportJob { pub fn validate(&self) -> bool { !self.name.trim().is_empty() && self.name.len() <= 256 && matches!(self.bit_depth, 16|24|32) && (8_000..=384_000).contains(&self.sample_rate) && self.lufs_target.is_finite() && (-120.0..=0.0).contains(&self.lufs_target) } }
impl ExportJob { pub fn with_broadcast_preset(name: &str, codec: Codec, sample_rate: u32, bit_depth: u32, preset: &str) -> Option<Self> { let lufs_target = broadcast_lufs_target(preset)?; let job = Self { name: name.trim().to_owned(), codec, bit_depth, sample_rate, normalize: true, lufs_target }; job.validate().then_some(job) } }
pub fn broadcast_lufs_target(preset: &str) -> Option<f32> { match preset.trim().to_ascii_uppercase().as_str() { "EBU_R128" | "EBU R128" => Some(-23.0), "ATSC_A85" | "ATSC A/85" => Some(-24.0), "STREAMING" => Some(-14.0), "CD" => Some(-9.0), _ => None } }

pub struct ExportOrchestrator {
    pub jobs: Vec<ExportJob>,
    pub last_error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WavExportError {
    EmptyBuffer,
    InvalidTask,
    InvalidSampleRate,
    InvalidChannelCount,
    IncompleteFrame,
    NonFiniteSample,
    RendererNotConnected,
    InvalidPath,
    FileTooLarge,
    UnsupportedFormat,
    Io(String),
}

impl WavExportError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptyBuffer => "audio.empty_buffer",
            Self::InvalidTask => "render.invalid_task",
            Self::InvalidSampleRate => "audio.invalid_sample_rate",
            Self::InvalidChannelCount => "audio.invalid_channel_count",
            Self::IncompleteFrame => "audio.incomplete_frame",
            Self::NonFiniteSample => "audio.non_finite_sample",
            Self::RendererNotConnected => "render.renderer_not_connected",
            Self::InvalidPath => "io.invalid_path",
            Self::FileTooLarge => "io.file_too_large",
            Self::UnsupportedFormat => "audio.unsupported_format",
            Self::Io(_) => "io.filesystem",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::RendererNotConnected | Self::Io(_))
    }

    pub fn bridge_error(&self) -> crate::bridge_error::BridgeError {
        crate::bridge_error::BridgeError::new(self.code(), self.code()).retryable(self.retryable())
    }
}

/// Writes interleaved f32 samples as a PCM16 WAV using an atomic replacement.
/// The conversion is deliberately separate from the real-time graph: callers
/// should invoke it from an export worker, never from the audio callback.
pub fn write_wav_pcm16(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), WavExportError> {
    write_wav_pcm(path, samples, sample_rate, channels, 16)
}

/// Writes IEEE-754 32-bit float WAV data. This is distinct from the 32-bit
/// integer PCM writer so the file header and payload agree with the export
/// format advertised by the UI.
pub fn write_wav_float32(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), WavExportError> {
    if path.as_os_str().is_empty() || samples.is_empty() {
        return Err(if samples.is_empty() {
            WavExportError::EmptyBuffer
        } else {
            WavExportError::InvalidPath
        });
    }
    if !(8_000..=384_000).contains(&sample_rate) {
        return Err(WavExportError::InvalidSampleRate);
    }
    if !(1..=32).contains(&channels) {
        return Err(WavExportError::InvalidChannelCount);
    }
    if !samples.len().is_multiple_of(channels as usize) {
        return Err(WavExportError::IncompleteFrame);
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err(WavExportError::NonFiniteSample);
    }
    let _output_lock = OutputPublicationLock::acquire(path)?;
    let data_bytes = samples
        .len()
        .checked_mul(4)
        .ok_or(WavExportError::FileTooLarge)?;
    let riff_size = 36usize
        .checked_add(data_bytes)
        .ok_or(WavExportError::FileTooLarge)?;
    let rf64 = data_bytes > u32::MAX as usize || riff_size > u32::MAX as usize;
    let riff_size64 = 72u64
        .checked_add(data_bytes as u64)
        .ok_or(WavExportError::FileTooLarge)?;
    let temp_path = path.with_extension(format!(
        "wav.tmp-f32-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| WavExportError::Io(error.to_string()))?
            .as_nanos()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| WavExportError::Io(error.to_string()))?;
        file.write_all(if rf64 { b"RF64" } else { b"RIFF" })
            .map_err(io_error)?;
        file.write_all(&(if rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(b"WAVE").map_err(io_error)?;
        if rf64 {
            // RF64 ds64 sizes are known before writing because the export
            // buffer is immutable. This avoids a seek/patch race on publish.
            file.write_all(b"ds64").map_err(io_error)?;
            file.write_all(&28u32.to_le_bytes()).map_err(io_error)?;
            file.write_all(&riff_size64.to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&(data_bytes as u64).to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&((samples.len() / channels as usize) as u64).to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
        }
        file.write_all(b"fmt ").map_err(io_error)?;
        file.write_all(&16u32.to_le_bytes()).map_err(io_error)?;
        file.write_all(&3u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
        file.write_all(&sample_rate.to_le_bytes())
            .map_err(io_error)?;
        let byte_rate = sample_rate
            .checked_mul(channels as u32)
            .and_then(|value| value.checked_mul(4))
            .ok_or(WavExportError::FileTooLarge)?;
        file.write_all(&byte_rate.to_le_bytes()).map_err(io_error)?;
        file.write_all(&(channels * 4).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(&32u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(b"data").map_err(io_error)?;
        file.write_all(&(if rf64 { u32::MAX } else { data_bytes as u32 }).to_le_bytes())
            .map_err(io_error)?;
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).map_err(io_error)?;
        }
        file.sync_all().map_err(io_error)?;
        fs::rename(&temp_path, path)
            .map_err(io_error)
            .and_then(|()| sync_parent_directory(path).map_err(io_error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

include!("export_wave64.rs");
pub fn write_wav_pcm(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    bit_depth: u32,
) -> Result<(), WavExportError> {
    if path.as_os_str().is_empty() {
        return Err(WavExportError::InvalidPath);
    }
    if samples.is_empty() {
        return Err(WavExportError::EmptyBuffer);
    }
    if !(8_000..=384_000).contains(&sample_rate) {
        return Err(WavExportError::InvalidSampleRate);
    }
    if !(1..=32).contains(&channels) {
        return Err(WavExportError::InvalidChannelCount);
    }
    if !matches!(bit_depth, 16 | 24 | 32) {
        return Err(WavExportError::UnsupportedFormat);
    }
    if !samples.len().is_multiple_of(channels as usize) {
        return Err(WavExportError::IncompleteFrame);
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err(WavExportError::NonFiniteSample);
    }
    let _output_lock = OutputPublicationLock::acquire(path)?;

    let frame_count = samples.len() / channels as usize;
    let bytes_per_sample = (bit_depth / 8) as usize;
    let data_bytes = samples
        .len()
        .checked_mul(bytes_per_sample)
        .ok_or(WavExportError::FileTooLarge)?;
    let riff_size = 36usize
        .checked_add(data_bytes)
        .ok_or(WavExportError::FileTooLarge)?;
    let rf64 = data_bytes > u32::MAX as usize
        || riff_size > u32::MAX as usize
        || frame_count > u32::MAX as usize;
    let riff_size64 = 72u64
        .checked_add(data_bytes as u64)
        .ok_or(WavExportError::FileTooLarge)?;

    let temp_path = path.with_extension(format!(
        "wav.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| WavExportError::Io(error.to_string()))?
            .as_nanos()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| WavExportError::Io(error.to_string()))?;
        file.write_all(if rf64 { b"RF64" } else { b"RIFF" })
            .map_err(io_error)?;
        file.write_all(&(if rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(b"WAVE").map_err(io_error)?;
        if rf64 {
            file.write_all(b"ds64").map_err(io_error)?;
            file.write_all(&28u32.to_le_bytes()).map_err(io_error)?;
            file.write_all(&riff_size64.to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&(data_bytes as u64).to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&(frame_count as u64).to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
        }
        file.write_all(b"fmt ").map_err(io_error)?;
        file.write_all(&16u32.to_le_bytes()).map_err(io_error)?;
        file.write_all(&1u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
        file.write_all(&sample_rate.to_le_bytes())
            .map_err(io_error)?;
        let byte_rate = sample_rate
            .checked_mul(channels as u32)
            .and_then(|value| value.checked_mul(bytes_per_sample as u32))
            .ok_or(WavExportError::FileTooLarge)?;
        file.write_all(&byte_rate.to_le_bytes()).map_err(io_error)?;
        file.write_all(&(channels * bytes_per_sample as u16).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(&(bit_depth as u16).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(b"data").map_err(io_error)?;
        file.write_all(&(if rf64 { u32::MAX } else { data_bytes as u32 }).to_le_bytes())
            .map_err(io_error)?;
        for sample in samples {
            let value = sample.clamp(-1.0, 1.0);
            match bit_depth {
                16 => {
                    let scaled = (value * 32767.0).round() as i16;
                    file.write_all(&scaled.to_le_bytes()).map_err(io_error)?;
                }
                24 => {
                    let scaled = (value * 8_388_607.0).round() as i32;
                    let bytes = scaled.to_le_bytes();
                    file.write_all(&bytes[..3]).map_err(io_error)?;
                }
                32 => {
                    let scaled = (value * 2_147_483_647.0).round() as i32;
                    file.write_all(&scaled.to_le_bytes()).map_err(io_error)?;
                }
                _ => unreachable!("bit depth validated above"),
            }
        }
        file.sync_all().map_err(io_error)?;
        fs::rename(&temp_path, path)
            .map_err(io_error)
            .and_then(|()| sync_parent_directory(path).map_err(io_error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// Writes interleaved PCM16 samples as a standards-compliant big-endian AIFF.
/// AIFF is kept as a separate writer so WAV/RF64 header invariants cannot be
/// accidentally mixed with the big-endian IFF container.
pub fn write_aiff_pcm16(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), WavExportError> {
    write_aiff_pcm(path, samples, sample_rate, channels, 16)
}

/// Writes signed big-endian AIFF PCM at 8, 16, 24, or 32 bits.
pub fn write_aiff_pcm(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    bit_depth: u32,
) -> Result<(), WavExportError> {
    if path.as_os_str().is_empty() { return Err(WavExportError::InvalidPath); }
    if samples.is_empty() { return Err(WavExportError::EmptyBuffer); }
    if !(8_000..=384_000).contains(&sample_rate) { return Err(WavExportError::InvalidSampleRate); }
    if !(1..=32).contains(&channels) { return Err(WavExportError::InvalidChannelCount); }
    if !matches!(bit_depth, 8 | 16 | 24 | 32) { return Err(WavExportError::UnsupportedFormat); }
    if !samples.len().is_multiple_of(channels as usize) { return Err(WavExportError::IncompleteFrame); }
    if samples.iter().any(|sample| !sample.is_finite()) { return Err(WavExportError::NonFiniteSample); }
    let bytes_per_sample = (bit_depth / 8) as usize;
    let data_bytes = samples.len().checked_mul(bytes_per_sample).ok_or(WavExportError::FileTooLarge)?;
    let padded_data = data_bytes.checked_add(data_bytes % 2).ok_or(WavExportError::FileTooLarge)?;
    let form_size = 4usize.checked_add(8 + 18).and_then(|v| v.checked_add(8 + padded_data)).ok_or(WavExportError::FileTooLarge)?;
    if form_size > u32::MAX as usize { return Err(WavExportError::FileTooLarge); }
    let _output_lock = OutputPublicationLock::acquire(path)?;
    let temp_path = path.with_extension(format!("aiff.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&temp_path).map_err(io_error)?;
        file.write_all(b"FORM").map_err(io_error)?; file.write_all(&(form_size as u32).to_be_bytes()).map_err(io_error)?; file.write_all(b"AIFF").map_err(io_error)?;
        file.write_all(b"COMM").map_err(io_error)?; file.write_all(&18u32.to_be_bytes()).map_err(io_error)?;
        file.write_all(&channels.to_be_bytes()).map_err(io_error)?; file.write_all(&((samples.len() / channels as usize) as u32).to_be_bytes()).map_err(io_error)?; file.write_all(&(bit_depth as u16).to_be_bytes()).map_err(io_error)?;
        let exponent = (sample_rate as f64).log2().floor() as i32;
        let mantissa = ((sample_rate as f64 / 2f64.powi(exponent)) * (1u64 << 63) as f64) as u64;
        file.write_all(&((exponent + 16383) as u16).to_be_bytes()).map_err(io_error)?;
        file.write_all(&mantissa.to_be_bytes()).map_err(io_error)?;
        file.write_all(b"SSND").map_err(io_error)?; file.write_all(&((8 + padded_data) as u32).to_be_bytes()).map_err(io_error)?; file.write_all(&0u32.to_be_bytes()).map_err(io_error)?; file.write_all(&0u32.to_be_bytes()).map_err(io_error)?;
        for sample in samples {
            let value = sample.clamp(-1.0, 1.0);
            match bit_depth {
                8 => {
                    let encoded = (value * 127.0).round().clamp(-128.0, 127.0) as i8;
                    file.write_all(&[encoded as u8]).map_err(io_error)?;
                }
                16 => {
                    let encoded = (value * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16;
                    file.write_all(&encoded.to_be_bytes()).map_err(io_error)?;
                }
                24 => {
                    let encoded = (value * 8_388_607.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                    file.write_all(&encoded.to_be_bytes()[1..]).map_err(io_error)?;
                }
                32 => {
                    let encoded = (value as f64 * 2_147_483_647.0).round()
                        .clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                    file.write_all(&encoded.to_be_bytes()).map_err(io_error)?;
                }
                _ => unreachable!("AIFF bit depth validated above"),
            }
        }
        if data_bytes % 2 != 0 { file.write_all(&[0]).map_err(io_error)?; }
        file.sync_all().map_err(io_error)?; fs::rename(&temp_path, path).map_err(io_error).and_then(|()| sync_parent_directory(path).map_err(io_error))
    })();
    if result.is_err() { let _ = fs::remove_file(&temp_path); }
    result
}

include!("export_render.rs");

include!("export_lossy.rs");

include!("export_orchestrator.rs");

#[cfg(test)]
mod tests {
    include!("export_tests.rs");

    #[test]
    fn export_errors_have_stable_codes_and_retry_policy() {
        assert_eq!(WavExportError::InvalidPath.code(), "io.invalid_path");
        assert!(!WavExportError::InvalidPath.retryable());
        assert!(WavExportError::RendererNotConnected.retryable());
        assert_eq!(WavExportError::FileTooLarge.code(), "io.file_too_large");
        let bridge = WavExportError::RendererNotConnected.bridge_error();
        assert_eq!(bridge.code, "render.renderer_not_connected");
        assert!(bridge.retryable);
    }

    #[test]
    fn lossy_settings_validate_codec_specific_bitrates_and_tags() {
        let mut settings = LossyExportSettings::cubase_quick_export();
        settings.metadata = Some(AudioTagMetadata { title: "Mix".into(), artist: "Artist".into(),
            album: "Album".into(), year: Some(2026), track: Some(1), genre: "Electronic".into(),
            comment: "Approval master".into() });
        assert!(settings.validate(LossyAudioCodec::Mp3));
        assert!(settings.validate(LossyAudioCodec::Aac));
        settings.bit_rate_kbps = 127;
        assert!(!settings.validate(LossyAudioCodec::Mp3));
        assert!(settings.validate(LossyAudioCodec::Aac));
    }

    #[test]
    fn ffmpeg_publishes_mp3_and_aac_without_temporary_files() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() { return; }
        let root = std::env::temp_dir().join(format!("aura-lossy-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let samples: Vec<f32> = (0..4_800).flat_map(|index| {
            let value = ((index as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU).sin() * 0.1;
            [value, value]
        }).collect();
        let settings = LossyExportSettings::cubase_quick_export();
        let mp3 = root.join("mix.mp3");
        let aac = root.join("mix.m4a");
        export_interleaved_buffer_to_lossy(&mp3, &samples, 48_000, 2, LossyAudioCodec::Mp3,
            &settings, true).unwrap();
        export_interleaved_buffer_to_lossy(&aac, &samples, 48_000, 2, LossyAudioCodec::Aac,
            &settings, true).unwrap();
        assert!(std::fs::metadata(&mp3).unwrap().len() > 100);
        let aac_bytes = std::fs::read(&aac).unwrap();
        assert_eq!(&aac_bytes[4..8], b"ftyp");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn aiff_writer_emits_big_endian_pcm_for_each_supported_depth() {
        let root = std::env::temp_dir().join(format!("aura-aiff-depths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for depth in [8_u32, 16, 24, 32] {
            let path = root.join(format!("depth-{depth}.aiff"));
            write_aiff_pcm(&path, &[-1.0, 0.0, 1.0], 48_000, 1, depth).unwrap();
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(&bytes[..4], b"FORM");
            assert_eq!(&bytes[8..12], b"AIFF");
            assert_eq!(&bytes[12..16], b"COMM");
            assert_eq!(u16::from_be_bytes([bytes[26], bytes[27]]), depth as u16);
            assert_eq!(&bytes[38..42], b"SSND");
            let data_bytes = 3 * (depth as usize / 8);
            assert_eq!(bytes.len(), 54 + data_bytes + data_bytes % 2);
            let first = &bytes[54..54 + depth as usize / 8];
            assert!(first.iter().any(|byte| *byte != 0));
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
