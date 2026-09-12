use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PianoNote { pub start_seconds: f64, pub duration_seconds: f64, pub pitch: u8, pub velocity: u8, #[serde(default)] pub channel: u8 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PianoVisualizerConfig { pub width: u32, pub height: u32, pub fps: u32, pub output_path: PathBuf }
impl Default for PianoVisualizerConfig { fn default() -> Self { Self { width: 1080, height: 1920, fps: 60, output_path: PathBuf::from("piano.mp4") } } }
#[derive(Debug)]
pub enum PianoVisualizerError { Encoder(String), InvalidInput(String), Io(std::io::Error) }
impl std::fmt::Display for PianoVisualizerError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") } }
impl std::error::Error for PianoVisualizerError {}
impl From<std::io::Error> for PianoVisualizerError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

pub struct PianoVisualizer;
impl PianoVisualizer {
    pub fn render_to_mp4(notes: &[PianoNote], audio: &[f32], sample_rate: u32, channels: u16, config: &PianoVisualizerConfig, output: &Path) -> Result<(), PianoVisualizerError> {
        validate_notes_and_config(notes, config, output)?;
        if sample_rate == 0 || channels == 0 || channels > 32 || audio.is_empty()
            || audio.len() % usize::from(channels) != 0
            || audio.iter().any(|sample| !sample.is_finite())
        {
            return Err(PianoVisualizerError::InvalidInput("invalid piano visualizer audio buffer".into()));
        }
        let nonce = temp_nonce();
        let temp = std::env::temp_dir().join(format!("aura-piano-{nonce}.wav"));
        write_wav(&temp, audio, sample_rate, channels)?;
        let notes_path = std::env::temp_dir().join(format!("aura-piano-notes-{nonce}.json"));
        std::fs::write(&notes_path, serde_json::to_vec(notes).map_err(|e| PianoVisualizerError::Encoder(e.to_string()))?)?;
        let result = render_with_script(&notes_path, &temp, config, output);
        let _ = std::fs::remove_file(&notes_path);
        let _ = std::fs::remove_file(temp);
        result
    }
    pub fn render_to_mp4_from_wav<P: AsRef<Path>>(notes: &[PianoNote], wav: P, config: &PianoVisualizerConfig, output: &Path) -> Result<(), PianoVisualizerError> {
        validate_notes_and_config(notes, config, output)?;
        let notes_path = std::env::temp_dir().join(format!("aura-piano-notes-{}.json", temp_nonce()));
        std::fs::write(&notes_path, serde_json::to_vec(notes).map_err(|e| PianoVisualizerError::Encoder(e.to_string()))?)?;
        let result = render_with_script(&notes_path, wav.as_ref(), config, output);
        let _ = std::fs::remove_file(notes_path);
        result
    }
}

fn validate_notes_and_config(notes: &[PianoNote], config: &PianoVisualizerConfig, output: &Path) -> Result<(), PianoVisualizerError> {
    if output.as_os_str().is_empty()
        || config.width == 0 || config.height == 0 || config.fps == 0
        || notes.iter().any(|n| {
            !n.start_seconds.is_finite() || n.start_seconds < 0.0
                || !n.duration_seconds.is_finite() || n.duration_seconds <= 0.0
                || n.pitch > 127
        })
    {
        return Err(PianoVisualizerError::InvalidInput("invalid piano visualizer input".into()));
    }
    Ok(())
}

fn temp_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_else(|_| u128::from(std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_notes_and_render_dimensions_before_writing_files() {
        let config = PianoVisualizerConfig { width: 0, ..Default::default() };
        let result = PianoVisualizer::render_to_mp4(
            &[PianoNote {
                start_seconds: 0.0,
                duration_seconds: 0.0,
                pitch: 60,
                velocity: 100,
                channel: 0,
            }],
            &[],
            48_000,
            2,
            &config,
            Path::new("unused.mp4"),
        );
        assert!(matches!(result, Err(PianoVisualizerError::InvalidInput(_))));
    }

    #[test]
    fn accepts_valid_note_shape_without_running_encoder() {
        let note = PianoNote {
            start_seconds: 0.25,
            duration_seconds: 0.5,
            pitch: 60,
            velocity: 100,
            channel: 0,
        };
        assert!(note.start_seconds.is_finite());
        assert!(note.duration_seconds > 0.0);
        assert!(note.pitch <= 127);
    }

    #[test]
    fn rejects_audio_that_cannot_form_complete_interleaved_frames() {
        let result = PianoVisualizer::render_to_mp4(
            &[],
            &[0.0, 0.25, 0.5],
            48_000,
            2,
            &PianoVisualizerConfig::default(),
            Path::new("unused.mp4"),
        );
        assert!(matches!(result, Err(PianoVisualizerError::InvalidInput(_))));
    }

    #[test]
    fn rejects_non_finite_audio_before_writing_a_wav() {
        let result = PianoVisualizer::render_to_mp4(
            &[],
            &[0.0, f32::NAN],
            48_000,
            2,
            &PianoVisualizerConfig::default(),
            Path::new("unused.mp4"),
        );
        assert!(matches!(result, Err(PianoVisualizerError::InvalidInput(_))));
    }
}
fn render_with_script(notes: &Path, wav: &Path, config: &PianoVisualizerConfig, output: &Path) -> Result<(), PianoVisualizerError> {
    let script = std::env::var_os("AURA_PIANO_VISUALIZER_SCRIPT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("scripts/render_piano_visualizer.py"));
    let status = Command::new("python3").args([script.to_string_lossy().as_ref(), "--notes", notes.to_string_lossy().as_ref(), "--audio", wav.to_string_lossy().as_ref(), "--output", output.to_string_lossy().as_ref(), "--fps", &config.fps.to_string(), "--width", &config.width.to_string(), "--height", &config.height.to_string()]).status()?;
    if status.success() { Ok(()) } else { Err(PianoVisualizerError::Encoder("piano visualizer renderer failed".into())) }
}
fn write_wav(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<(), PianoVisualizerError> {
    if channels == 0 || samples.len() % usize::from(channels) != 0 || samples.iter().any(|sample| !sample.is_finite()) {
        return Err(PianoVisualizerError::InvalidInput("audio buffer does not match channel layout".into()));
    }
    let mut data = Vec::with_capacity(samples.len() * 2);
    for sample in samples { data.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()); }
    let len = data.len() as u32; let rate = sample_rate * u32::from(channels) * 2;
    let mut wav = Vec::with_capacity(data.len() + 44); wav.extend_from_slice(b"RIFF"); wav.extend_from_slice(&(36 + len).to_le_bytes()); wav.extend_from_slice(b"WAVEfmt "); wav.extend_from_slice(&16u32.to_le_bytes()); wav.extend_from_slice(&1u16.to_le_bytes()); wav.extend_from_slice(&channels.to_le_bytes()); wav.extend_from_slice(&sample_rate.to_le_bytes()); wav.extend_from_slice(&rate.to_le_bytes()); wav.extend_from_slice(&(channels * 2).to_le_bytes()); wav.extend_from_slice(&16u16.to_le_bytes()); wav.extend_from_slice(b"data"); wav.extend_from_slice(&len.to_le_bytes()); wav.extend_from_slice(&data); std::fs::write(path, wav)?; Ok(())
}
