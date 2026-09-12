use std::collections::HashMap;
use std::path::Path;

pub enum AssetTypeRust {
    Kick,
    Snare,
    HiHat,
    Percussion,
    Vocal,
    Bass,
    Synth,
    Loop,
    Unknown,
}

pub struct AnalysisResultRust {
    pub asset_type: AssetTypeRust,
    pub confidence: f32,
    pub bpm: f32,
    pub key: String,
}

pub struct ClassifierOrchestrator {}

impl Default for ClassifierOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassifierOrchestrator {
    pub fn new() -> Self {
        Self {}
    }

    /// Classifies samples when no file identity or metadata is available.
    /// This is intentionally conservative: it uses robust time-domain cues,
    /// reports a modest confidence, and falls back to Unknown when the buffer
    /// does not contain enough evidence for a useful label.
    pub fn classify(&self, samples: &[f32], sample_rate: f64) -> AnalysisResultRust {
        if samples.is_empty()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return unknown();
        }
        let duration = samples.len() as f64 / sample_rate;
        if duration < 0.02 {
            return unknown();
        }
        let mut sum_squares = 0.0f64;
        let mut peak = 0.0f32;
        let mut zero_crossings = 0usize;
        let mut previous = samples[0];
        for &sample in samples {
            let value = sample.abs();
            peak = peak.max(value);
            sum_squares += f64::from(sample) * f64::from(sample);
            if (sample >= 0.0) != (previous >= 0.0) {
                zero_crossings += 1;
            }
            previous = sample;
        }
        let rms = (sum_squares / samples.len() as f64).sqrt() as f32;
        if !rms.is_finite() || rms < 1.0e-5 || peak < 1.0e-4 {
            return unknown();
        }
        let zero_cross_rate = zero_crossings as f32 / samples.len() as f32;
        let crest = (peak / rms.max(1.0e-6)).min(100.0);

