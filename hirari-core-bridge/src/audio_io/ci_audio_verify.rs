//! Deterministic render assertions for CI and headless workflows.

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq)]
pub struct AudioVerification {
    pub sample_count: usize,
    pub peak: f32,
    pub rms: f32,
    pub sha256: String,
}

pub fn verify(
    samples: &[f32],
    max_peak: f32,
    min_rms: f32,
) -> Result<AudioVerification, &'static str> {
    if samples.is_empty()
        || !max_peak.is_finite()
        || max_peak < 0.0
        || !min_rms.is_finite()
        || min_rms < 0.0
    {
        return Err("invalid audio verification limits");
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err("render contains non-finite samples");
    }
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0, f32::max);
    let rms = (samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt() as f32;
    if peak > max_peak {
        return Err("render peak exceeds CI limit");
    }
    if rms < min_rms {
        return Err("render RMS is below CI limit");
    }
    let mut hash = Sha256::new();
    for sample in samples {
        hash.update(sample.to_le_bytes());
    }
    Ok(AudioVerification {
        sample_count: samples.len(),
        peak,
        rms,
        sha256: format!("{:x}", hash.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use super::verify;
    #[test]
    fn verifies_deterministic_render_limits_and_hash() {
        let a = verify(&[0.25, -0.25, 0.5, -0.5], 1.0, 0.1).unwrap();
        let b = verify(&[0.25, -0.25, 0.5, -0.5], 1.0, 0.1).unwrap();
        assert_eq!(a, b);
        assert!(verify(&[1.1], 1.0, 0.0).is_err());
        assert!(verify(&[f32::NAN], 1.0, 0.0).is_err());
    }
}
