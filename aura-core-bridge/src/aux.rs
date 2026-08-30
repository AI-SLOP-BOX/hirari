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
        let gain = if config.gain.is_finite() { config.gain } else { 0.0 };
        let multiplier = if config.phase_invert {
            -gain
        } else {
            gain
        };

        // INDUSTRIAL: Simple panning law (Linear for now, but in production this would be Sin/Cos).
        let pan = if config.pan.is_finite() { config.pan.clamp(-1.0, 1.0) } else { 0.0 };
        let pan_l = (1.0 - pan).clamp(0.0, 1.0);
        let pan_r = (1.0 + pan).clamp(0.0, 1.0);

        for ((target_left, target_right), (source_left, source_right)) in target_l.iter_mut()
            .zip(target_r.iter_mut()).zip(source_l.iter().zip(source_r.iter())) {
            *target_left = source_left * multiplier * pan_l;
            *target_right = source_right * multiplier * pan_r;
        }
    }
}
