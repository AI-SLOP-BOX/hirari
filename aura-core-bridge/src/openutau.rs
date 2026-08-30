//! OpenUtau bridge metadata.
//!
//! OpenUtau is a standalone vocal editor rather than AU/VST3/CLAP. Aura
//! therefore embeds its source/render relationship in the project model and
//! treats the rendered WAV as the audio-region input. This keeps the DAW
//! project deterministic without pretending that OpenUtau is an audio plugin.

use sha2::{Digest, Sha256};
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const HEADER_PROBE_BYTES: usize = 1024 * 1024;
const DEFAULT_APP_PATH: &str = "/Applications/OpenUtau.app";

/// Resolve the OpenUtau application without baking a user's installation
/// layout into the project.  Forks and CI can point at a portable/test build
/// with `AURA_OPENUTAU_APP`; the default preserves the normal macOS install.
pub fn application_path() -> std::path::PathBuf {
    application_path_from(std::env::var_os("AURA_OPENUTAU_APP"))
}

fn application_path_from(configured: Option<std::ffi::OsString>) -> std::path::PathBuf {
    configured
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_APP_PATH))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenUtauStatus {
    pub installed: bool,
    pub app_path: Option<String>,
    pub singer_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenUtauFileAudit {
    pub source_path: String,
    pub rendered_audio_path: String,
    pub source_hash: String,
    pub rendered_audio_hash: String,
    pub rendered_audio_bytes: u64,
    pub rendered_sample_rate: Option<u32>,
    pub rendered_channels: Option<u16>,
    pub rendered_frames: Option<u64>,
    #[serde(default)]
    pub source_note_count: u64,
    #[serde(default)]
    pub source_singers: Vec<String>,
}

/// A bounded, format-neutral note representation for the in-DAW vocal editor.
/// UST uses tick positions while USTX uses the same project tick domain; the
/// caller converts ticks to beats using the project resolution.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OpenUtauNotePreview {
    pub position: u64,
    pub duration: u64,
    pub pitch: Option<u16>,
    pub lyric: String,
}

/// Parse vocal notes without launching OpenUtau. This is intentionally a
/// bounded control-plane parser: malformed notes are skipped and the result
/// is capped so a hostile source file cannot exhaust the UI model.
pub fn parse_notes(path: &str) -> Vec<OpenUtauNotePreview> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    if bytes.len() > MAX_SOURCE_BYTES as usize { return Vec::new(); }
    let text = String::from_utf8_lossy(&bytes);
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "ust" { parse_ust_notes(&text) } else { parse_ustx_notes(&text) }
}

/// Convert OpenUtau ticks into Aura's canonical sample-based MIDI contract.
/// This keeps the source editor's timing resolution explicit and rejects any
/// conversion that would overflow the project timeline.
pub fn notes_as_midi(
    path: &str,
    track_id: u32,
    sample_rate: u32,
    ticks_per_beat: u32,
) -> Result<Vec<crate::project_contracts::MidiNoteContract>, String> {
    validate_source_file(path)?;
    if sample_rate == 0 || ticks_per_beat == 0 {
        return Err("OpenUtau MIDI conversion requires positive sample rate and PPQ".into());
    }
    let notes = parse_notes(path);
    let mut result = Vec::with_capacity(notes.len());
    for note in notes {
        let Some(pitch) = note.pitch else { continue };
        if pitch > 127 || note.duration == 0 { continue; }
        let start = (u128::from(note.position) * u128::from(sample_rate))
            .checked_div(u128::from(ticks_per_beat))
            .ok_or_else(|| "OpenUtau note start conversion failed".to_owned())?;
        let length = (u128::from(note.duration) * u128::from(sample_rate))
            .checked_div(u128::from(ticks_per_beat))
            .ok_or_else(|| "OpenUtau note length conversion failed".to_owned())?;
        let start_sample = u64::try_from(start).map_err(|_| "OpenUtau note start overflows sample timeline")?;
        let length_samples = u64::try_from(length.max(1)).map_err(|_| "OpenUtau note length overflows sample timeline")?;
        let contract = crate::project_contracts::MidiNoteContract {
            track_id,
            pitch: pitch as u8,
            velocity: 100,
            start_sample,
            length_samples,
            lyric: note.lyric,
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            portamento_samples: 0, probability: 100, repeat_count: 1,
        };
        contract.validate().map_err(|error| error.to_string())?;
        result.push(contract);
    }
    Ok(result)
}

