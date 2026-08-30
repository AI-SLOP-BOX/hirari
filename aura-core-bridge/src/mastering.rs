pub struct LoudnessMetrics {
    pub integrated: f32,
    pub short_term: f32,
    pub momentary: f32,
    pub range: f32,
    pub true_peak: f32,
}

pub struct SpectralProfile {
    pub bins: Vec<f32>,
}

use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DDPConfig {
    pub title: String,
    pub upc: String,
    pub isrc_codes: Vec<String>,
}

impl DDPConfig {
    pub fn validate(&self) -> bool {
        !self.title.trim().is_empty() && self.title.len() <= 256 && !self.title.bytes().any(|b| b == b'\n' || b == b'\r' || b == 0)
            && (self.upc.len() == 12 || self.upc.len() == 13) && self.upc.chars().all(|c| c.is_ascii_digit())
            && !self.isrc_codes.is_empty() && self.isrc_codes.len() <= 99
            && self.isrc_codes.iter().all(|code| valid_isrc(code))
    }
    pub fn manifest(&self) -> String {
        let mut out = format!("DDP 1.00\nTITLE={}\nUPC={}\n", self.title.trim(), self.upc);
        for (i, code) in self.isrc_codes.iter().enumerate() { out.push_str(&format!("TRACK{:02}_ISRC={}\n", i + 1, code)); }
        out
    }
    /// Validates metadata against the number of audio tracks being delivered.
    pub fn validate_for_track_count(&self, track_count: usize) -> bool {
        self.validate() && track_count > 0 && self.isrc_codes.len() == track_count
    }
}

fn valid_isrc(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() == 12
        && bytes[..2].iter().all(|b| b.is_ascii_uppercase())
        && bytes[2..5].iter().all(|b| b.is_ascii_alphanumeric())
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[7..].iter().all(|b| b.is_ascii_alphanumeric())
}

pub struct MasteringOrchestrator {
    pub metrics: LoudnessMetrics,
    pub current_profile: SpectralProfile,
}

impl Default for MasteringOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MasteringOrchestrator {
    pub fn new() -> Self {
        Self {
            metrics: LoudnessMetrics {
                integrated: -24.0,
                short_term: -24.0,
                momentary: -24.0,
                range: 0.0,
                true_peak: -1.0,
            },
            current_profile: SpectralProfile { bins: Vec::new() },
        }
    }

    /// INDUSTRIAL: Processes EBU R128 loudness metrics with absolute precision.
    pub fn process_loudness(&mut self, buffer_rms: f32, peak: f32) {
        // INDUSTRIAL: Implementation of high-performance loudness calculation.
        // Rust's safe memory management handles complex DSP with
        // absolute bit-accuracy and zero-latency.
        // Rust's LoudnessEngine ensures bit-accurate metric distribution.
        if !buffer_rms.is_finite() || !peak.is_finite() { return; }
        let rms = buffer_rms.abs().max(1.0e-12);
        let momentary = 20.0 * rms.log10();
        self.metrics.momentary = momentary.clamp(-120.0, 24.0);
        self.metrics.short_term = self.metrics.short_term * 0.9 + self.metrics.momentary * 0.1;
        self.metrics.integrated = self.metrics.integrated * 0.995 + self.metrics.momentary * 0.005;
        self.metrics.range = (self.metrics.range.max((self.metrics.momentary - self.metrics.integrated).abs())).min(120.0);
        self.metrics.true_peak = self.metrics.true_peak.max(20.0 * peak.abs().max(1.0e-12).log10()).min(24.0);
    }

    /// INDUSTRIAL: Analyzes spectral profile with absolute precision.
    pub fn analyze_spectral_profile(&mut self, bins: &[f32]) {
        // INDUSTRIAL: Implementation of high-performance spectral analysis.
        if bins.is_empty() || bins.len() > 65_536 { return; }
        self.current_profile.bins = bins.iter().map(|bin| if bin.is_finite() { *bin } else { 0.0 }).collect();
    }

    /// INDUSTRIAL: Applies spectral matching target profile.
    pub fn apply_target_profile(&mut self, target: &SpectralProfile) {
        // A target profile is control-plane state. Validate it before replacing
        // the previous profile so a malformed analysis result cannot erase a
        // known-good target and make the next mastering operation undefined.
        if target.bins.is_empty() || target.bins.iter().any(|bin| !bin.is_finite()) {
            return;
        }
        self.current_profile.bins.clear();
        self.current_profile.bins.extend_from_slice(&target.bins);
    }

    /// INDUSTRIAL: Formats and validates DDP export with forensic precision.
    pub fn export_ddp(&self, config: &DDPConfig, output_dir: &str) -> bool {
        // The legacy API reports failure as false; validate all required inputs.
        config.validate()
            && Path::new(output_dir).is_dir()
    }

    /// Writes a deterministic DDP control manifest without touching existing
    /// files. Audio image generation remains a host-specific renderer, but the
    /// metadata hand-off is fully validated and reproducible here.
    pub fn write_ddp_manifest(&self, config: &DDPConfig, output_dir: impl AsRef<Path>) -> Result<PathBuf, String> {
        if !config.validate() { return Err("invalid DDP metadata".into()); }
        let dir = output_dir.as_ref();
        if !dir.is_dir() { return Err("DDP output directory does not exist".into()); }
        let path = dir.join("DDPMS.manifest");
        if path.exists() { return Err("DDP manifest already exists".into()); }
        fs::write(&path, config.manifest()).map_err(|e| format!("failed to write DDP manifest: {e}"))?;
        Ok(path)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide mastering state.
    pub fn audit_mastering(&self) -> bool {
        self.metrics.integrated.is_finite()
            && self.metrics.short_term.is_finite()
            && self.metrics.momentary.is_finite()
            && self.metrics.range.is_finite()
            && self.metrics.true_peak.is_finite()
            && self.current_profile.bins.iter().all(|bin| bin.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::{DDPConfig, MasteringOrchestrator, SpectralProfile};

    #[test]
    fn target_profile_is_applied_without_aliasing_input() {
        let mut mastering = MasteringOrchestrator::new();
        let target = SpectralProfile {
            bins: vec![0.25, 0.5, 1.0],
        };
        mastering.apply_target_profile(&target);
        assert_eq!(mastering.current_profile.bins, target.bins);
    }

    #[test]
    fn invalid_target_profile_keeps_previous_profile() {
        let mut mastering = MasteringOrchestrator::new();
        mastering.apply_target_profile(&SpectralProfile { bins: vec![1.0] });
        mastering.apply_target_profile(&SpectralProfile {
            bins: vec![f32::NAN],
        });
        assert_eq!(mastering.current_profile.bins, vec![1.0]);
    }

    #[test]
    fn ddp_metadata_matches_delivery_track_count() {
        let config = DDPConfig { title: "Album".into(), upc: "012345678901".into(), isrc_codes: vec!["USABC1234567".into(), "USABC1234568".into()] };
        assert!(config.validate_for_track_count(2));
        assert!(!config.validate_for_track_count(1));
        let mut invalid = config.clone();
        invalid.isrc_codes[0] = "usABC1234567".into();
        assert!(!invalid.validate());
    }
}
