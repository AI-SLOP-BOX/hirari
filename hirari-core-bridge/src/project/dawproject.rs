//! DAWproject 1.0 import/export for exchanging a Hirari arrangement with other DAWs.
//! Audio is embedded; tracks, audio clips, MIDI notes, tempo, meter, and
//! arrangement markers are represented in the portable project XML. The
//! import/export families and the XML/JSON primitives were split into
//! sibling modules; see dawproject_import.rs, dawproject_export.rs and
//! dawproject_parse.rs.

use crate::persistence::ProjectMetadata;
use crate::project::{ProjectDocument, ProjectRegion, ProjectTrack};
use crate::xml_attribute::escape_xml;
use crate::project_contracts::{
    AutomationPointContract, MarkerContract, MidiNoteContract, TempoEventContract,
    TimeSignatureEventContract, WarpMarkerContract,
};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MAX_MEDIA_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL_MEDIA_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DawProjectExportReport {
    pub track_count: usize,
    pub audio_clip_count: usize,
    pub midi_note_count: usize,
    pub embedded_media_count: usize,
    pub unsupported_plugin_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DawProjectImportReport {
    pub track_count: usize,
    pub audio_clip_count: usize,
    pub midi_note_count: usize,
    pub embedded_media_count: usize,
    pub unsupported_plugin_count: usize,
}

fn resolve_region_path(source_root: &Path, region: &ProjectRegion) -> Result<PathBuf> {
    let path = Path::new(&region.path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_root.join(path)
    };
    if !resolved.is_file() {
        bail!("audio media is missing for region {}", region.id);
    }
    supported_audio_extension(resolved.to_string_lossy().as_ref())
        .context("DAWproject audio format is unsupported")?;
    Ok(resolved)
}

fn supported_audio_extension(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?;
    match extension.to_ascii_lowercase().as_str() {
        "wav" | "rf64" | "aif" | "aiff" | "flac" | "mp3" | "ogg" | "aac" | "m4a" => {
            // Return a stable lowercase extension so extracted assets keep a
            // decoder-recognized suffix on every platform.
            Some(match extension.to_ascii_lowercase().as_str() {
                "aif" => "aif",
                "aiff" => "aiff",
                "rf64" => "rf64",
                "flac" => "flac",
                "mp3" => "mp3",
                "ogg" => "ogg",
                "aac" => "aac",
                "m4a" => "m4a",
                _ => "wav",
            })
        }
        _ => None,
    }
}

/// Read the authoritative media properties through Symphonia so DAWproject
/// interchange can preserve common compressed and lossless source formats.
/// WAV falls back to Hirari's bounded parser for variants Symphonia rejects.
fn read_audio_info(path: &Path) -> Result<(u16, u32, u64)> {
    let decoded = (|| -> Result<(u16, u32, u64)> {
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::errors::Error as SymphoniaError;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let file =
            File::open(path).with_context(|| format!("could not read {}", path.display()))?;
        let source = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .context("unsupported or invalid audio file")?;
        let mut format = probed.format;
        let track = format
            .default_track()
            .context("audio file has no default track")?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let sample_rate = params
            .sample_rate
            .context("audio file has no sample rate")?;
        let channels = params
            .channels
            .context("audio file has no channel layout")?
            .count();
        if !(1..=384_000).contains(&sample_rate) || !(1..=32).contains(&channels) {
            bail!("audio sample rate or channel count is unsupported");
        }
        let mut decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .context("audio codec is unsupported")?;
        let mut frames = 0u64;
        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break
                }
                Err(SymphoniaError::ResetRequired) => {
                    bail!("chained audio streams are unsupported in DAWproject media")
                }
                Err(error) => return Err(error).context("audio packet read failed"),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = decoder
                .decode(&packet)
                .context("audio packet decode failed")?;
            frames = frames
                .checked_add(decoded.frames() as u64)
                .context("audio frame count overflow")?;
        }
        Ok((channels as u16, sample_rate, frames))
    })();
    match decoded {
        Ok(info) => Ok(info),
        Err(error) if supported_audio_extension(path.to_string_lossy().as_ref()) == Some("wav") => {
            read_wav_info(path).map_err(|fallback| {
                anyhow::anyhow!("{error}; WAV metadata fallback failed: {fallback}")
            })
        }
        Err(error) => Err(error),
    }
}

