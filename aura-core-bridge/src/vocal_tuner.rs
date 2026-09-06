#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocalScale {
    Chromatic,
    Major,
    NaturalMinor,
}

pub struct VocalPitchCorrectorEngine {
    pub sample_rate: f64,
    pub buffer: Vec<f32>,
    pub input_pos: usize,
    pub detected_freq: f32,
    pub target_ratio: f32,
    pub phase: f64,
    pub window_size: f32,
    pub amount: f32,
    pub speed: f32,
    pub scale: VocalScale,
}

impl VocalPitchCorrectorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                sr
            } else {
                44_100.0
            },
            buffer: vec![0.0; 8192],
            input_pos: 0,
            detected_freq: 440.0,
            target_ratio: 1.0,
            phase: 0.0,
            window_size: 1024.0,
            amount: 1.0,
            speed: 0.1,
            scale: VocalScale::Chromatic,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.target_ratio = 1.0;
        self.phase = 0.0;
        self.input_pos = 0;
    }

    pub fn set_sample_rate(&mut self, sr: f64) {
        if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            self.sample_rate = sr;
            self.reset();
        }
    }

    pub fn set_params(&mut self, amount: f32, speed: f32, scale: VocalScale) {
        if amount.is_finite() {
            self.amount = amount.clamp(0.0, 1.0);
        }
        if speed.is_finite() {
            self.speed = speed.clamp(0.0, 1.0);
        }
        self.scale = scale;
    }

    fn read_buffer(&self, phase: f64) -> f32 {
        let mut read_pos = self.input_pos as f64 - phase;
        while read_pos < 0.0 {
            read_pos += self.buffer.len() as f64;
        }

        let i1 = read_pos as usize % self.buffer.len();
        let i2 = (i1 + 1) % self.buffer.len();
        let frac = (read_pos - read_pos.floor()) as f32;

        (1.0 - frac) * self.buffer[i1] + frac * self.buffer[i2]
    }

    /**
     * @brief DETECT: High-stability autocorrelation-based pitch tracking.
     * INDUSTRIAL: Replaces volatile zero-crossing counting with rigorous autocorrelation,
     * suppressing noise, harmonics, and breath sibilance to find true vocal fundamental frequency.
     */
    fn detect_pitch(&self) -> f32 {
        let len = self.buffer.len();
        let size = 512; // Window size for correlation
        if len <= size + 2
            || !self.sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&self.sample_rate)
        {
            return self.detected_freq;
        }

        // Human voice pitch ranges between 80Hz and 1000Hz.
        // Lag tau in samples: fs/1000 to fs/80.
        // For 44.1kHz: lag range is ~44 to ~550 samples.
        let min_lag = ((self.sample_rate / 1000.0) as usize).max(1);
        let max_lag = ((self.sample_rate / 80.0) as usize).min(len.saturating_sub(2));
        if min_lag >= max_lag {
            return self.detected_freq;
        }

        let mut r = vec![0.0f32; max_lag + 1];

        // 1. Calculate autocorrelation for each lag
        for tau in min_lag..=max_lag {
            let mut sum = 0.0f32;
            let mut energy_a = 0.0f32;
            let mut energy_b = 0.0f32;
            for n in 0..size {
                let idx1 = (self.input_pos + len - size - max_lag + n) % len;
                let idx2 = (idx1 + tau) % len;
                let a = self.buffer[idx1];
                let b = self.buffer[idx2];
                sum += a * b;
                energy_a += a * a;
                energy_b += b * b;
            }
            let denominator = (energy_a * energy_b).sqrt();
            r[tau] = if denominator.is_finite() && denominator > f32::EPSILON {
                sum / denominator
            } else {
                0.0
            };
        }

        // 2. Find the highest local peak inside our lag range
        let mut best_lag = 0;
        let mut max_val = -1e9f32;

        for tau in min_lag..=max_lag {
            // Peak condition: local maximum
            if r[tau] > max_val && r[tau] > r[tau - 1] && r[tau] > r[tau + 1] {
                max_val = r[tau];
                best_lag = tau;
            }
        }

        // Prefer the earliest strong periodic peak. Autocorrelation often
        // rates a subharmonic (e.g. 110 Hz for a 440 Hz tone) slightly higher;
        // selecting the first peak within 92% of the global maximum preserves
        // the fundamental while retaining robustness for breathy voices.
        if best_lag > 0 && max_val.is_finite() {
            let threshold = max_val * 0.92;
            for tau in min_lag..=max_lag {
                if r[tau] >= threshold && (tau == min_lag || r[tau] >= r[tau - 1]) {
                    best_lag = tau;
                    break;
                }
            }
        }

        if best_lag == 0 {
            return self.detected_freq; // Fallback to previous
        }

        let mut refined_lag = best_lag as f32;
        if best_lag > min_lag && best_lag < max_lag {
            let ym = r[best_lag - 1];
            let y0 = r[best_lag];
            let yp = r[best_lag + 1];
            let denominator = ym - 2.0 * y0 + yp;
            if denominator.is_finite() && denominator.abs() > f32::EPSILON {
                refined_lag += 0.5 * (ym - yp) / denominator;
            }
        }
        let refined_lag = refined_lag.clamp(min_lag as f32, max_lag as f32);
        let freq = self.sample_rate as f32 / refined_lag;
        if (60.0..=1200.0).contains(&freq) {
            freq
        } else {
            self.detected_freq
        }
    }

    /**
     * @brief SCALE: Snaps raw MIDI note frequency to Major, Minor, or Chromatic scale.
     * INDUSTRIAL: Provides correct harmonic snapping context to prevent out-of-scale tuning artifacts.
     */
    fn get_nearest_scale_freq(&self, f: f32) -> f32 {
        if f < 20.0 {
            return 20.0;
        }

        // Convert to absolute MIDI float
        let midi = 12.0 * (f / 440.0).log2() + 69.0;
        let note = midi.round() as i32;
        let octave = note / 12;
        let pitch_in_octave = note % 12;

        // Scale bitmasks
        let mask = match self.scale {
            VocalScale::Chromatic => 0b111111111111,
            VocalScale::Major => 0b101010110101, // C D E F G A B
            VocalScale::NaturalMinor => 0b101101011010, // C D Eb F G Ab Bb
        };

        // Find nearest enabled note
        let mut best_note = pitch_in_octave;
        let mut min_dist = 999;
        for offset in -6..=6 {
            let test_note = (pitch_in_octave + offset + 24) % 12;
            if (mask & (1 << test_note)) != 0 {
                let dist = offset.abs();
                if dist < min_dist {
                    min_dist = dist;
                    best_note = test_note;
                }
            }
        }

        let snapped_midi = (octave * 12 + best_note) as f32;
        440.0 * 2.0f32.powf((snapped_midi - 69.0) / 12.0)
    }

    /// INDUSTRIAL: Performs real-time sample-accurate scale-corrected vocal pitch shifting.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        let buffer_len = self.buffer.len();
        if len == 0
            || buffer_len == 0
            || !self.sample_rate.is_finite()
            || !(8_000.0..=384_000.0).contains(&self.sample_rate)
        {
            return;
        }
        if !self.window_size.is_finite()
            || self.window_size < 2.0
            || self.window_size > buffer_len as f32
        {
            self.window_size = 1024.0_f32.min(buffer_len as f32).max(2.0);
            self.phase = 0.0;
        }
        self.amount = if self.amount.is_finite() {
            self.amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.speed = if self.speed.is_finite() {
            self.speed.clamp(0.0, 1.0)
        } else {
            0.0
        };

        for s in 0..len {
            let in_val = ((if l[s].is_finite() { l[s] } else { 0.0 })
                + if r[s].is_finite() { r[s] } else { 0.0 })
                * 0.5;

            // 1. Buffer incoming audio for analysis (20ms windows)
            self.buffer[self.input_pos] = in_val;
            self.input_pos = (self.input_pos + 1) % buffer_len;

            // 2. High-stability pitch detection every 512 samples
            if self.input_pos.is_multiple_of(512) {
                self.detected_freq = self.detect_pitch();
            }

            // 3. Compare to scale snapped target frequency
            let nearest_freq = self.get_nearest_scale_freq(self.detected_freq);
            let ratio = (nearest_freq / (self.detected_freq + 1e-9)).clamp(0.25, 4.0);

            // 4. Smooth Ratio (Auto-Tune speed)
            self.target_ratio =
                ((1.0 - self.speed) * self.target_ratio + self.speed * ratio).clamp(0.25, 4.0);

            // 5. Dual-tap delay-line pitch shifting with cosine window crossfading
            self.phase += (self.target_ratio - 1.0) as f64;
            if self.phase >= self.window_size as f64 {
                self.phase -= self.window_size as f64;
            }
            if self.phase < 0.0 {
                self.phase += self.window_size as f64;
            }

            let tap1 = self.read_buffer(self.phase);
            let tap2 = self.read_buffer(
                (self.phase + self.window_size as f64 * 0.5) % self.window_size as f64,
            );

            let window1 = 0.5
                * (1.0 - (2.0 * std::f64::consts::PI * self.phase / self.window_size as f64).cos())
                    as f32;
            let window2 = 1.0 - window1;

            let shifted = tap1 * window1 + tap2 * window2;

            // Apply correction amount
            let output = in_val * (1.0 - self.amount) + shifted * self.amount;

            l[s] = output.clamp(-4.0, 4.0);
            r[s] = l[s];
        }
    }

    pub fn audit_vocal_tuner(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && !self.buffer.is_empty()
            && self.input_pos < self.buffer.len()
            && self.detected_freq.is_finite()
            && self.target_ratio.is_finite()
            && self.phase.is_finite()
            && self.amount.is_finite()
            && (0.0..=1.0).contains(&self.amount)
            && self.speed.is_finite()
            && (0.0..=1.0).contains(&self.speed)
    }
}

