pub struct BusTrackConfig {
    pub gain: f32,
    pub phase_invert: bool,
    pub pan: f32,   // -1.0 to 1.0
    pub width: f32, // 0.0 to 1.0
}

pub struct BusTrackOrchestrator;

impl BusTrackOrchestrator {
    /// INDUSTRIAL: Processes AUX signal with high-precision gain, phase, and spatial control.
    pub fn process_signal(
        &self,
        target_l: &mut [f32],
        target_r: &mut [f32],
        source_l: &[f32],
        source_r: &[f32],
        config: &BusTrackConfig,
    ) {
        let gain = if config.gain.is_finite() {
            config.gain
        } else {
            0.0
        };
        let multiplier = if config.phase_invert { -gain } else { gain };

        // Equal-power balance keeps the center at unity while avoiding a dip when
        // a mono or narrowed signal is moved across the stereo field.
        let pan = if config.pan.is_finite() {
            config.pan.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let pan_l = angle.cos() * std::f32::consts::SQRT_2;
        let pan_r = angle.sin() * std::f32::consts::SQRT_2;
        let width = if config.width.is_finite() {
            config.width.clamp(0.0, 1.0)
        } else {
            1.0
        };

        for ((target_left, target_right), (source_left, source_right)) in target_l
            .iter_mut()
            .zip(target_r.iter_mut())
            .zip(source_l.iter().zip(source_r.iter()))
        {
            let mid = (source_left + source_right) * 0.5;
            let side = (source_left - source_right) * 0.5 * width;
            let widened_left = mid + side;
            let widened_right = mid - side;
            *target_left += widened_left * multiplier * pan_l;
            *target_right += widened_right * multiplier * pan_r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BusTrackConfig, BusTrackOrchestrator};

    #[test]
    fn aux_processing_accumulates_with_equal_power_balance_and_width() {
        let mut left = vec![0.25];
        let mut right = vec![0.25];
        BusTrackOrchestrator.process_signal(
            &mut left,
            &mut right,
            &[0.5],
            &[0.5],
            &BusTrackConfig { gain: 1.0, phase_invert: false, pan: 0.0, width: 1.0 },
        );
        assert!((left[0] - 0.75).abs() < 1e-6);
        assert!((right[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn width_zero_collapses_side_to_mono() {
        let mut left = vec![0.0];
        let mut right = vec![0.0];
        BusTrackOrchestrator.process_signal(
            &mut left,
            &mut right,
            &[1.0],
            &[-1.0],
            &BusTrackConfig { gain: 1.0, phase_invert: false, pan: 0.0, width: 0.0 },
        );
        assert!((left[0] - right[0]).abs() < 1e-6);
    }
}