fn parse_ust_notes(text: &str) -> Vec<OpenUtauNotePreview> {
    let mut notes = Vec::new();
    let mut position = 0u64;
    let mut duration = None;
    let mut pitch = None;
    let mut lyric = String::new();
    let flush = |notes: &mut Vec<OpenUtauNotePreview>, position: &mut u64,
                     duration: &mut Option<u64>, pitch: &mut Option<u16>, lyric: &mut String| {
        if let (Some(length), Some(note_pitch)) = (*duration, *pitch) {
            if length > 0 && notes.len() < 4096 {
                notes.push(OpenUtauNotePreview {
                    position: *position,
                    duration: length,
                    pitch: Some(note_pitch),
                    lyric: lyric.clone(),
                });
            }
            *position = position.saturating_add(length);
        }
        *duration = None;
        *pitch = None;
        lyric.clear();
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[#") {
            flush(&mut notes, &mut position, &mut duration, &mut pitch, &mut lyric);
        } else if let Some(value) = trimmed.strip_prefix("Length=") {
            duration = value.trim().parse::<u64>().ok();
        } else if let Some(value) = trimmed.strip_prefix("NoteNum=") {
            pitch = value.trim().parse::<u16>().ok().filter(|value| *value <= 127);
        } else if let Some(value) = trimmed.strip_prefix("Lyric=") {
            lyric = value.trim().trim_matches('"').to_owned();
        }
    }
    flush(&mut notes, &mut position, &mut duration, &mut pitch, &mut lyric);
    notes
}

fn parse_ustx_notes(text: &str) -> Vec<OpenUtauNotePreview> {
    let mut notes = Vec::new();
    let mut position = None;
    let mut duration = None;
    let mut pitch = None;
    let mut lyric = String::new();
    let flush = |notes: &mut Vec<OpenUtauNotePreview>, position: &mut Option<u64>,
                     duration: &mut Option<u64>, pitch: &mut Option<u16>, lyric: &mut String| {
        if let (Some(start), Some(length)) = (*position, *duration) {
            if length > 0 && notes.len() < 4096 {
                notes.push(OpenUtauNotePreview { position: start, duration: length, pitch: *pitch, lyric: lyric.clone() });
            }
        }
        *position = None;
        *duration = None;
        *pitch = None;
        lyric.clear();
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- position:") {
            flush(&mut notes, &mut position, &mut duration, &mut pitch, &mut lyric);
            position = trimmed.strip_prefix("- position:").and_then(|value| value.trim().parse().ok());
        } else if let Some(value) = trimmed.strip_prefix("position:") {
            position = value.trim().parse().ok();
        } else if let Some(value) = trimmed.strip_prefix("- duration:") {
            duration = value.trim().parse().ok();
        } else if let Some(value) = trimmed.strip_prefix("duration:") {
            duration = value.trim().parse().ok();
        } else if let Some(value) = trimmed.strip_prefix("tone:") {
            pitch = value.trim().parse::<u16>().ok().filter(|value| *value <= 127);
        } else if let Some(value) = trimmed.strip_prefix("- tone:") {
            pitch = value.trim().parse::<u16>().ok().filter(|value| *value <= 127);
        } else if let Some(value) = trimmed.strip_prefix("lyric:") {
            lyric = value.trim().trim_matches('"').to_owned();
        } else if let Some(value) = trimmed.strip_prefix("- lyric:") {
            lyric = value.trim().trim_matches('"').to_owned();
        }
    }
    flush(&mut notes, &mut position, &mut duration, &mut pitch, &mut lyric);
    notes
}

