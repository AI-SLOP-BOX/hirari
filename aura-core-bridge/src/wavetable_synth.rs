use crate::lfo::{LfoEngine, Waveform};
use crate::wavetable_oscillator::WavetableOscillatorEngine;

pub struct Voice {
    pub active: bool,
    pub note: u8,
    pub velocity: f32,
    pub env: f32,
    pub osc: WavetableOscillatorEngine,
    pub lfo: LfoEngine,
}

pub struct WavetableSynthEngine {
    pub sample_rate: f64,
    pub voices: Vec<Voice>,
}

impl WavetableSynthEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 1.0 { sample_rate } else { 48_000.0 };
        let mut voices = Vec::with_capacity(16);
        for _ in 0..16 {
            let mut lfo = LfoEngine::new(sample_rate as f32);
            lfo.set_frequency(5.0); // 5Hz Default
            voices.push(Voice {
                active: false,
                note: 0,
                velocity: 0.0,
                env: 0.0,
                osc: WavetableOscillatorEngine::new(sample_rate),
                lfo,
            });
        }
        Self {
            sample_rate,
            voices,
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 { self.note_off(note); return; }
        for v in &mut self.voices {
            if !v.active {
                v.active = true;
                v.note = note;
                v.velocity = velocity as f32 / 127.0;
                v.env = 0.0;
                v.osc
                    .set_frequency(440.0 * 2.0f64.powf((note as f64 - 69.0) / 12.0));
                return;
            }
        }
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.active && v.note == note {
                v.note = 0; // Trigger Release
            }
        }
    }

    /// INDUSTRIAL: Processes an audio block with LFO-modulated wavetable synthesis.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let num_samples = l.len().min(r.len());
        if !self.sample_rate.is_finite() || self.sample_rate <= 1.0 { return; }

        for v in &mut self.voices {
            if !v.active {
                continue;
            }

            for s in 0..num_samples {
                // ADSR (Simple)
                v.env += if v.note > 0 { 0.001 } else { -0.001 };
                v.env = v.env.clamp(0.0, 1.0);
                if v.env <= 0.0 && v.note == 0 {
                    v.active = false;
                    break;
                }

                // LFO MODULATION
                let mod_val = v.lfo.process(Waveform::Sine) * 0.5 + 0.5;
                let sample = v.osc.process(mod_val) * v.velocity * v.env;

                let sample = if sample.is_finite() { sample } else { 0.0 };
                let current_l = if l[s].is_finite() { l[s] } else { 0.0 };
                let current_r = if r[s].is_finite() { r[s] } else { 0.0 };
                l[s] = (current_l + sample * 0.7).clamp(-1.0e6, 1.0e6);
                r[s] = (current_r + sample * 0.7).clamp(-1.0e6, 1.0e6);
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Wavetable Synth state.
    pub fn audit_wavetable_synth(&self) -> bool {
        self.sample_rate.is_finite() && self.sample_rate > 1.0
            && self.voices.iter().all(|voice| voice.velocity.is_finite()
                && (0.0..=1.0).contains(&voice.velocity)
                && voice.env.is_finite() && (0.0..=1.0).contains(&voice.env))
    }
}
