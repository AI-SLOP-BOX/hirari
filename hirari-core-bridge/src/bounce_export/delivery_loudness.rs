//! Loudness/QC requirements, measurements and the artifact gate, split out of
//! delivery.rs.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LoudnessRequirements {
    pub integrated_lufs: f32,
    pub integrated_tolerance_lu: f32,
    pub max_true_peak_dbtp: f32,
    pub max_short_term_lufs: Option<f32>,
    pub max_momentary_lufs: Option<f32>,
    pub max_loudness_range_lu: Option<f32>,
}

impl LoudnessRequirements {
    pub fn ebu_r128() -> Self {
        Self {
            integrated_lufs: -23.0,
            integrated_tolerance_lu: 1.0,
            max_true_peak_dbtp: -1.0,
            max_short_term_lufs: None,
            max_momentary_lufs: None,
            max_loudness_range_lu: None,
        }
    }

    pub fn validate(self) -> bool {
        self.integrated_lufs.is_finite()
            && (-70.0..=0.0).contains(&self.integrated_lufs)
            && self.integrated_tolerance_lu.is_finite()
            && (0.0..=20.0).contains(&self.integrated_tolerance_lu)
            && self.max_true_peak_dbtp.is_finite()
            && (-20.0..=0.0).contains(&self.max_true_peak_dbtp)
            && optional_level(self.max_short_term_lufs, -70.0, 0.0)
            && optional_level(self.max_momentary_lufs, -70.0, 0.0)
            && optional_level(self.max_loudness_range_lu, 0.0, 70.0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LoudnessMeasurements {
    pub integrated_lufs: f32,
    pub max_short_term_lufs: f32,
    pub max_momentary_lufs: f32,
    pub loudness_range_lu: f32,
    pub max_true_peak_dbtp: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryQcReport {
    pub passed: bool,
    pub failures: Vec<String>,
}

pub fn audit_loudness(
    measurements: LoudnessMeasurements,
    requirements: LoudnessRequirements,
) -> DeliveryQcReport {
    let mut failures = Vec::new();
    if !requirements.validate() || !measurements_valid(measurements) {
        failures.push("invalid loudness measurements or requirements".into());
    } else {
        if (measurements.integrated_lufs - requirements.integrated_lufs).abs()
            > requirements.integrated_tolerance_lu
        {
            failures.push("integrated loudness is outside tolerance".into());
        }
        if measurements.max_true_peak_dbtp > requirements.max_true_peak_dbtp {
            failures.push("true peak exceeds limit".into());
        }
        if requirements
            .max_short_term_lufs
            .is_some_and(|limit| measurements.max_short_term_lufs > limit)
        {
            failures.push("short-term loudness exceeds limit".into());
        }
        if requirements
            .max_momentary_lufs
            .is_some_and(|limit| measurements.max_momentary_lufs > limit)
        {
            failures.push("momentary loudness exceeds limit".into());
        }
        if requirements
            .max_loudness_range_lu
            .is_some_and(|limit| measurements.loudness_range_lu > limit)
        {
            failures.push("loudness range exceeds limit".into());
        }
    }
    DeliveryQcReport {
        passed: failures.is_empty(),
        failures,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryArtifact {
    pub filename: String,
    pub byte_len: u64,
    pub sha256: String,
}

impl DeliveryArtifact {
    pub fn from_bytes(filename: &str, bytes: &[u8]) -> Option<Self> {
        if !safe_delivery_filename(filename) || bytes.is_empty() {
            return None;
        }
        Some(Self {
            filename: filename.into(),
            byte_len: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    pub fn verify(&self, bytes: &[u8]) -> bool {
        self.byte_len == bytes.len() as u64
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self
                .sha256
                .eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(bytes)))
    }
}

pub(crate) fn bounded_text(value: &str, max: usize, required: bool) -> bool {
    (!required || !value.trim().is_empty()) && value.len() <= max && !value.contains('\0')
}

pub(crate) fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u16>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u8>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u8>() else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=max_day).contains(&day)
}

pub(crate) fn valid_time(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 3
        && parts[0].parse::<u8>().is_ok_and(|hour| hour < 24)
        && parts[1].parse::<u8>().is_ok_and(|minute| minute < 60)
        && parts[2].parse::<u8>().is_ok_and(|second| second < 60)
}

pub(crate) fn optional_level(value: Option<f32>, min: f32, max: f32) -> bool {
    value.is_none_or(|value| value.is_finite() && (min..=max).contains(&value))
}

pub(crate) fn measurements_valid(value: LoudnessMeasurements) -> bool {
    [
        value.integrated_lufs,
        value.max_short_term_lufs,
        value.max_momentary_lufs,
        value.max_true_peak_dbtp,
    ]
    .into_iter()
    .all(|level| level.is_finite() && (-200.0..=24.0).contains(&level))
        && value.loudness_range_lu.is_finite()
        && (0.0..=200.0).contains(&value.loudness_range_lu)
}

pub(crate) fn safe_delivery_filename(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 512
        && !value.contains('\0')
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
}