fn samples_to_beats(
    samples: u64,
    sample_rate: f64,
    events: &[crate::project_contracts::TempoEventContract],
    base_bpm: f64,
) -> Result<f64> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 || !base_bpm.is_finite() || base_bpm <= 0.0 {
        bail!("project sample rate or base tempo is invalid");
    }
    let mut ordered = events.to_vec();
    ordered.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    if ordered.is_empty() || ordered.first().is_some_and(|event| event.beat > 0.0) {
        ordered.insert(
            0,
            crate::project_contracts::TempoEventContract {
                beat: 0.0,
                bpm: base_bpm,
                ramp: false,
            },
        );
    }
    let mut elapsed_samples = 0.0f64;
    for index in 0..ordered.len() {
        let current = &ordered[index];
        let Some(next) = ordered.get(index + 1) else {
            let seconds = (samples as f64 - elapsed_samples).max(0.0) / sample_rate;
            return Ok(current.beat + seconds * current.bpm / 60.0);
        };
        let beat_span = next.beat - current.beat;
        if beat_span < 0.0 || !current.bpm.is_finite() || current.bpm <= 0.0 {
            bail!("project tempo map is invalid");
        }
        let duration_seconds = if current.ramp {
            120.0 * beat_span / (current.bpm + next.bpm)
        } else {
            60.0 * beat_span / current.bpm
        };
        let segment_samples = duration_seconds * sample_rate;
        if samples as f64 <= elapsed_samples + segment_samples {
            let offset_seconds = (samples as f64 - elapsed_samples).max(0.0) / sample_rate;
            let beat_offset = if current.ramp && duration_seconds > 0.0 {
                let alpha = (next.bpm - current.bpm) / duration_seconds;
                if alpha.abs() < 1e-12 {
                    offset_seconds * current.bpm / 60.0
                } else {
                    (current.bpm * offset_seconds + 0.5 * alpha * offset_seconds.powi(2)) / 60.0
                }
            } else {
                offset_seconds * current.bpm / 60.0
            };
            return Ok(current.beat + beat_offset.clamp(0.0, beat_span));
        }
        elapsed_samples += segment_samples;
    }
    bail!("could not convert sample position to beats")
}

fn normalized_pan(pan: f32) -> f32 {
    ((pan.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0)
}

fn read_wav_info(path: &Path) -> Result<(u16, u32, u64)> {
    let mut file =
        File::open(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header)
        .context("WAV header is truncated")?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        bail!("DAWproject export currently requires RIFF WAV audio");
    }
    let (mut channels, mut sample_rate, mut block_align, mut data_bytes) = (None, None, None, None);
    loop {
        let mut chunk = [0u8; 8];
        match file.read_exact(&mut chunk) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error).context("could not inspect WAV chunks"),
        }
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;
        match &chunk[0..4] {
            b"fmt " if size >= 16 && size <= 4096 => {
                let mut fmt = vec![0; size as usize];
                file.read_exact(&mut fmt)
                    .context("WAV format chunk is truncated")?;
                let format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                if !matches!(format, 1 | 3 | 0xfffe) {
                    bail!("WAV encoding is unsupported for DAWproject export");
                }
                channels = Some(u16::from_le_bytes(fmt[2..4].try_into().unwrap()));
                sample_rate = Some(u32::from_le_bytes(fmt[4..8].try_into().unwrap()));
                block_align = Some(u16::from_le_bytes(fmt[12..14].try_into().unwrap()));
                if size % 2 != 0 {
                    file.seek(std::io::SeekFrom::Current(1))?;
                }
            }
            b"data" => {
                data_bytes = Some(size);
                file.seek(std::io::SeekFrom::Current(
                    i64::try_from(size).unwrap_or(i64::MAX),
                ))?;
                if size % 2 != 0 {
                    file.seek(std::io::SeekFrom::Current(1))?;
                }
            }
            _ => {
                file.seek(std::io::SeekFrom::Current(
                    i64::try_from(size).unwrap_or(i64::MAX),
                ))?;
                if size % 2 != 0 {
                    file.seek(std::io::SeekFrom::Current(1))?;
                }
            }
        }
    }
    let channels = channels.context("WAV format chunk is missing")?;
    let sample_rate = sample_rate.context("WAV sample rate is missing")?;
    let block_align = block_align.context("WAV block alignment is missing")?;
    let data_bytes = data_bytes.context("WAV data chunk is missing")?;
    if channels == 0 || sample_rate == 0 || block_align == 0 {
        bail!("WAV format metadata is invalid");
    }
    Ok((channels, sample_rate, data_bytes / u64::from(block_align)))
}