pub fn status() -> OpenUtauStatus {
    let app = application_path();
    let singer = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|home| home.join("Library/OpenUtau/Singers"))
        .filter(|path| path.is_dir());
    OpenUtauStatus {
        installed: app.is_dir(),
        app_path: app.is_dir().then(|| app.display().to_string()),
        singer_root: singer.map(|path| path.display().to_string()),
    }
}

pub fn validate_source(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.eq_ignore_ascii_case("ustx") || v.eq_ignore_ascii_case("ust"))
        .unwrap_or(false)
    {
        return Ok(());
    }
    Err("OpenUtau source must use the .ustx or .ust extension".into())
}

/// Validate a source at an external-process boundary.  `validate_source` is
/// intentionally structural so project loading can report missing assets;
/// import/parse operations must additionally reject missing, non-regular, or
/// oversized files before they enter the editor.
pub fn validate_source_file(path: &str) -> Result<(), String> {
    validate_source(path)?;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("OpenUtau source cannot be read: {error}"))?;
    if !metadata.is_file() {
        return Err("OpenUtau source is not a regular file".into());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err("OpenUtau source exceeds the 64 MiB limit".into());
    }
    Ok(())
}

/// Return a bounded note summary for the embedded vocal editor.  Include the
/// actual tick span and MIDI pitch, not only a lyric label: this makes timing,
/// length, and pitch mistakes visible before launching the external editor.
/// The summary is parsed on the control thread and is never used by audio.
pub fn note_preview(path: &str) -> Vec<String> {
    parse_notes(path)
        .into_iter()
        .take(256)
        .enumerate()
        .map(|(index, note)| {
            let label = note
                .pitch
                .map(|pitch| format!("{} / {}", pitch, pitch_name(pitch)))
                .unwrap_or_else(|| "-- / unknown".to_owned());
            let end = note.position.saturating_add(note.duration);
            let lyric = if note.lyric.is_empty() { "…" } else { note.lyric.as_str() };
            format!(
                "#{:03}  tick {:>6}–{:>6}  len {:>5}  {}  ·  {}",
                index + 1,
                note.position,
                end,
                note.duration,
                label,
                lyric.replace(['\n', '\r'], " ")
            )
        })
        .collect()
}

fn pitch_name(note: u16) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (note / 12) as i32 - 1;
    format!("{}{}", NAMES[(note % 12) as usize], octave)
}

pub fn validate_render(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if matches!(
        path.extension()
            .and_then(|v| v.to_str())
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("wav" | "aif" | "aiff")
    ) {
        return Ok(());
    }
    Err("OpenUtau render must use WAV/AIFF extension".into())
}

pub fn validate_import_files(source: &str, render: &str) -> Result<(), String> {
    validate_source(source)?;
    validate_render(render)?;
    for (label, path) in [("source", source), ("rendered audio", render)] {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| format!("OpenUtau {label} file is missing"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("OpenUtau {label} must not be a symbolic link"));
        }
        if !metadata.is_file() {
            return Err(format!("OpenUtau {label} is not a regular file"));
        }
    }
    Ok(())
}

/// Build the persisted identity for an OpenUtau source/render pair.  This is
/// deliberately performed on the control thread; the audio callback never
/// reads or hashes external files.
pub fn audit_import_files(source: &str, render: &str) -> Result<OpenUtauFileAudit, String> {
    validate_import_files(source, render)?;
    let source_size = std::fs::metadata(source)
        .map_err(|error| format!("stat OpenUtau source: {error}"))?
        .len();
    if source_size > MAX_SOURCE_BYTES {
        return Err(format!("OpenUtau source exceeds {} MiB limit", MAX_SOURCE_BYTES / (1024 * 1024)));
    }
    let source_bytes = std::fs::read(source).map_err(|error| format!("read OpenUtau source: {error}"))?;
    let render_size = std::fs::metadata(render)
        .map_err(|error| format!("stat OpenUtau render: {error}"))?
        .len();
    let render_prefix = read_prefix(render, HEADER_PROBE_BYTES)?;
    let (sample_rate, channels, frames, data_end) = wav_metadata_prefix(&render_prefix);
    validate_render_payload(
        render,
        &render_prefix,
        render_size,
        sample_rate,
        channels,
        frames,
        data_end,
    )?;
    let (rendered_audio_hash, rendered_audio_bytes) = sha256_file(render)?;
    let (source_note_count, source_singers) = source_metadata(source, &source_bytes);
    Ok(OpenUtauFileAudit {
        source_path: source.to_owned(),
        rendered_audio_path: render.to_owned(),
        source_hash: sha256(&source_bytes),
        rendered_audio_hash,
        rendered_audio_bytes,
        rendered_sample_rate: sample_rate,
        rendered_channels: channels,
        rendered_frames: frames,
        source_note_count,
        source_singers,
    })
}

