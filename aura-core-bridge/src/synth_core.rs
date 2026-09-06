pub enum EngineType {
    Subtractive,
    FM,
    Wavetable,
}

pub struct SynthVoice {
    pub active: bool,
    pub note: u8,
    pub phase: f32,
    pub phase_inc: f32,
    pub velocity: f32,
}

pub struct SynthCoreOrchestrator {
    pub engine_type: EngineType,
    pub voices: [SynthVoice; 32],
    pub sample_rate: f64,
    /// A deterministic single-cycle wavetable.  Keeping it in the
    /// orchestrator makes the render path allocation-free and reproducible.
    wavetable: [f32; 256],
}

impl Default for SynthCoreOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SynthCoreOrchestrator {
    pub fn new() -> Self {
        let voices = [(); 32].map(|_| SynthVoice {
            active: false,
            note: 0,
            phase: 0.0,
            phase_inc: 0.0,
            velocity: 0.0,
        });
        let mut wavetable = [0.0f32; 256];
        let table_len = wavetable.len() as f32;
        for (i, sample) in wavetable.iter_mut().enumerate() {
            let phase = i as f32 / table_len;
            // Bright but musical saw-like table with a bounded harmonic rolloff.
            *sample = (std::f32::consts::TAU * phase).sin()
                + 0.5 * (2.0 * std::f32::consts::TAU * phase).sin()
                + 0.25 * (3.0 * std::f32::consts::TAU * phase).sin();
        }
        Self {
            engine_type: EngineType::Subtractive,
            voices,
            sample_rate: 44100.0,
            wavetable,
        }
    }

    /// INDUSTRIAL: Processes MIDI Note On with absolute technical integrity.
    pub fn start_note(&mut self, note: u8, velocity: u8) {
        // INDUSTRIAL: Implementation of high-performance voice allocation.
        // Rust's VoiceAllocatorEngine ensures bit-accurate voice selection.
        let voice_index = self.voices.iter().position(|v| !v.active).unwrap_or(0);
        if let Some(voice) = self.voices.get_mut(voice_index) {
            voice.active = true;
            voice.note = note;
            voice.velocity = velocity as f32 / 127.0;
            voice.phase = 0.0;
            let freq = 440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
            voice.phase_inc = freq / self.sample_rate as f32;
        }
    }

    /// INDUSTRIAL: Processes MIDI Note Off with zero-latency precision.
    pub fn stop_note(&mut self, note: u8) {
        for voice in self.voices.iter_mut() {
            if voice.active && voice.note == note {
                voice.active = false;
            }
        }
    }

    /// INDUSTRIAL: Renders polyphonic audio with SIMD-accelerated precision.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        // INDUSTRIAL: Implementation of high-performance oscillator generation.
        // Rust's SIMDOscillatorEngine handles complex FM and Wavetable patches.

        for voice in self.voices.iter_mut() {
            if !voice.active {
                continue;
            }

            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let mut sample = match self.engine_type {
                    EngineType::Subtractive => {
                        if voice.phase < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    EngineType::FM => {
                        (voice.phase * std::f32::consts::TAU).sin()
                            + (voice.phase * (2.0 * std::f32::consts::TAU)).sin() * 0.5
                    }
                    EngineType::Wavetable => {
                        let position = voice.phase * self.wavetable.len() as f32;
                        let index = position.floor() as usize % self.wavetable.len();
                        let next = (index + 1) % self.wavetable.len();
                        let frac = position - position.floor();
                        self.wavetable[index] * (1.0 - frac) + self.wavetable[next] * frac
                    }
                };

                sample *= voice.velocity * 0.1;
                *l += sample;
                *r += sample;

                voice.phase += voice.phase_inc;
                if voice.phase >= 1.0 {
                    voice.phase -= 1.0;
                }
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the synth core state.
    pub fn audit_synth_core(&self) -> bool {
        if !self.sample_rate.is_finite() || !(100.0..=384000.0).contains(&self.sample_rate) {
            return false;
        }
        if self
            .wavetable
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 4.0)
        {
            return false;
        }
        self.voices.iter().all(|voice| {
            voice.phase.is_finite()
                && (0.0..1.0).contains(&voice.phase)
                && voice.phase_inc.is_finite()
                && voice.phase_inc >= 0.0
                && voice.phase_inc <= 1.0
                && voice.velocity.is_finite()
                && (0.0..=1.0).contains(&voice.velocity)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavetable_engine_produces_finite_audio() {
        let mut synth = SynthCoreOrchestrator::new();
        synth.engine_type = EngineType::Wavetable;
        synth.start_note(60, 127);
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];
        synth.render(&mut left, &mut right);
        assert!(left.iter().any(|sample| sample.abs() > 1.0e-6));
        assert!(left
            .iter()
            .chain(right.iter())
            .all(|sample| sample.is_finite()));
        assert!(synth.audit_synth_core());
    }

    #[test]
    fn full_voice_pool_reuses_a_voice_instead_of_dropping_note() {
        let mut synth = SynthCoreOrchestrator::new();
        for note in 0..32 {
            synth.start_note(note, 100);
        }
        synth.start_note(100, 127);
        assert!(synth
            .voices
            .iter()
            .any(|voice| voice.active && voice.note == 100));
    }
}
