//! Objective vocal/UTAU tuning diagnostics.
//!
//! The editor can use this report after rendering a singer.  It deliberately
//! compares rendered pitch frames against the intended note plan, so a pretty
//! waveform is not mistaken for a well-tuned vocal.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VocalNoteTarget {
    pub start_sample: u64,
    pub end_sample: u64,
    pub midi_note: u8,
    #[serde(default)]
    pub lyric: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VocalPitchFrame {
    pub sample: u64,
    pub frequency_hz: f32,
    pub rms: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VocalNoteDiagnostic {
    pub note_index: usize,
    pub lyric: String,
    pub pitch_error_cents: f32,
    pub pitch_instability_cents: f32,
    pub onset_error_ms: f32,
    pub energy_cv: f32,
    pub severity: f32,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VocalQualityReport {
    pub score: f32,
    pub sample_rate: f32,
    pub notes_analyzed: usize,
    pub failed_notes: usize,
    pub diagnostics: Vec<VocalNoteDiagnostic>,
}

fn cents(actual: f32, expected: f32) -> f32 {
    if actual <= 0.0 || expected <= 0.0 {
        1200.0
    } else {
        1200.0 * (actual / expected).log2()
    }
}

/// Grade a rendered vocal against the MIDI/UTAU note plan.
pub fn analyze_vocal_quality(
    targets: &[VocalNoteTarget],
    frames: &[VocalPitchFrame],
    sample_rate: f32,
) -> VocalQualityReport {
    let sr = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
        sample_rate
    } else {
        44_100.0
    };
    let mut diagnostics = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let expected = 440.0 * 2.0_f32.powf((target.midi_note as f32 - 69.0) / 12.0);
        let window: Vec<&VocalPitchFrame> = frames
            .iter()
            .filter(|frame| {
                frame.sample >= target.start_sample
                    && frame.sample <= target.end_sample
                    && frame.frequency_hz.is_finite()
                    && frame.frequency_hz > 20.0
            })
            .collect();
        let mut issues = Vec::new();
        if window.is_empty() {
            issues.push("no_voiced_signal".to_string());
            diagnostics.push(VocalNoteDiagnostic {
                note_index: index,
                lyric: target.lyric.clone(),
                pitch_error_cents: 1200.0,
                pitch_instability_cents: 1200.0,
                onset_error_ms: 0.0,
                energy_cv: 1.0,
                severity: 1.0,
                issues,
            });
            continue;
        }
        let errors: Vec<f32> = window
            .iter()
            .map(|frame| cents(frame.frequency_hz, expected))
            .collect();
        let mean = errors.iter().sum::<f32>() / errors.len() as f32;
        let abs_error = errors.iter().map(|v| v.abs()).sum::<f32>() / errors.len() as f32;
        let variance = errors.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / errors.len() as f32;
        let first = window[0].sample.saturating_sub(target.start_sample) as f32 / sr * 1000.0;
        let energies: Vec<f32> = window.iter().map(|frame| frame.rms.max(0.0)).collect();
        let energy_mean = energies.iter().sum::<f32>() / energies.len() as f32;
        let energy_cv = if energy_mean > 1.0e-5 {
            (energies
                .iter()
                .map(|v| (v - energy_mean).powi(2))
                .sum::<f32>()
                / energies.len() as f32)
                .sqrt()
                / energy_mean
        } else {
            1.0
        };
        let mut severity = 0.0;
        if abs_error > 35.0 {
            issues.push("pitch_off_center".into());
            severity += ((abs_error - 35.0) / 120.0).min(1.0) * 0.55;
        }
        if variance.sqrt() > 28.0 {
            issues.push("pitch_wobble".into());
            severity += ((variance.sqrt() - 28.0) / 100.0).min(1.0) * 0.2;
        }
        if first > 55.0 {
            issues.push("late_onset".into());
            severity += ((first - 55.0) / 180.0).min(1.0) * 0.15;
        }
        if energy_cv > 0.65 {
            issues.push("unstable_energy".into());
            severity += ((energy_cv - 0.65) / 1.2).min(1.0) * 0.1;
        }
        diagnostics.push(VocalNoteDiagnostic {
            note_index: index,
            lyric: target.lyric.clone(),
            pitch_error_cents: abs_error,
            pitch_instability_cents: variance.sqrt(),
            onset_error_ms: first,
            energy_cv,
            severity: severity.min(1.0),
            issues,
        });
    }
    let failed_notes = diagnostics
        .iter()
        .filter(|note| note.severity >= 0.35)
        .count();
    let average = if diagnostics.is_empty() {
        1.0
    } else {
        diagnostics.iter().map(|note| note.severity).sum::<f32>() / diagnostics.len() as f32
    };
    VocalQualityReport {
        score: (100.0 * (1.0 - average)).clamp(0.0, 100.0),
        sample_rate: sr,
        notes_analyzed: diagnostics.len(),
        failed_notes,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_flat_and_late_render() {
        let target = VocalNoteTarget {
            start_sample: 0,
            end_sample: 44_100,
            midi_note: 69,
            lyric: "la".into(),
        };
        let frames = vec![
            VocalPitchFrame {
                sample: 5_000,
                frequency_hz: 450.0,
                rms: 0.2,
            },
            VocalPitchFrame {
                sample: 20_000,
                frequency_hz: 450.0,
                rms: 0.2,
            },
        ];
        let report = analyze_vocal_quality(&[target], &frames, 44_100.0);
        assert!(report.score < 100.0);
        assert!(report.diagnostics[0]
            .issues
            .iter()
            .any(|issue| issue == "late_onset"));
    }
}
