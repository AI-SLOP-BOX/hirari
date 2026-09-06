pub struct WavetableOscillatorEngine {
    pub sample_rate: f64,
    pub phase: f64,
    pub phase_inc: f64,
    pub sine_tables: Vec<Vec<f32>>,
    pub saw_tables: Vec<Vec<f32>>,
}

impl WavetableOscillatorEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
        {
            sample_rate
        } else {
            48_000.0
        };
        let k_table_size = 2048;
        let k_num_mip_maps = 10;

        let mut sine_tables = vec![vec![0.0f32; k_table_size]; k_num_mip_maps];
        let mut saw_tables = vec![vec![0.0f32; k_table_size]; k_num_mip_maps];

        for m in 0..k_num_mip_maps {
            // Generate Sine
            for i in 0..k_table_size {
                sine_tables[m][i] =
                    (2.0 * std::f64::consts::PI * i as f64 / k_table_size as f64).sin() as f32;
            }

            // Generate Saw (Band-limited)
            let max_harmonic = ((sample_rate / 2.0) / (20.0 * 2.0f64.powi(m as i32))) as i32;
            let max_harmonic = max_harmonic.clamp(1, 128);

            for i in 0..k_table_size {
                let mut val = 0.0f32;
                for h in 1..=max_harmonic {
                    val += (2.0 * std::f64::consts::PI * h as f64 * i as f64 / k_table_size as f64)
                        .sin() as f32
                        / h as f32;
                }
                saw_tables[m][i] = val * (2.0 / std::f32::consts::PI);
            }
        }

        Self {
            sample_rate,
            phase: 0.0,
            phase_inc: 0.0,
            sine_tables,
            saw_tables,
        }
    }

    pub fn set_frequency(&mut self, freq: f64) {
        if freq.is_finite() && self.sample_rate.is_finite() {
            self.phase_inc = (freq.clamp(-self.sample_rate * 0.49, self.sample_rate * 0.49)
                / self.sample_rate)
                .clamp(-0.49, 0.49);
        }
    }

    /// INDUSTRIAL: RENDER: Morphing with Cubic Hermite Spline Interpolation.
    pub fn process(&mut self, morph_pos: f32) -> f32 {
        if !self.audit_wavetable_oscillator() || !morph_pos.is_finite() {
            return 0.0;
        }
        self.phase += self.phase_inc;
        self.phase = self.phase.rem_euclid(1.0);

        let freq = (self.phase_inc * self.sample_rate) as f32;
        let mip_idx = (freq / 20.0).log2();
        let m1 = (mip_idx as usize).clamp(0, 9);
        let m2 = (m1 + 1).clamp(0, 9);
        let m_mix = (mip_idx - m1 as f32).clamp(0.0, 1.0);

        let k_table_size = 2048;
        let read_idx = self.phase * k_table_size as f64;
        let i1 = read_idx as usize;
        let i0 = (i1 + k_table_size - 1) % k_table_size;
        let i2 = (i1 + 1) % k_table_size;
        let i3 = (i1 + 2) % k_table_size;
        let frac = (read_idx - i1 as f64) as f32;

        let interpolate = |table: &[f32]| {
            let y0 = table[i0];
            let y1 = table[i1];
            let y2 = table[i2];
            let y3 = table[i3];
            let a = (3.0 * (y1 - y2) - y0 + y3) * 0.5;
            let b = 2.0 * y2 + y0 - 2.5 * y1 - 0.5 * y3;
            let c = (y2 - y0) * 0.5;
            ((a * frac + b) * frac + c) * frac + y1
        };

        let val_a = interpolate(&self.sine_tables[m1]) * (1.0 - m_mix)
            + interpolate(&self.sine_tables[m2]) * m_mix;
        let val_b = interpolate(&self.saw_tables[m1]) * (1.0 - m_mix)
            + interpolate(&self.saw_tables[m2]) * m_mix;

        (val_a * (1.0 - morph_pos.clamp(0.0, 1.0)) + val_b * morph_pos.clamp(0.0, 1.0))
            .clamp(-1.0, 1.0)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Wavetable state.
    pub fn audit_wavetable_oscillator(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.phase.is_finite()
            && self.phase >= 0.0
            && self.phase < 1.0
            && self.phase_inc.is_finite()
            && self.phase_inc.abs() <= 0.5
            && self.sine_tables.len() == 10
            && self.saw_tables.len() == 10
            && self.sine_tables.iter().chain(self.saw_tables.iter()).all(|table| {
                table.len() == 2048 && table.iter().all(|sample| sample.is_finite() && sample.abs() <= 2.0)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::WavetableOscillatorEngine;

    #[test]
    fn wavetable_oscillator_bounds_frequency_and_morph() {
        let mut oscillator = WavetableOscillatorEngine::new(48_000.0);
        oscillator.set_frequency(200_000.0);
        let value = oscillator.process(2.0);
        assert!(value.is_finite() && value.abs() <= 1.0);
        oscillator.set_frequency(f64::NAN);
        assert!(oscillator.audit_wavetable_oscillator());
    }

    #[test]
    fn invalid_sample_rate_uses_safe_default() {
        let oscillator = WavetableOscillatorEngine::new(f64::NAN);
        assert_eq!(oscillator.sample_rate, 48_000.0);
        assert!(oscillator.audit_wavetable_oscillator());
    }
}
