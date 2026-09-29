use crate::delay_line::DelayLineEngine;
use crate::lfo::{LfoEngine, Waveform};

pub struct VocalDoublerEngine {
    pub sample_rate: f64,
    pub lfo: LfoEngine,
    pub delay_l: DelayLineEngine,
    pub delay_r: DelayLineEngine,
}

impl VocalDoublerEngine {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) { sr } else { 48_000.0 };
        let mut lfo = LfoEngine::new(sr as f32);
        lfo.set_frequency(0.2); // 0.2 Hz

        let max_delay = (sr * 0.1).round().max(1.0) as u32; // 100ms max

        Self {
            sample_rate: sr,
            lfo,
            delay_l: DelayLineEngine::new(max_delay),
            delay_r: DelayLineEngine::new(max_delay),
        }
    }

    pub fn reset(&mut self) {
        self.lfo.phase = 0.0;
        self.delay_l.reset();
        self.delay_r.reset();
    }

    /// INDUSTRIAL: VocalDoubler: Industrial-standard vocal thickening.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if !self.audit_vocal_doubler() { return; }
        let len = l.len().min(r.len());

        for i in 0..len {
            let in_l = if l[i].is_finite() { l[i] } else { 0.0 };
            let in_r = if r[i].is_finite() { r[i] } else { 0.0 };

            // 1. GENERATE MODULATION (Micro-detuning)
            let mod_val = self.lfo.process(Waveform::Sine);

            // 2. DELAY L/R (Micro-timing offsets)
            // C++: dL = 20.0f + mod * 10.0f; // 10ms - 30ms jitter
            // C++: dR = 25.0f - mod * 12.0f;
            // These are in samples or milliseconds?
            // In C++, `dL` is passed to `m_delayL.read(dL)`.
            // If `read` takes samples, then it's 20-30 samples (very short).
            // If it takes milliseconds, it's 20-30 ms.
            // Let's check `vocal_doubler.hpp` lines 33-34: "10ms - 30ms jitter".
            // So they are in MILLISECONDS!
            // I need to convert them to samples.

            let dl_ms = 20.0 + mod_val * 10.0;
            let dr_ms = 25.0 - mod_val * 12.0;

            let dl_samps = (self.sample_rate as f32) * dl_ms * 0.001;
            let dr_samps = (self.sample_rate as f32) * dr_ms * 0.001;

            // Reusing DelayLineEngine which pushes and pops in one step with fractional linear interpolation.
            let voice_l = self.delay_l.process(in_l, dl_samps);
            let voice_r = self.delay_r.process(in_r, dr_samps);

            // 3. STEREO SUM (Center + Wide Doubles)
            l[i] = (in_l * 0.7 + voice_l * 0.5).clamp(-2.0, 2.0);
            r[i] = (in_r * 0.7 + voice_r * 0.5).clamp(-2.0, 2.0);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Vocal Doubler state.
    pub fn audit_vocal_doubler(&self) -> bool {
        self.sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.lfo.phase.is_finite() && self.delay_l.buffer.len() > 1 && self.delay_r.buffer.len() > 1
    }
}
