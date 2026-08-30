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
    /// A buffer alone is deliberately insufficient evidence for a file label.
    pub fn classify(&self, samples: &[f32], sample_rate: f64) -> AnalysisResultRust {
        if samples.is_empty()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return unknown();
        }
        unknown()
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
        let result = self.classify(&[0.0], 48_000.0);
        matches!(result.asset_type, AssetTypeRust::Unknown)
            && result.confidence == 0.0
            && result.bpm == 0.0
            && result.key == "N/A"
    }
}

#[cfg(test)]
mod tests {
    use super::ClassifierOrchestrator;

    #[test]
    fn classifier_audit_preserves_safe_unknown_fallback() {
        assert!(ClassifierOrchestrator::new().audit_smart_file_classifier());
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