fn validate_render_payload(
    path: &str,
    bytes: &[u8],
    file_size: u64,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    frames: Option<u64>,
    data_end: Option<u64>,
) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "wav" => {
            if sample_rate.is_none()
                || channels.is_none()
                || frames.unwrap_or(0) == 0
                || file_size < 12
                || data_end.is_none_or(|end| end > file_size)
            {
                return Err("OpenUtau WAV render has no valid non-empty RIFF/WAVE audio payload".into());
            }
        }
        "aif" | "aiff" => {
            if bytes.len() < 12 || &bytes[0..4] != b"FORM" || !matches!(&bytes[8..12], b"AIFF" | b"AIFC") {
                return Err("OpenUtau AIFF render has no valid FORM/AIFF payload".into());
            }
        }
        _ => return Err("OpenUtau render extension is unsupported".into()),
    }
    Ok(())
}

fn source_metadata(path: &str, bytes: &[u8]) -> (u64, Vec<String>) {
    let text = String::from_utf8_lossy(bytes);
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "ustx" {
        let note_count = text
            .lines()
            .filter(|line| line.trim_start().starts_with("- position:"))
            .count() as u64;
        let mut singers = Vec::new();
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("- singer:") {
                let singer = value.trim().trim_matches('"').to_owned();
                if !singer.is_empty() && !singers.contains(&singer) {
                    singers.push(singer);
                }
            }
        }
        return (note_count, singers);
    }

    let mut note_count = 0u64;
    let mut has_note_num = false;
    for line in text.lines() {
        if line.starts_with("#[") {
            if has_note_num {
                note_count += 1;
            }
            has_note_num = false;
        } else if let Some(value) = line.strip_prefix("NoteNum=") {
            has_note_num = value.trim().parse::<u16>().is_ok();
        }
    }
    if has_note_num {
        note_count += 1;
    }
    (note_count, Vec::new())
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_prefix(path: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|error| format!("open OpenUtau render: {error}"))?;
    let mut prefix = vec![0u8; limit];
    let read = file
        .read(&mut prefix)
        .map_err(|error| format!("read OpenUtau render header: {error}"))?;
    prefix.truncate(read);
    Ok(prefix)
}

fn sha256_file(path: &str) -> Result<(String, u64), String> {
    let mut file = File::open(path).map_err(|error| format!("open OpenUtau render: {error}"))?;
    // Keep the explicit seek so a future caller can reuse this helper after a
    // header probe without accidentally hashing from the current offset.
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("seek OpenUtau render: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("hash OpenUtau render: {error}"))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| "OpenUtau render size overflow".to_owned())?;
        hasher.update(&buffer[..count]);
    }
    let hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((hash, total))
}

