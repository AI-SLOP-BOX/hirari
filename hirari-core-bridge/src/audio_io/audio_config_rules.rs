//! Audio configuration admission.
//!
//! A configuration transition is a stop-the-world control operation, so the
//! same bounds guard the session bridge (tempo + format) and the engine's
//! `prepareToPlay` (format only). Keeping one copy here means an invalid
//! tempo, sample rate or block size can never quiesce the callback in one
//! entry point while being rejected in another.

const MIN_SAMPLE_RATE: f64 = 8_000.0;
const MAX_SAMPLE_RATE: f64 = 384_000.0;
const MAX_BLOCK_SIZE: u32 = 16_384;
const MAX_TEMPO: f32 = 999.0;

pub(crate) fn sample_format_valid(sample_rate: f64, block_size: u32) -> bool {
    sample_rate.is_finite()
        && (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
        && (1..=MAX_BLOCK_SIZE).contains(&block_size)
}

pub(crate) fn apply_config_valid(tempo: f32, sample_rate: f64, block_size: u32) -> bool {
    tempo.is_finite() && tempo > 0.0 && tempo <= MAX_TEMPO
        && sample_format_valid(sample_rate, block_size)
}

/// Format-only negotiation used by `prepareToPlay` before a device is prepared.
#[no_mangle]
pub extern "C" fn hirari_audio_format_valid(sample_rate: f64, block_size: u32) -> bool {
    sample_format_valid(sample_rate, block_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_every_edge_of_the_documented_range() {
        assert!(apply_config_valid(0.001, 8_000.0, 1));
        assert!(apply_config_valid(999.0, 384_000.0, 16_384));
        assert!(sample_format_valid(44_100.0, 512));
    }

    #[test]
    fn rejects_out_of_range_sample_rates_and_block_sizes() {
        assert!(!sample_format_valid(7_999.999, 256));
        assert!(!sample_format_valid(384_000.001, 256));
        assert!(!sample_format_valid(0.0, 256));
        assert!(!sample_format_valid(44_100.0, 0));
        assert!(!sample_format_valid(44_100.0, 16_385));
        assert!(!sample_format_valid(f64::NAN, 256));
        assert!(!sample_format_valid(f64::INFINITY, 256));
    }

    #[test]
    fn rejects_out_of_range_tempos() {
        assert!(!apply_config_valid(0.0, 44_100.0, 256));
        assert!(!apply_config_valid(-1.0, 44_100.0, 256));
        assert!(!apply_config_valid(999.001, 44_100.0, 256));
        assert!(!apply_config_valid(f32::NAN, 44_100.0, 256));
        assert!(!apply_config_valid(f32::INFINITY, 44_100.0, 256));
    }

    #[test]
    fn format_check_ignores_tempo() {
        // prepareToPlay has no tempo argument; only the format is gated.
        assert!(sample_format_valid(48_000.0, 1_024));
        assert!(!apply_config_valid(0.0, 48_000.0, 1_024));
    }
}
