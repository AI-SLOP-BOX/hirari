use crate::forensics::{ForensicSeverity, ForensicModule};
use crate::math::ParameterSmoother;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum SynthEngineType {
    VirtualAnalog,
    Additive,
    Spectral,
    Granular,
}

pub struct Oscillator {
    pub frequency: f32,
    pub phase: f32,
}

pub struct LadderFilter {
    pub cutoff: ParameterSmoother,
    pub resonance: f32,
    state: [f32; 4],
}

impl LadderFilter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            cutoff: ParameterSmoother::new(1000.0, sample_rate as f32, 10.0),
            resonance: 0.5,
            state: [0.0; 4],
        }
    }
    pub fn process(&mut self, input: f32) -> f32 {
        let f = (self.cutoff.next() * 2.0 / 44100.0).clamp(0.0, 1.0);
        let k = 4.0 * self.resonance;
        let mut x = input - k * self.state[3];
        for i in 0..4 {
            let next = x * f + self.state[i] * (1.0 - f);
            self.state[i] = next;
            x = next;
        }
        x
    }
}

pub struct SovereignSynth {
    pub engine_type: SynthEngineType,
    pub oscillators: Vec<Oscillator>,
    pub filter: LadderFilter,
    pub sample_rate: u32,
}

impl SovereignSynth {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            engine_type: SynthEngineType::VirtualAnalog,
            oscillators: vec![Oscillator { frequency: 440.0, phase: 0.0 }],
            filter: LadderFilter::new(sample_rate),
            sample_rate,
        }
    }
    pub fn render(&mut self, output: &mut [f32]) {
        for s in output.iter_mut() {
            let mut val = 0.0;
            for osc in &mut self.oscillators {
                val += (osc.phase * 2.0 * std::f32::consts::PI).sin();
                osc.phase = (osc.phase + osc.frequency / self.sample_rate as f32) % 1.0;
            }
            *s += self.filter.process(val) * 0.2;
        }
    }
}
