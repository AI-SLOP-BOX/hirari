// ============================================================
// Physical limits shared by every DSP and transport module.
//
// These bounds were previously spelled out inline in ~100 places, which meant a
// change to the supported range (a 384 kHz device, a 768 kHz oversampling rate)
// was a ~100-file edit, and a module that typed the range slightly differently
// diverged silently. One definition, one test.
// ============================================================

/// Lowest sample rate any module accepts, in Hz.
pub const MIN_SAMPLE_RATE: f64 = 8_000.0;

/// Highest sample rate any module accepts, in Hz.
pub const MAX_SAMPLE_RATE: f64 = 384_000.0;

/// Whether a sample rate is one this codebase supports.
///
/// Rejects non-finite values before the range check so a NaN cannot compare
/// its way into acceptance, and so callers do not each re-derive that guard.
#[inline]
pub fn is_supported_sample_rate(sample_rate: f64) -> bool {
    sample_rate.is_finite() && (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
}

/// Upper bound on the number of entries a per-region edit table may hold.
///
/// Shared by the note-edit store, comp ranges, processing steps and the
/// project decoders so a bounded table always means the same thing.
pub const MAX_EDIT_ENTRIES: usize = 4096;

/// Largest operation string a processing step may carry, in bytes.
pub const MAX_OPERATION_BYTES: usize = 256;

/// Cents of pitch offset a note edit may apply. Beyond this a pitch ratio
/// stops being a musical edit and becomes an artifact generator.
pub const MAX_PITCH_CENTS: f64 = 4800.0;

/// Cents of formant offset a note edit may apply.
pub const MAX_FORMANT_CENTS: f64 = 2400.0;

/// Cents per octave, the denominator of every cents-to-ratio conversion here.
pub const CENTS_PER_OCTAVE: f64 = 1200.0;

/// Converts a cents offset into a pitch ratio, clamped to the supported range.
///
/// The clamp is part of the contract rather than the caller's problem: an
/// unclamped ratio below 0.5 or above 4.0 inverts or tears audio, so every
/// caller needs the same one.
#[inline]
pub fn cents_to_pitch_ratio(cents: f64) -> f64 {
    2.0f64.powf(cents.clamp(-MAX_PITCH_CENTS, MAX_PITCH_CENTS) / CENTS_PER_OCTAVE)
}

#[cfg(test)]
mod dsp_limits_tests {
    use super::*;

    #[test]
    fn supported_sample_rate_bounds_are_inclusive() {
        assert!(is_supported_sample_rate(8_000.0));
        assert!(is_supported_sample_rate(44_100.0));
        assert!(is_supported_sample_rate(96_000.0));
        assert!(is_supported_sample_rate(384_000.0));
        for rejected in [7_999.9, 384_000.1, 0.0, -1.0] {
            assert!(
                !is_supported_sample_rate(rejected),
                "{rejected} must be rejected"
            );
        }
    }

    #[test]
    fn non_finite_sample_rates_are_rejected() {
        for rejected in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!is_supported_sample_rate(rejected), "{rejected:?}");
        }
    }

    #[test]
    fn cents_to_ratio_is_identity_at_zero_and_clamped_at_the_edges() {
        assert!((cents_to_pitch_ratio(0.0) - 1.0).abs() < 1.0e-12);
        // One octave up is exactly 2.
        assert!((cents_to_pitch_ratio(CENTS_PER_OCTAVE) - 2.0).abs() < 1.0e-12);
        // 4800 cents is four octaves, so the clamp caps the ratio at 16.
        assert!((cents_to_pitch_ratio(MAX_PITCH_CENTS) - 16.0).abs() < 1.0e-12);
        // Beyond the clamp the ratio stops growing instead of tearing audio.
        assert!((cents_to_pitch_ratio(9_600.0) - 16.0).abs() < 1.0e-12);
        assert!((cents_to_pitch_ratio(-9_600.0) - 1.0 / 16.0).abs() < 1.0e-12);
    }

    #[test]
    fn formant_limit_is_half_the_pitch_limit() {
        assert_eq!(MAX_FORMANT_CENTS * 2.0, MAX_PITCH_CENTS);
    }
}
