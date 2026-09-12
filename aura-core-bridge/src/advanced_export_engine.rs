#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodecRust {
    Wav,
    Aiff,
    Flac,
    Mp3,
    Aac,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchExportSpec { pub stem: String, pub formats: Vec<CodecRust>, pub normalize_lufs: Option<i16> }

impl BatchExportSpec {
    pub fn validate(&self) -> bool { !self.stem.trim().is_empty() && self.stem.len() <= 256 && !self.formats.is_empty() && self.formats.len() <= 8 && self.deduplicated_formats().len() == self.formats.len() && self.normalize_lufs.map(|v| (-60..=0).contains(&v)).unwrap_or(true) }
    pub fn safe_stem(&self) -> Option<String> { if !self.validate() || self.stem.bytes().any(|b| b == 0 || b == b'/' || b == b'\\' || b < 0x20) { return None; } let normalized = self.stem.trim().trim_matches('.'); (!normalized.is_empty() && normalized != ".." && normalized.len() <= 128).then(|| normalized.to_owned()) }
    pub fn deduplicated_formats(&self) -> Vec<CodecRust> { let mut out = Vec::new(); for format in &self.formats { if !out.contains(format) { out.push(*format); } } out }
}

#[cfg(test)]
mod batch_export_tests {
    use super::*;
    #[test]
    fn safe_stem_rejects_paths_and_normalizes() {
        let base = |stem: &str| BatchExportSpec { stem: stem.into(), formats: vec![CodecRust::Wav], normalize_lufs: None };
        assert_eq!(base("  Mixdown... ").safe_stem().as_deref(), Some("Mixdown"));
        assert!(base("../escape").safe_stem().is_none());
        assert!(base("bad\nname").safe_stem().is_none());
        let duplicate = BatchExportSpec { stem: "x".into(), formats: vec![CodecRust::Wav, CodecRust::Wav, CodecRust::Mp3], normalize_lufs: None };
        assert_eq!(duplicate.deduplicated_formats(), vec![CodecRust::Wav, CodecRust::Mp3]);
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExportFormatRust {
    pub codec: CodecRust,
    pub bit_depth: u32,
    pub sample_rate: u32,
    pub normalize: bool,
}

impl ExportFormatRust {
    pub fn validate(&self) -> bool {
        (8_000..=384_000).contains(&self.sample_rate)
            && match self.codec {
                CodecRust::Wav => matches!(self.bit_depth, 16 | 24 | 32),
                CodecRust::Aiff => matches!(self.bit_depth, 8 | 16 | 24 | 32),
                CodecRust::Flac | CodecRust::Mp3 | CodecRust::Aac => self.bit_depth == 16,
            }
    }
}

pub fn validate_rendered_buffer(samples: &[f32], channels: u16, peak_limit: f32) -> bool { !samples.is_empty() && (1..=32).contains(&channels) && samples.len().is_multiple_of(channels as usize) && peak_limit.is_finite() && samples.iter().all(|s| s.is_finite() && s.abs() <= peak_limit) }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StemJobRust {
    pub track_id: u32,
    pub stem_name: String,
    pub format: ExportFormatRust,
}

pub struct ExportOrchestrator {
    pub active_jobs: Vec<StemJobRust>,
    completed: Vec<(u32, PathBuf)>,
    failed: Vec<(u32, String)>,
    pub last_error: Option<String>,
    renderer: Option<Box<dyn FnMut(u32) -> Result<Vec<f32>, String> + Send>>,
}

include!("advanced_export_batch.rs");
include!("advanced_export_plan.rs");
include!("advanced_export_queue.rs");
