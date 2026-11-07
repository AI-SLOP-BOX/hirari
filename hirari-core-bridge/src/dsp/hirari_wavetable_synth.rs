use crate::wavetable_oscillator::WavetableOscillatorEngine;

pub struct SimpleAdsr {
    pub level: f32,
    pub target: f32,
}
impl Default for SimpleAdsr {
    fn default() -> Self {
        Self::new()
    }
}
impl SimpleAdsr {
    pub fn new() -> Self {
        Self {
            level: 0.0,
            target: 0.0,
        }
    }
    pub fn get_next(&mut self) -> f32 {
        self.level += (self.target - self.level) * 0.001;
        self.level
    }
    pub fn trigger(&mut self) {
        self.target = 1.0;
    }
    pub fn release(&mut self) {
        self.target = 0.0;
    }
    pub fn reset(&mut self) {
        self.level = 0.0;
        self.target = 0.0;
    }
}

pub struct HirariWavetableSynthEngine {
    pub sample_rate: f64,
    pub osc: WavetableOscillatorEngine,
    pub aeg: SimpleAdsr,
    pub feg: SimpleAdsr,
    pub morph_pos: f32,
    pub cutoff1: f32,
    pub res1: f32,
    pub cutoff2: f32,
    pub res2: f32,
    pub drive: f32,
    pub feedback: f32,
    pub f1_z1: f32,
    pub f2_z1: f32,
    pub hpf_z1: f32,
    pub last_in: f32,
    pub last_f2_out: f32,
    pub velocity: f32,
}
impl HirariWavetableSynthEngine {
    pub fn new(sample_rate: f64) -> Self {
        // The C++ oscillator historically generated its mip tables at the
        // default rate before the synth applied the requested playback rate.
        let mut osc = WavetableOscillatorEngine::new(44_100.0);
        osc.set_sample_rate(sample_rate);
        Self {
            sample_rate,
            osc,
            aeg: SimpleAdsr::new(),
            feg: SimpleAdsr::new(),
            morph_pos: 0.5,
            cutoff1: 1000.0,
            res1: 0.2,
            cutoff2: 2000.0,
            res2: 0.1,
            drive: 1.0,
            feedback: 0.1,
            f1_z1: 0.0,
            f2_z1: 0.0,
            hpf_z1: 0.0,
            last_in: 0.0,
            last_f2_out: 0.0,
            velocity: 0.0,
        }
    }
    pub fn note_on(&mut self, freq: f64, vel: f32) {
        if !freq.is_finite() || !vel.is_finite() {
            return;
        }
        self.osc.set_frequency(freq);
        self.velocity = vel.clamp(0.0, 1.0);
        self.aeg.trigger();
        self.feg.trigger();
    }
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        if l.len() != r.len()
            || !self.sample_rate.is_finite()
            || !crate::dsp_limits::is_supported_sample_rate(self.sample_rate)
        {
            return;
        }
        for i in 0..l.len() {
            let out = self.next_sample();
            l[i] = out;
            r[i] = out;
        }
    }
    fn next_sample(&mut self) -> f32 {
        let sr = self.sample_rate as f32;
        let morph = self.morph_pos.clamp(0.0, 1.0);
        let cutoff1 = self.cutoff1.clamp(20.0, sr * 0.45);
        let cutoff2 = self.cutoff2.clamp(20.0, sr * 0.45);
        let resonance1 = self.res1.clamp(0.0, 0.95);
        let resonance2 = self.res2.clamp(0.0, 0.95);
        let feedback = self.feedback.clamp(0.0, 0.8);
        let drive = self.drive.clamp(0.1, 8.0);
        let env = self.aeg.get_next().clamp(0.0, 1.0);
        let filter_env = self.feg.get_next().clamp(0.0, 1.0);
        let osc = self.osc.process(morph) * self.velocity * env;
        let f1 = Self::filter(
            osc + feedback * self.last_f2_out,
            &mut self.f1_z1,
            cutoff1 * (0.5 + filter_env),
            resonance1,
            sr,
        );
        let f2 = Self::filter(
            f1,
            &mut self.f2_z1,
            cutoff2 * (0.5 + filter_env),
            resonance2,
            sr,
        );
        self.last_f2_out = f2;
        let output = ((f2 + 0.2 * f1) * drive).tanh() * 0.65;
        if output.is_finite() {
            output
        } else {
            0.0
        }
    }
    fn filter(input: f32, state: &mut f32, cutoff: f32, resonance: f32, sr: f32) -> f32 {
        let coefficient = 1.5 * (std::f32::consts::PI * cutoff / sr).sin();
        let q = 1.0 - resonance;
        *state += coefficient * (input - *state + q * (input - *state));
        *state
    }
    pub fn reset(&mut self) {
        self.aeg.reset();
        self.feg.reset();
        self.f1_z1 = 0.0;
        self.f2_z1 = 0.0;
        self.hpf_z1 = 0.0;
        self.last_in = 0.0;
        self.last_f2_out = 0.0;
        self.velocity = 0.0;
    }
    pub fn audit_hirari_wavetable_synth(&self) -> bool {
        self.sample_rate.is_finite()
            && crate::dsp_limits::is_supported_sample_rate(self.sample_rate)
            && [
                self.morph_pos,
                self.cutoff1,
                self.res1,
                self.cutoff2,
                self.res2,
                self.drive,
                self.feedback,
                self.velocity,
                self.f1_z1,
                self.f2_z1,
                self.hpf_z1,
                self.last_in,
                self.last_f2_out,
            ]
            .iter()
            .all(|v| v.is_finite())
            && (0.0..=1.0).contains(&self.morph_pos)
            && (0.0..=1.0).contains(&self.res1)
            && (0.0..=1.0).contains(&self.res2)
            && (0.0..=1.0).contains(&self.feedback)
            && self.drive >= 0.0
    }
}

#[repr(C)]
pub struct HirariWavetableMidiEvent {
    pub(crate) sample_offset: u64,
    pub(crate) size: u32,
    pub(crate) data: [u8; 256],
    pub(crate) articulation_id: u8,
    pub(crate) _padding: [u8; 3],
}
const _: () = assert!(std::mem::size_of::<HirariWavetableMidiEvent>() == 272);