fn temporary_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    destination.with_file_name(name)
}

fn timeline_position_to_samples(
    value: f64,
    unit: &str,
    sample_rate: f64,
    tempo_events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<u64> {
    if !value.is_finite() || value < 0.0 {
        bail!("DAWproject timeline position is invalid");
    }
    let samples = match unit {
        "beats" => beats_to_samples(value, sample_rate, tempo_events, base_bpm)?,
        "seconds" => value * sample_rate,
        other => bail!("unsupported DAWproject timeline unit: {other}"),
    };
    if !samples.is_finite() || samples < 0.0 || samples > u64::MAX as f64 {
        bail!("DAWproject timeline position exceeds Hirari's sample range");
    }
    Ok(samples.round() as u64)
}

fn beats_to_samples(
    beats: f64,
    sample_rate: f64,
    events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<f64> {
    if !beats.is_finite() || beats < 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        bail!("tempo conversion input is invalid");
    }
    let ordered = normalize_tempo_events(events.to_vec(), base_bpm)?;
    let mut elapsed_seconds = 0.0;
    for (index, current) in ordered.iter().enumerate() {
        let Some(next) = ordered.get(index + 1) else {
            if beats <= current.beat {
                return Ok(elapsed_seconds * sample_rate);
            }
            return Ok(
                (elapsed_seconds + (beats - current.beat) * 60.0 / current.bpm) * sample_rate,
            );
        };
        let beat_span = next.beat - current.beat;
        if beat_span < 0.0 || current.bpm <= 0.0 || next.bpm <= 0.0 {
            bail!("DAWproject tempo map is invalid");
        }
        let duration_seconds = if current.ramp {
            120.0 * beat_span / (current.bpm + next.bpm)
        } else {
            60.0 * beat_span / current.bpm
        };
        if beats <= next.beat {
            let beat_offset = (beats - current.beat).max(0.0).min(beat_span);
            let seconds_offset = if current.ramp && duration_seconds > 0.0 {
                let alpha = (next.bpm - current.bpm) / duration_seconds;
                if alpha.abs() < 1e-12 {
                    beat_offset * 60.0 / current.bpm
                } else {
                    let discriminant = current.bpm.powi(2) + 120.0 * alpha * beat_offset;
                    if discriminant < 0.0 {
                        bail!("tempo ramp cannot be integrated");
                    }
                    (-current.bpm + discriminant.sqrt()) / alpha
                }
            } else {
                beat_offset * 60.0 / current.bpm
            };
            return Ok(
                (elapsed_seconds + seconds_offset.clamp(0.0, duration_seconds)) * sample_rate,
            );
        }
        elapsed_seconds += duration_seconds;
    }
    bail!("could not convert beats to samples")
}

#[path = "dawproject_parse.rs"]
mod parse_;
pub(in crate::dawproject) use parse_::*;
#[path = "dawproject_export.rs"]
mod export_;
pub use export_::export_dawproject_file;
#[path = "dawproject_import.rs"]
mod import_;
pub use import_::import_dawproject_file;
