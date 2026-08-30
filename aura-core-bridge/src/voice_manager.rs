pub struct Voice {
    pub is_active: bool,
    pub note: u8,
    pub velocity: f32,
    phase: f32,
}

pub struct VoiceManagerEngine {
    pub voices: Vec<Voice>,
    pub max_voices: usize,
}

impl VoiceManagerEngine {
    pub fn new(max_voices: usize) -> Self {
        let max_voices = max_voices.min(4096);
        let mut voices = Vec::with_capacity(max_voices);
        for _ in 0..max_voices {
            voices.push(Voice {
                is_active: false,
                note: 0,
                velocity: 0.0,
                phase: 0.0,
            });
        }
        Self { voices, max_voices }
    }

    pub fn trigger_voice(&mut self, note: u8, velocity: u8) {
        let vel_float = velocity as f32 / 127.0;

        // 1. Check if note is already playing
        for v in &mut self.voices {
            if v.is_active && v.note == note {
                v.velocity = vel_float;
                return;
            }
        }

        // 2. Find free voice
        for v in &mut self.voices {
            if !v.is_active {
                v.is_active = true;
                v.note = note;
                v.velocity = vel_float;
                v.phase = 0.0;
                return;
            }
        }

        // 3. Voice Stealing (Simplistic: steal the first one)
        if !self.voices.is_empty() {
            let v = &mut self.voices[0];
            v.is_active = true;
            v.note = note;
            v.velocity = vel_float;
            v.phase = 0.0;
        }
    }

    pub fn release_voice(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.is_active && v.note == note {
                // In a real ADSR, this would trigger release phase.
                // Here we just mark it inactive for simplicity.
                v.is_active = false;
                v.phase = 0.0;
            }
        }
    }

    /// INDUSTRIAL: Coordinates voice rendering.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        // Keep the bridge self-contained: each active voice contributes a
        // bounded sine wave to the shared output buffers.  `zip` also makes
        // mismatched channel lengths safe without allocating a temporary
        // buffer.
        const SAMPLE_RATE: f32 = 44_100.0;
        const TWO_PI: f32 = core::f32::consts::TAU;
        const VOICE_GAIN: f32 = 0.1;

        if l.is_empty() || r.is_empty() {
            return;
        }

        for voice in &mut self.voices {
            if !voice.is_active {
                continue;
            }

            let frequency = 440.0 * 2.0f32.powf((voice.note as f32 - 69.0) / 12.0);
            if !frequency.is_finite() {
                continue;
            }

            let gain = voice.velocity.clamp(0.0, 1.0) * VOICE_GAIN;
            let phase_step = frequency / SAMPLE_RATE;
            for (left, right) in l.iter_mut().zip(r.iter_mut()) {
                let sample = (voice.phase * TWO_PI).sin() * gain;
                if sample.is_finite() {
                    *left += sample;
                    *right += sample;
                }
                voice.phase = (voice.phase + phase_step).fract();
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Voice Manager state.
    pub fn audit_voice_manager(&self) -> bool {
        self.voices.len() <= self.max_voices
            && self.voices.iter().all(|voice| {
                voice.velocity.is_finite()
                    && (0.0..=1.0).contains(&voice.velocity)
                    && voice.phase.is_finite()
                    && (0.0..1.0).contains(&voice.phase)
            })
    }
}