#[cfg(test)]
mod tests {
    use super::{VocalPitchCorrectorEngine, VocalScale};

    #[test]
    fn vocal_tuner_keeps_ratio_bounded_and_handles_short_stereo_buffers() {
        let mut tuner = VocalPitchCorrectorEngine::new(48_000.0);
        tuner.set_params(1.0, 1.0, VocalScale::Chromatic);
        let mut left = vec![0.5_f32; 2_048];
        let mut right = vec![0.25_f32; 1_024];
        tuner.process(&mut left, &mut right);
        assert!(left[..1_024]
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite() && sample.abs() <= 4.0));
        assert!(tuner.audit_vocal_tuner());
        assert!((0.25..=4.0).contains(&tuner.target_ratio));
    }

    #[test]
    fn tuner_rejects_nonfinite_parameter_updates() {
        let mut tuner = VocalPitchCorrectorEngine::new(f64::NAN);
        assert!(tuner.audit_vocal_tuner());
        tuner.set_params(f32::NAN, f32::INFINITY, VocalScale::Major);
        assert!(tuner.audit_vocal_tuner());
        tuner.set_params(4.0, -2.0, VocalScale::NaturalMinor);
        assert_eq!(tuner.amount, 1.0);
        assert_eq!(tuner.speed, 0.0);
        tuner.set_sample_rate(96_000.0);
        assert_eq!(tuner.sample_rate, 96_000.0);
        tuner.set_sample_rate(f64::INFINITY);
        assert_eq!(tuner.sample_rate, 96_000.0);
    }

    #[test]
    fn detector_tracks_fractional_period() {
        let mut tuner = VocalPitchCorrectorEngine::new(48_000.0);
        tuner.set_params(0.0, 1.0, VocalScale::Chromatic);
        let mut left = vec![0.0_f32; 4096];
        let mut right = vec![0.0_f32; 4096];
        for (index, sample) in left.iter_mut().enumerate() {
            *sample = (2.0 * std::f32::consts::PI * 440.0 * index as f32 / 48_000.0).sin();
        }
        right.copy_from_slice(&left);
        tuner.process(&mut left, &mut right);
        assert!(
            (tuner.detected_freq - 440.0).abs() < 12.0,
            "detected {}",
            tuner.detected_freq
        );
    }
}
