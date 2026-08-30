use crate::forensics::{ForensicSeverity, ForensicModule};
use crate::math::ParameterSmoother;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum SynthEngineType {
    VirtualAnalog,
    Additive,
    Spectral,
    Granular,
}

/// Sovereign Multi-Engine Synthesizer [Alchemy Architecture]
/// A high-density synthesis core supporting hybrid audio manifestation.
pub struct SovereignSynth {
    pub engine_type: SynthEngineType,
    pub oscillators: Vec<Oscillator>,
    pub filter: LadderFilter,
    pub sample_rate: u32,
}

pub struct Oscillator {
    pub frequency: f32,
    pub phase: f32,
}

pub struct LadderFilter {
    pub cutoff: ParameterSmoother,
    pub resonance: f32,
    state: [f32; 4],
    sample_rate: u32,
}

impl LadderFilter {
    const MAX_SIGNAL: f32 = 4.0;
    const MAX_FEEDBACK: f32 = 3.5;

    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        Self {
            cutoff: ParameterSmoother::new(1000.0, sample_rate as f32, 10.0),
            resonance: 0.5,
            state: [0.0; 4],
            sample_rate,
        }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        self.process_with_sample_rate(input, self.sample_rate)
    }

    fn process_with_sample_rate(&mut self, input: f32, sample_rate: u32) -> f32 {
        let sample_rate = sample_rate.max(1) as f32;
        let cutoff = self.cutoff.next();
        let cutoff = if cutoff.is_finite() { cutoff } else { 1000.0 };
        let f = (cutoff * 2.0 / sample_rate).clamp(0.0, 1.0);
        let f = if f.is_finite() { f } else { 0.0 };
        let resonance = if self.resonance.is_finite() {
            self.resonance.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let k = (4.0 * resonance)
            .clamp(0.0, Self::MAX_FEEDBACK);
        
        // Moog-style Ladder Filter approximation
        let input = if input.is_finite() {
            input.clamp(-Self::MAX_SIGNAL, Self::MAX_SIGNAL)
        } else {
            0.0
        };
        let feedback = if self.state[3].is_finite() {
            self.state[3].clamp(-Self::MAX_SIGNAL, Self::MAX_SIGNAL)
        } else {
            self.state[3] = 0.0;
            0.0
        };
        let mut x = (input - k * feedback).clamp(-Self::MAX_SIGNAL, Self::MAX_SIGNAL);
        for i in 0..4 {
            let state = if self.state[i].is_finite() {
                self.state[i].clamp(-Self::MAX_SIGNAL, Self::MAX_SIGNAL)
            } else {
                0.0
            };
            let next = (x * f + state * (1.0 - f))
                .clamp(-Self::MAX_SIGNAL, Self::MAX_SIGNAL);
            self.state[i] = next;
            x = next;
        }
        x
    }
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
        let sample_rate = self.sample_rate.max(1) as f32;
        for s in output.iter_mut() {
            let mut val = 0.0;
            for osc in &mut self.oscillators {
                val += (osc.phase * 2.0 * std::f32::consts::PI).sin();
                osc.phase = (osc.phase + osc.frequency / sample_rate) % 1.0;
            }
            
            // Apply Moog Filter
            *s += self.filter.process_with_sample_rate(val, self.sample_rate) * 0.2;
        }
        
        crate::aura_log!(
            ForensicSeverity::Info,
            ForensicModule::Audio,
            "SYNTH: Manifested hybrid frame (Engine: {:?})",
            self.engine_type
        );
    }
}