/// Parse the RIFF/RF64 header without requiring the complete data payload to
/// be resident in memory. The full-file size is checked by the caller; this
/// helper only needs enough bytes to reach fmt/data metadata.
fn wav_metadata_prefix(bytes: &[u8]) -> (Option<u32>, Option<u16>, Option<u64>, Option<u64>) {
    if bytes.len() < 12
        || (&bytes[0..4] != b"RIFF" && &bytes[0..4] != b"RF64")
        || &bytes[8..12] != b"WAVE"
    {
        return (None, None, None, None);
    }
    let mut offset = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut block_align = None;
    let mut rf64_data_bytes = None;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[offset..offset + 4];
        let size32 = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        let start = offset + 8;
        if id == b"ds64" && size32 >= 16 {
            if start.checked_add(16).is_none_or(|end| end > bytes.len()) {
                return (None, None, None, None);
            }
            rf64_data_bytes = Some(u64::from_le_bytes(
                bytes[start + 8..start + 16].try_into().unwrap(),
            ));
        }
        if id == b"fmt " {
            if size32 < 16 || start.checked_add(16).is_none_or(|end| end > bytes.len()) {
                return (None, None, None, None);
            }
            channels = Some(u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()));
            sample_rate = Some(u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()));
            block_align = Some(u16::from_le_bytes(bytes[start + 12..start + 14].try_into().unwrap()));
        } else if id == b"data" {
            let data_size = if size32 == u32::MAX {
                match rf64_data_bytes {
                    Some(size) => size,
                    None => return (None, None, None, None),
                }
            } else {
                u64::from(size32)
            };
            let Some(data_end) = (start as u64).checked_add(data_size) else {
                return (None, None, None, None);
            };
            return (
                sample_rate,
                channels,
                block_align
                    .filter(|align| *align > 0)
                    .map(|align| data_size / u64::from(align)),
                Some(data_end),
            );
        }
        let Some(padded) = u64::from(size32).checked_add(u64::from(size32 & 1)) else {
            return (None, None, None, None);
        };
        let Ok(padded) = usize::try_from(padded) else {
            return (None, None, None, None);
        };
        let Some(next) = start.checked_add(padded) else {
            return (None, None, None, None);
        };
        offset = next;
        if offset > bytes.len() {
            break;
        }
    }
    (sample_rate, channels, None, None)
}

