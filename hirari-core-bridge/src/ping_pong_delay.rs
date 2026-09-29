use crate::delay_line::DelayLineEngine;

pub struct PingPongDelayEngine {
    pub sample_rate: f64,
    pub delay_l: DelayLineEngine,
    pub delay_r: DelayLineEngine,
    pub last_out_l: f32,
    pub last_out_r: f32,
    pub note_value: f32,
    pub feedback_l: f32,
    pub feedback_r: f32,
    pub mix: f32,

    // High-frequency feedback damping 1-pole LPF states
    pub lpf_state_l: f32,
    pub lpf_state_r: f32,
}

impl PingPongDelayEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            delay_l: DelayLineEngine::new(65536),
            delay_r: DelayLineEngine::new(65536),
            last_out_l: 0.0,
            last_out_r: 0.0,
            note_value: 0.25, // Quarter note sync
            feedback_l: 0.5,
            feedback_r: 0.5,
            mix: 0.5,
            lpf_state_l: 0.0,
            lpf_state_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.delay_l.reset();
        self.delay_r.reset();
        self.last_out_l = 0.0;
        self.last_out_r = 0.0;
        self.lpf_state_l = 0.0;
        self.lpf_state_r = 0.0;
    }

    pub fn set_params(&mut self, note_value: f32, feedback: f32, mix: f32) {
        self.note_value = note_value;
        self.feedback_l = feedback.clamp(0.0, 0.99);
        self.feedback_r = feedback.clamp(0.0, 0.99);
        self.mix = mix;
    }

    /**
     * @brief PROCESS: Renders cross-feedback ping-pong echoes.
     * INDUSTRIAL:
     *  - Utilizes smooth fractional delays to prevent aliasing when BPM fluctuates.
     *  - Applies a 1-pole feedback LPF (crossover around 2500 Hz) to create a warm,
     *    highly musical analog tape / BBD echo response.
     */
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], bpm: f64) {
        let len = l.len().min(r.len());
        if !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || !bpm.is_finite()
            || bpm <= 0.0
            || !self.note_value.is_finite()
            || self.note_value <= 0.0
        {
            return;
        }
        let samples_per_beat = (60.0 / bpm) * self.sample_rate;
        let delay_samps = (samples_per_beat * self.note_value as f64) as f32;

        for s in 0..len {
            let in_l = l[s];
            let in_r = r[s];

            // 1. Damped Feedback loop via 1-pole Low-Pass Filter
            // lpf = 0.7 * lpf + 0.3 * current
            let fb_in_l = in_r + self.feedback_l * self.last_out_r;
            self.lpf_state_l = 0.7 * self.lpf_state_l + 0.3 * fb_in_l;

            let fb_in_r = in_l + self.feedback_r * self.last_out_l;
            self.lpf_state_r = 0.7 * self.lpf_state_r + 0.3 * fb_in_r;

            // 2. Fetch Delayed Output (Ping-Pong Cross-Tap)
            let out_l = self.delay_l.process(self.lpf_state_l, delay_samps);
            let out_r = self.delay_r.process(self.lpf_state_r, delay_samps);

            self.last_out_l = out_l;
            self.last_out_r = out_r;

            // 3. Mix Dry/Wet
            let mix = self.mix.clamp(0.0, 1.0);
            l[s] = (in_l * (1.0 - mix) + out_l * mix).clamp(-4.0, 4.0);
            r[s] = (in_r * (1.0 - mix) + out_r * mix).clamp(-4.0, 4.0);
        }
    }

    pub fn audit_ping_pong_delay(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.note_value.is_finite()
            && self.note_value > 0.0
            && self.feedback_l.is_finite()
            && self.feedback_r.is_finite()
            && (0.0..=0.99).contains(&self.feedback_l)
            && (0.0..=0.99).contains(&self.feedback_r)
            && self.mix.is_finite()
            && (0.0..=1.0).contains(&self.mix)
            && self.delay_l.audit_delay_line()
            && self.delay_r.audit_delay_line()
    }
}
