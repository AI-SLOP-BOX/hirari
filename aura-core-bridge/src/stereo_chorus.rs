use crate::delay_line::DelayLineEngine;

pub struct StereoChorusEngine {
    pub sample_rate: f64,
    pub delay_l: DelayLineEngine,
    pub delay_r: DelayLineEngine,
    pub lfo_phase: f64,
    pub rate: f32,
    pub mix: f32,
}

impl StereoChorusEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            delay_l: DelayLineEngine::new(8192),
            delay_r: DelayLineEngine::new(8192),
            lfo_phase: 0.0,
            rate: 0.8,
            mix: 0.5,
        }
    }

    pub fn reset(&mut self) {
        self.delay_l.reset();
        self.delay_r.reset();
        self.lfo_phase = 0.0;
    }

    pub fn set_rate(&mut self, r: f32) {
        self.rate = r.clamp(0.1, 5.0);
    }

    pub fn set_mix(&mut self, m: f32) {
        self.mix = m;
    }

    /// INDUSTRIAL: Modulates delay taps to create pitch-fluctuating width.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || !self.rate.is_finite()
            || !self.mix.is_finite()
        {
            return;
        }

        for s in 0..len {
            // 1. Slow LFO (Chorus drift)
            self.lfo_phase += self.rate as f64 / self.sample_rate;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }

            let lfo_l = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * self.lfo_phase).sin();
            let lfo_r = 0.5
                + 0.5
                    * (2.0 * std::f64::consts::PI * self.lfo_phase + 0.5 * std::f64::consts::PI)
                        .sin();

            // 2. Modulate Delay Taps (10ms to 30ms offset)
            let delay_samps_l = ((0.01 + 0.02 * lfo_l) * self.sample_rate) as f32;
            let delay_samps_r = ((0.01 + 0.02 * lfo_r) * self.sample_rate) as f32;

            let in_l = l[s];
            let in_r = r[s];

            // Real-time smooth fractional linear interpolation to suppress zipper noise
            let out_l = self.delay_l.process(in_l, delay_samps_l);
            let out_r = self.delay_r.process(in_r, delay_samps_r);

            let mix = self.mix.clamp(0.0, 1.0);
            l[s] = (in_l * (1.0 - mix) + out_l * mix).clamp(-4.0, 4.0);
            r[s] = (in_r * (1.0 - mix) + out_r * mix).clamp(-4.0, 4.0);
        }
    }

    pub fn audit_stereo_chorus(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.lfo_phase.is_finite()
            && self.rate.is_finite()
            && self.mix.is_finite()
            && (0.0..=1.0).contains(&self.mix)
            && self.delay_l.audit_delay_line()
            && self.delay_r.audit_delay_line()
    }
}
