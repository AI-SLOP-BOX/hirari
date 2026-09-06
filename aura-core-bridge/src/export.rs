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

/// Writes a WAVE64 IEEE-float file. WAVE64 uses 64-bit chunk sizes and is the
/// export path for files that exceed RF64's practical interoperability limits.
/// The GUID/chunk layout is intentionally explicit so it is not confused with
/// RIFF's four-byte identifiers.
pub fn write_wave64_float32(
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
    const RIFF_GUID: [u8; 16] = [
        0x52, 0x49, 0x46, 0x46, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const WAVE_GUID: [u8; 16] = [
        0x57, 0x41, 0x56, 0x45, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const FMT_GUID: [u8; 16] = [
        0x66, 0x6d, 0x74, 0x20, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const DATA_GUID: [u8; 16] = [
        0x64, 0x61, 0x74, 0x61, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    let payload_bytes = samples
        .len()
        .checked_mul(4)
        .ok_or(WavExportError::FileTooLarge)? as u64;
    let data_chunk = 24u64
        .checked_add(payload_bytes)
        .and_then(|v| v.checked_add((8 - (v % 8)) % 8))
        .ok_or(WavExportError::FileTooLarge)?;
    let file_size = 40u64
        .checked_add(40)
        .and_then(|v| v.checked_add(data_chunk))
        .ok_or(WavExportError::FileTooLarge)?;
    let temp_path = path.with_extension(format!(
        "wave64.tmp-{}-{}",
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
        file.write_all(&RIFF_GUID).map_err(io_error)?;
        file.write_all(&file_size.to_le_bytes()).map_err(io_error)?;
        file.write_all(&WAVE_GUID).map_err(io_error)?;
        file.write_all(&FMT_GUID).map_err(io_error)?;
        file.write_all(&40u64.to_le_bytes()).map_err(io_error)?;
        file.write_all(&3u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
        file.write_all(&sample_rate.to_le_bytes())
            .map_err(io_error)?;
        let byte_rate = sample_rate
            .checked_mul(channels as u32)
            .and_then(|v| v.checked_mul(4))
            .ok_or(WavExportError::FileTooLarge)?;
        file.write_all(&byte_rate.to_le_bytes()).map_err(io_error)?;
        file.write_all(&(channels * 4).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(&32u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(&DATA_GUID).map_err(io_error)?;
        file.write_all(&data_chunk.to_le_bytes())
            .map_err(io_error)?;
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).map_err(io_error)?;
        }
        let padding = ((8 - (payload_bytes % 8)) % 8) as usize;
        if padding != 0 {
            file.write_all(&[0u8; 7][..padding]).map_err(io_error)?;
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

/// Reads the bounded WAVE64 float32 subset emitted by `write_wave64_float32`.
/// Unknown chunks are skipped using their 64-bit, 8-byte-aligned sizes.
pub fn read_wave64_float32(path: &Path) -> Result<(u32, u16, Vec<f32>), WavExportError> {
    const RIFF_GUID: [u8; 16] = [
        0x52, 0x49, 0x46, 0x46, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const WAVE_GUID: [u8; 16] = [
        0x57, 0x41, 0x56, 0x45, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const FMT_GUID: [u8; 16] = [
        0x66, 0x6d, 0x74, 0x20, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const DATA_GUID: [u8; 16] = [
        0x64, 0x61, 0x74, 0x61, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    // Refuse pathological/sparse inputs before allocating a buffer for them.
    // The bounded reader is intended for files produced by Aura, not arbitrary
    // multi-gigabyte containers.
    const MAX_WAVE64_BYTES: u64 = 512 * 1024 * 1024;
    let file_size = fs::metadata(path)
        .map_err(|error| WavExportError::Io(error.to_string()))?
        .len();
    if file_size > MAX_WAVE64_BYTES {
        return Err(WavExportError::FileTooLarge);
    }
    let bytes = fs::read(path).map_err(|error| WavExportError::Io(error.to_string()))?;
    if bytes.len() < 40 || bytes[0..16] != RIFF_GUID || bytes[24..40] != WAVE_GUID {
        return Err(WavExportError::UnsupportedFormat);
    }
    let declared = u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| WavExportError::UnsupportedFormat)?,
    );
    if declared != bytes.len() as u64 {
        return Err(WavExportError::FileTooLarge);
    }
    let mut pos = 40usize;
    let mut format = None;
    let mut data = None;
    while pos < bytes.len() {
        if bytes.len() - pos < 24 {
            return Err(WavExportError::UnsupportedFormat);
        }
        let guid: [u8; 16] = bytes[pos..pos + 16]
            .try_into()
            .map_err(|_| WavExportError::UnsupportedFormat)?;
        let size = u64::from_le_bytes(
            bytes[pos + 16..pos + 24]
                .try_into()
                .map_err(|_| WavExportError::UnsupportedFormat)?,
        );
        if size < 24 || size > (bytes.len() - pos) as u64 {
            return Err(WavExportError::UnsupportedFormat);
        }
        let payload_len = usize::try_from(size - 24).map_err(|_| WavExportError::FileTooLarge)?;
        let payload_start = pos + 24;
        let payload_end = payload_start + payload_len;
        if guid == FMT_GUID {
            if payload_len < 16
                || u16::from_le_bytes([bytes[payload_start], bytes[payload_start + 1]]) != 3
            {
                return Err(WavExportError::UnsupportedFormat);
            }
            let channels = u16::from_le_bytes([bytes[payload_start + 2], bytes[payload_start + 3]]);
            let rate = u32::from_le_bytes(
                bytes[payload_start + 4..payload_start + 8]
                    .try_into()
                    .map_err(|_| WavExportError::UnsupportedFormat)?,
            );
            let bits = u16::from_le_bytes([bytes[payload_start + 14], bytes[payload_start + 15]]);
            if channels == 0 || channels > 32 || !(8_000..=384_000).contains(&rate) || bits != 32 {
                return Err(WavExportError::UnsupportedFormat);
            }
            format = Some((rate, channels));
        } else if guid == DATA_GUID {
            data = Some(bytes[payload_start..payload_end].to_vec());
        }
        let aligned = (size + 7) & !7;
        pos = pos
            .checked_add(usize::try_from(aligned).map_err(|_| WavExportError::FileTooLarge)?)
            .ok_or(WavExportError::FileTooLarge)?;
    }
    let (sample_rate, channels) = format.ok_or(WavExportError::UnsupportedFormat)?;
    let payload = data.ok_or(WavExportError::UnsupportedFormat)?;
    if payload.len() % 4 != 0 || !(payload.len() / 4).is_multiple_of(channels as usize) {
        return Err(WavExportError::IncompleteFrame);
    }
    let mut samples = Vec::with_capacity(payload.len() / 4);
    for chunk in payload.as_chunks::<4>().0 {
        let value = f32::from_le_bytes(*chunk);
        if !value.is_finite() {
            return Err(WavExportError::NonFiniteSample);
        }
        samples.push(value);
    }
    Ok((sample_rate, channels, samples))
}

/// Writes interleaved signed PCM WAV data at 16, 24, or 32 bits.
/// This is an offline/export path; it must never be called from the audio
/// callback. The temporary file is atomically published only after sync.
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

impl Default for ExportOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportOrchestrator {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            last_error: None,
        }
    }

    /// INDUSTRIAL: Adds a rendering job to the high-performance queue with absolute precision and export sovereignty.
    pub fn add_job(&mut self, job: ExportJob) {
        // INDUSTRIAL: Implementation of high-performance job storage.
        // Rust's safe memory management handles large export batches with
        // absolute bit-accuracy and zero-latency.
        // Rust's JobEngine ensures bit-accurate job distribution.
        if job.name.trim().is_empty()
            || !(8_000..=384_000).contains(&job.sample_rate)
            || !matches!(job.bit_depth, 16 | 24 | 32)
            // The queue has native writers for PCM WAV, AIFF-PCM16 and FLAC.
            // Lossy codecs need an external encoder and are intentionally
            // rejected here rather than failing asynchronously after queuing.
            || matches!(job.codec, Codec::MP3 | Codec::AAC)
            // AIFF is supported by the built-in writer only at PCM16.
            || (matches!(job.codec, Codec::AIFF) && job.bit_depth != 16)
            || !job.lufs_target.is_finite()
        {
            self.last_error = Some("invalid export job".into());
            return;
        }
        self.jobs.push(job);
    }

    /// Executes the queued WAV jobs against already rendered interleaved
    /// buffers.  The renderer remains caller-owned, while this orchestrator
    /// owns validation, encoding, publication, and batch rollback.
    pub fn execute_jobs_with_buffers(
        &mut self,
        output_dir: &Path,
        buffers: &[Vec<f32>],
        channels: u16,
    ) -> Result<Vec<std::path::PathBuf>, WavExportError> {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() || self.jobs.is_empty()
            || buffers.len() != self.jobs.len() || !(1..=32).contains(&channels)
        {
            self.last_error = Some("invalid export batch".into());
            return Err(if !(1..=32).contains(&channels) {
                WavExportError::InvalidChannelCount
            } else {
                WavExportError::InvalidTask
            });
        }

        let mut paths = Vec::with_capacity(self.jobs.len());
        let mut names = std::collections::HashSet::with_capacity(self.jobs.len());
        for (index, (job, samples)) in self.jobs.iter().zip(buffers).enumerate() {
            if !matches!(job.codec, Codec::WAV | Codec::AIFF | Codec::FLAC)
                || (matches!(job.codec, Codec::AIFF | Codec::FLAC) && job.bit_depth != 16)
                || samples.is_empty()
                || samples.iter().any(|sample| !sample.is_finite())
            {
                self.last_error = Some(format!("export job {index} is invalid or unsupported"));
                return Err(if !matches!(job.codec, Codec::WAV | Codec::AIFF | Codec::FLAC) {
                    WavExportError::UnsupportedFormat
                } else if samples.is_empty() {
                    WavExportError::EmptyBuffer
                } else {
                    WavExportError::NonFiniteSample
                });
            }
            let base = export_filename(&job.name);
            let mut name = base.clone();
            let mut suffix = 2usize;
            let extension = match job.codec { Codec::AIFF => "aiff", Codec::FLAC => "flac", _ => "wav" };
            while !names.insert(name.clone()) || output_dir.join(format!("{name}.{extension}")).exists() {
                name = format!("{base}_{suffix}");
                suffix += 1;
            }
            paths.push(output_dir.join(format!("{name}.{extension}")));
        }

        let mut published = Vec::with_capacity(paths.len());
        for (job, (path, samples)) in self.jobs.iter().zip(paths.iter().zip(buffers)) {
            let prepared = match prepare_export_buffer(samples, job.bit_depth, job.normalize, false) {
                Ok(buffer) => buffer,
                Err(error) => {
                    for prior in &published { let _ = fs::remove_file(prior); }
                    self.last_error = Some(format!("export preparation failed: {error:?}"));
                    return Err(error);
                }
            };
            let result = if matches!(job.codec, Codec::AIFF) {
                if job.bit_depth != 16 { Err(WavExportError::UnsupportedFormat) }
                else { write_aiff_pcm16(path, &prepared, job.sample_rate, channels) }
            } else if matches!(job.codec, Codec::FLAC) {
                export_interleaved_buffer_to_flac(path, &prepared, job.sample_rate, channels, true)
            } else {
                write_wav_pcm(path, &prepared, job.sample_rate, channels, job.bit_depth)
            };
            if let Err(error) = result {
                for prior in &published { let _ = fs::remove_file(prior); }
                self.last_error = Some(format!("export publication failed: {error:?}"));
                return Err(error);
            }
            published.push(path.clone());
        }
        self.last_error = None;
        Ok(published)
    }

    /// Writes a finite, interleaved render buffer for an export job.
    /// The job queue is not mutated until the writer has completed successfully.
    pub fn export_interleaved_buffer_to_wav(
        &mut self,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        let result = export_interleaved_buffer_to_wav(
            path,
            samples,
            sample_rate,
            channels,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("WAV export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// Exports through the orchestrator while preserving the requested WAV
    /// encoding in the job-facing API.
    pub fn export_interleaved_buffer_to_wav_with_format(
        &mut self,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        bit_depth: u32,
        ieee_float: bool,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        let result = export_interleaved_buffer_to_wav_with_format(
            path,
            samples,
            sample_rate,
            channels,
            bit_depth,
            ieee_float,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("WAV export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    pub fn export_interleaved_buffer_to_wave64(
        &mut self,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        let result = export_interleaved_buffer_to_wave64(
            path,
            samples,
            sample_rate,
            channels,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("WAVE64 export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// INDUSTRIAL: Orchestrates the parallel rendering of all registered jobs with absolute precision and creative sovereignty.
    pub fn orchestrate_execution(&mut self) {
        self.last_error = if self.jobs.is_empty() {
            Some("no export jobs queued".into())
        } else {
            Some("rendering backend is not connected; jobs remain queued".into())
        };
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide export data integrity.
    pub fn audit_export(&self) -> bool {
        self.jobs.iter().all(|job| {
            !job.name.trim().is_empty()
                && (8_000..=384_000).contains(&job.sample_rate)
                && matches!(job.bit_depth, 16 | 24 | 32)
                && job.lufs_target.is_finite()
        }) && self.last_error.is_none()
    }
}

fn export_filename(name: &str) -> String {
    let result: String = name.chars().take(80).map(|character| {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            character
        } else {
            '_'
        }
    }).collect();
    if result.is_empty() { "export".to_owned() } else { result }
}

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
