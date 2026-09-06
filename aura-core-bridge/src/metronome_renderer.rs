pub struct MetronomeOrchestrator {
    pub is_enabled: bool,
    pub sample_rate: f64,
}

const MIN_METRONOME_SAMPLE_RATE: f64 = 8_000.0;
const MAX_METRONOME_SAMPLE_RATE: f64 = 384_000.0;

impl Default for MetronomeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl MetronomeOrchestrator {
    pub fn new() -> Self {
        Self {
            is_enabled: false,
            sample_rate: 44100.0,
        }
    }

    /**
     * @brief PROCESS: Rhythmic click signal generation with sample-accurate, lock-free precision.
     * INDUSTRIAL: Stateless computation based purely on the playhead sample offset.
     *  - Downbeat (beat 0 of bar): 1200 Hz click.
     *  - Upbeat (beats 1-3): 800 Hz click.
     *  - Exponential decay envelope: e^(-60 * t) (~80ms duration).
     */
    pub fn process_metronome(&self, out_l: &mut [f32], out_r: &mut [f32], playhead: u64, bpm: f64) {
        if !self.is_enabled
            || !self.sample_rate.is_finite()
            || !(MIN_METRONOME_SAMPLE_RATE..=MAX_METRONOME_SAMPLE_RATE).contains(&self.sample_rate)
            || !bpm.is_finite()
            || bpm <= 0.0
        {
            return;
        }

        let n = out_l.len().min(out_r.len());
        if n == 0 {
            return;
        }

        let samples_per_beat = (60.0 / bpm) * self.sample_rate;
        let click_duration_samples = (self.sample_rate * 0.08) as u64; // 80ms duration

        for i in 0..n {
            let abs_sample = playhead + i as u64;

            // Find the immediate preceding beat trigger position
            let prev_beat_idx = (abs_sample as f64 / samples_per_beat).floor() as u64;
            let prev_beat_sample = (prev_beat_idx as f64 * samples_per_beat) as u64;
            let offset_samples = abs_sample - prev_beat_sample;

            if offset_samples < click_duration_samples {
                let t = offset_samples as f64 / self.sample_rate;
                let is_downbeat = prev_beat_idx.is_multiple_of(4);
                let freq = if is_downbeat { 1200.0 } else { 800.0 };

                // exponential decay envelope
                let env = (-65.0 * t).exp() as f32;
                let wave = (2.0 * std::f64::consts::PI * freq * t).sin() as f32 * env * 0.2f32;

                out_l[i] =
                    ((if out_l[i].is_finite() { out_l[i] } else { 0.0 }) + wave).clamp(-1.0, 1.0);
                out_r[i] =
                    ((if out_r[i].is_finite() { out_r[i] } else { 0.0 }) + wave).clamp(-1.0, 1.0);
            }
        }
    }

    pub fn audit_metronome(&self) -> bool {
        self.sample_rate.is_finite()
            && (MIN_METRONOME_SAMPLE_RATE..=MAX_METRONOME_SAMPLE_RATE)
                .contains(&self.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::MetronomeOrchestrator;

    #[test]
    fn invalid_tempo_or_sample_rate_is_silent_and_safe() {
        let mut metronome = MetronomeOrchestrator::new();
        metronome.is_enabled = true;
        metronome.sample_rate = 0.0;
        let mut left = [f32::NAN; 8];
        let mut right = [f32::INFINITY; 8];
        metronome.process_metronome(&mut left, &mut right, 0, 0.0);
        assert!(left.iter().all(|sample| sample.is_nan()));
        assert!(right.iter().all(|sample| sample.is_infinite()));

        metronome.sample_rate = 48_000.0;
        metronome.process_metronome(&mut left, &mut right, 0, f64::NAN);
        assert!(metronome.audit_metronome());

        metronome.sample_rate = 1.0;
        assert!(!metronome.audit_metronome());
        metronome.process_metronome(&mut left, &mut right, 0, 120.0);
        assert!(left.iter().all(|sample| sample.is_nan()));
        assert!(right.iter().all(|sample| sample.is_infinite()));
    }
}