        let (asset_type, confidence) = if duration >= 2.0 {
            (AssetTypeRust::Loop, 0.48)
        } else if duration <= 0.8 && crest >= 5.0 && zero_cross_rate < 0.12 {
            (AssetTypeRust::Kick, 0.62)
        } else if duration <= 1.2 && zero_cross_rate >= 0.28 {
            (AssetTypeRust::HiHat, 0.56)
        } else if duration <= 1.5 && crest >= 3.0 {
            (AssetTypeRust::Snare, 0.51)
        } else if zero_cross_rate < 0.08 && duration >= 0.25 {
            (AssetTypeRust::Bass, 0.46)
        } else if duration >= 0.25 {
            (AssetTypeRust::Synth, 0.42)
        } else {
            (AssetTypeRust::Unknown, 0.0)
        };
        AnalysisResultRust {
            asset_type,
            confidence,
            bpm: 0.0,
            key: "N/A".to_string(),
        }
    }

    /// Classifies a file using only conservative filename/extension/metadata evidence.
    pub fn classify_file(
        &self,
        file_name: &str,
        metadata: &HashMap<String, String>,
    ) -> AnalysisResultRust {
        let path = Path::new(file_name);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if file_name.trim().is_empty()
            || stem.is_empty()
            || !matches!(
                extension.as_deref(),
                Some("wav" | "aif" | "aiff" | "mp3" | "flac" | "m4a" | "ogg")
            )
        {
            return unknown();
        }

        let mut haystack = stem.to_ascii_lowercase();
        for (key, value) in metadata {
            if matches!(
                key.to_ascii_lowercase().as_str(),
                "name" | "title" | "comment" | "description" | "keywords" | "tags" | "type"
            ) {
                haystack.push(' ');
                haystack.push_str(&value.to_ascii_lowercase());
            }
        }

        let asset_type = if has_token(&haystack, &["kick", "bassdrum", "bd"]) {
            AssetTypeRust::Kick
        } else if has_token(&haystack, &["snare", "sd"]) {
            AssetTypeRust::Snare
        } else if has_token(&haystack, &["hihat", "hi-hat", "hat", "hh", "cymbal"]) {
            AssetTypeRust::HiHat
        } else if has_token(
            &haystack,
            &["perc", "percussion", "shaker", "tom", "clap", "rim"],
        ) {
            AssetTypeRust::Percussion
        } else if has_token(&haystack, &["vocal", "vox", "voice", "acapella"]) {
            AssetTypeRust::Vocal
        } else if has_token(&haystack, &["bass", "sub"]) {
            AssetTypeRust::Bass
        } else if has_token(&haystack, &["synth", "lead", "pad", "pluck", "arp"]) {
            AssetTypeRust::Synth
        } else if has_token(&haystack, &["loop", "phrase", "stem"]) {
            AssetTypeRust::Loop
        } else {
            return unknown();
        };

        AnalysisResultRust {
            asset_type,
            confidence: 0.75,
            bpm: parse_bpm(metadata).unwrap_or(0.0),
            key: parse_key(metadata).unwrap_or_else(|| "N/A".to_string()),
        }
    }

    pub fn audit_smart_file_classifier(&self) -> bool {
        let silence = self.classify(&[0.0; 2_048], 48_000.0);
        let sustained = self.classify(&vec![0.1; 120_000], 48_000.0);
        matches!(silence.asset_type, AssetTypeRust::Unknown)
            && matches!(sustained.asset_type, AssetTypeRust::Loop)
            && sustained.confidence > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{AssetTypeRust, ClassifierOrchestrator};

    #[test]
    fn classifier_rejects_invalid_input_and_detects_sustained_loop() {
        let classifier = ClassifierOrchestrator::new();
        assert!(matches!(classifier.classify(&[], 48_000.0).asset_type, AssetTypeRust::Unknown));
        assert!(matches!(
            classifier.classify(&[f32::NAN; 2_048], 48_000.0).asset_type,
            AssetTypeRust::Unknown
        ));
        let sustained = classifier.classify(&[0.1; 120_000], 48_000.0);
        assert!(matches!(sustained.asset_type, AssetTypeRust::Loop));
        assert!(sustained.confidence > 0.0);
    }

    #[test]
    fn short_high_crest_low_crossing_buffer_is_kick_like() {
        let mut samples = vec![0.0f32; 24_000];
        for (index, sample) in samples.iter_mut().enumerate() {
            *sample = (-(index as f32) / 1_200.0).exp() * 0.9;
        }
        let result = ClassifierOrchestrator::new().classify(&samples, 48_000.0);
        assert!(matches!(result.asset_type, super::AssetTypeRust::Kick));
        assert!(result.confidence > 0.5);
    }
}

fn unknown() -> AnalysisResultRust {
    AnalysisResultRust {
        asset_type: AssetTypeRust::Unknown,
        confidence: 0.0,
        bpm: 0.0,
        key: "N/A".to_string(),
    }
}

fn has_token(text: &str, tokens: &[&str]) -> bool {
    tokens.iter().any(|token| {
        text.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| part == *token)
    })
}

fn parse_bpm(metadata: &HashMap<String, String>) -> Option<f32> {
    let value = metadata
        .iter()
        .find(|(key, _)| matches!(key.to_ascii_lowercase().as_str(), "bpm" | "tempo"))?
        .1
        .trim()
        .parse::<f32>()
        .ok()?;
    (value.is_finite() && (20.0..=300.0).contains(&value)).then_some(value)
}

fn parse_key(metadata: &HashMap<String, String>) -> Option<String> {
    let value = metadata
        .iter()
        .find(|(key, _)| matches!(key.to_ascii_lowercase().as_str(), "key" | "musical_key"))?
        .1
        .trim();
    let valid = !value.is_empty()
        && value.len() <= 3
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value[1..]
            .chars()
            .all(|c| matches!(c, '#' | 'b' | 'm' | 'M' | 'i' | 'a' | 'j' | 'o' | 'r'));
    valid.then(|| value.to_string())
}