#[cfg(test)]
fn wav_metadata(bytes: &[u8]) -> (Option<u32>, Option<u16>, Option<u64>) {
    if bytes.len() < 12
        || (&bytes[0..4] != b"RIFF" && &bytes[0..4] != b"RF64")
        || &bytes[8..12] != b"WAVE"
    {
        return (None, None, None);
    }
    let mut offset = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut block_align = None;
    let mut data_bytes = None;
    let mut rf64_data_bytes = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size32 = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        let start = offset + 8;
        if start > bytes.len() {
            return (None, None, None);
        }
        if id == b"ds64" && size32 >= 16 && start + 16 <= bytes.len() {
            rf64_data_bytes = Some(u64::from_le_bytes(
                bytes[start + 8..start + 16].try_into().unwrap(),
            ));
        }
        let declared_size = if id == b"data" && size32 == u32::MAX {
            match rf64_data_bytes {
                Some(size) => size,
                None => return (None, None, None),
            }
        } else {
            u64::from(size32)
        };
        let payload_size = match usize::try_from(declared_size) {
            Ok(size) => size,
            Err(_) => return (None, None, None),
        };
        let _payload_end = match start.checked_add(payload_size) {
            Some(end) if end <= bytes.len() => end,
            _ => return (None, None, None),
        };
        if id == b"fmt " && declared_size >= 16 {
            channels = Some(u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()));
            sample_rate = Some(u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()));
            block_align = Some(u16::from_le_bytes(bytes[start + 12..start + 14].try_into().unwrap()));
        } else if id == b"data" {
            data_bytes = Some(declared_size);
        }
        let padded_size = match declared_size.checked_add(declared_size & 1) {
            Some(size) => size,
            None => return (None, None, None),
        };
        let padded_size = match usize::try_from(padded_size) {
            Ok(size) => size,
            Err(_) => return (None, None, None),
        };
        offset = match start.checked_add(padded_size) {
            Some(next) => next,
            None => return (None, None, None),
        };
        if offset > bytes.len() {
            return (None, None, None);
        }
        if data_bytes.is_some() && sample_rate.is_some() && channels.is_some() {
            break;
        }
    }
    let frames = match (data_bytes, block_align) {
        (Some(data), Some(align)) if align > 0 => Some(data / u64::from(align)),
        _ => None,
    };
    (sample_rate, channels, frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_path_uses_default_or_explicit_override() {
        assert_eq!(application_path_from(None), Path::new(DEFAULT_APP_PATH));
        assert_eq!(
            application_path_from(Some(std::ffi::OsString::from("/tmp/OpenUtau.app"))),
            Path::new("/tmp/OpenUtau.app")
        );
        assert_eq!(application_path_from(Some(std::ffi::OsString::new())), Path::new(DEFAULT_APP_PATH));
    }
    use std::io::Write;

    #[test]
    fn accepts_project_references_even_when_assets_are_not_local() {
        assert!(validate_source("voice/chorus.ustx").is_ok());
        assert!(validate_render("audio/chorus.wav").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_source_and_render_files() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "aura-openutau-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        let source_target = root.join("real.ustx");
        let render_target = root.join("real.wav");
        std::fs::write(&source_target, b"ustx_version: \"0.7\"\n").unwrap();
        std::fs::write(&render_target, b"RIFF").unwrap();

        symlink(&source_target, &source).unwrap();
        std::fs::copy(&render_target, &render).unwrap();
        let error = validate_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap_err();
        assert!(error.contains("source") && error.contains("symbolic link"));

        std::fs::remove_file(&source).unwrap();
        std::fs::remove_file(&render).unwrap();
        symlink(&render_target, &render).unwrap();
        let error = validate_import_files(source_target.to_str().unwrap(), render.to_str().unwrap()).unwrap_err();
        assert!(error.contains("rendered audio") && error.contains("symbolic link"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_non_openutau_extensions() {
        assert!(validate_source("voice/chorus.mid").is_err());
        assert!(validate_render("audio/chorus.mp3").is_err());
    }

    #[test]
    fn audit_records_content_identity_and_wav_shape() {
        let root = std::env::temp_dir().join(format!("aura-openutau-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        std::fs::write(&source, b"project-version: 0.1\n").unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0, 0, 0]);
        std::fs::File::create(&render).unwrap().write_all(&wav).unwrap();
        let audit = audit_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap();
        assert_eq!(audit.rendered_audio_bytes, wav.len() as u64);
        assert_eq!(audit.rendered_sample_rate, Some(48_000));
        assert_eq!(audit.rendered_channels, Some(2));
        assert_eq!(audit.rendered_frames, Some(1));
        assert_eq!(audit.source_hash.len(), 64);
        assert_eq!(audit.rendered_audio_hash.len(), 64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn audits_ustx_note_count_and_singer_identity() {
        let root = std::env::temp_dir().join(format!("aura-openutau-shape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        std::fs::write(
            &source,
            "ustx_version: \"0.7\"\ntracks:\n- singer: KasaneTetoOfficial\nvoice_parts:\nnotes:\n  - position: 0\n  - position: 480\n",
        )
        .unwrap();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&88_200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0]);
        std::fs::write(&render, wav).unwrap();
        let audit = audit_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap();
        assert_eq!(audit.source_note_count, 2);
        assert_eq!(audit.source_singers, vec!["KasaneTetoOfficial"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_non_audio_openutau_render_payloads() {
        let root = std::env::temp_dir().join(format!("aura-openutau-invalid-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let render = root.join("voice.wav");
        std::fs::write(&source, "ustx_version: \"0.7\"\n").unwrap();
        std::fs::write(&render, b"not-a-wav").unwrap();
        let error = audit_import_files(source.to_str().unwrap(), render.to_str().unwrap()).unwrap_err();
        assert!(error.contains("valid non-empty RIFF/WAVE"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_rf64_and_unknown_odd_chunks_without_overreading() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RF64");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"ds64");
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&88_200u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(wav_metadata(&bytes), (Some(44_100), Some(1), Some(1)));
    }
}
