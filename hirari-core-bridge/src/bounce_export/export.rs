use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

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
impl ExportJob {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 256
            && matches!(self.bit_depth, 16 | 24 | 32)
            && (8_000..=384_000).contains(&self.sample_rate)
            && self.lufs_target.is_finite()
            && (-120.0..=0.0).contains(&self.lufs_target)
    }
}
impl ExportJob {
    pub fn with_broadcast_preset(
        name: &str,
        codec: Codec,
        sample_rate: u32,
        bit_depth: u32,
        preset: &str,
    ) -> Option<Self> {
        let lufs_target = broadcast_lufs_target(preset)?;
        let job = Self {
            name: name.trim().to_owned(),
            codec,
            bit_depth,
            sample_rate,
            normalize: true,
            lufs_target,
        };
        job.validate().then_some(job)
    }
}
pub fn broadcast_lufs_target(preset: &str) -> Option<f32> {
    match preset.trim().to_ascii_uppercase().as_str() {
        "EBU_R128" | "EBU R128" => Some(-23.0),
        "ATSC_A85" | "ATSC A/85" => Some(-24.0),
        "STREAMING" => Some(-14.0),
        "CD" => Some(-9.0),
        _ => None,
    }
}

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

#[path = "export_writers.rs"]
mod writers;
pub use writers::*;
