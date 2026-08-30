/**
 * @struct VocalSynthKernel
 * @brief Professional formant-based vocal synthesis engine.
 * INDUSTRIAL: Simulates human vocal tract resonances (F1, F2, F3) using 
 * high-precision resonant filters for natural vowel synthesis.
 */
pub struct VocalSynthKernel {
    pub sample_rate: f64,
}

impl VocalSynthKernel {
    pub fn new(sr: f64) -> Self {
        Self { sample_rate: sr }
    }

    /**
     * @brief SYNTHESIZE: Generates a vocal sound for a given pitch and vowel.
     * INDUSTRIAL: Beyond wavetable synthesis, this uses spectral shaping 
     * to recreate the unique acoustic signature of human vowels.
     * Vowels: 0=A, 1=E, 2=I, 3=O, 4=U
     */
    pub fn synthesize_vowel(&self, frequency: f32, vowel_idx: u32) -> Vec<f32> {
        // INDUSTRIAL: Formant frequency targets (Standard Human Reference).
        let formants = match vowel_idx {
            0 => [800.0, 1200.0, 2500.0], // A
            1 => [400.0, 2200.0, 3000.0], // E
            2 => [250.0, 2400.0, 3200.0], // I
            3 => [450.0, 800.0, 2800.0],  // O
            4 => [300.0, 700.0, 2600.0],  // U
            _ => [500.0, 1500.0, 2500.0], // Neutral
        };

        const LENGTH: usize = 1024;

        // A bad sample rate must not reach the trigonometric/filter coefficient
        // calculations.  Returning the correctly-sized buffer preserves the API
        // contract while keeping the result finite.
        if !self.sample_rate.is_finite() || self.sample_rate <= 100.0 {
            return vec![0.0; LENGTH];
        }

        let sr = self.sample_rate as f32;
        let max_pitch = (sr * 0.45).max(40.0);
        let pitch = if frequency.is_finite() && frequency > 0.0 {
            frequency.clamp(40.0, max_pitch)
        } else {
            140.0_f32.min(sr * 0.25)
        };
        let phase_step = pitch / sr;

        // Three parallel, lightly damped resonators form the vocal-tract
        // envelope.  Their bandwidths are intentionally broad enough for a
        // short 1024-sample render and avoid allocating filter objects.
        let mut states = [[0.0_f32; 2]; 3];
        let mut coefficients = [[0.0_f32; 3]; 3]; // b0, 2*r*cos(w), -r*r
        for (i, &formant) in formants.iter().enumerate() {
            let f = formant.min(sr * 0.45).max(20.0);
            let radius = (-std::f32::consts::PI * (90.0 + f * 0.045) / sr).exp();
            let angle = 2.0 * std::f32::consts::PI * f / sr;
            coefficients[i] = [1.0 - radius, 2.0 * radius * angle.cos(), -radius * radius];
        }

        let mut output = vec![0.0; LENGTH];
        let mut phase = 0.0_f32;
        for sample in &mut output {
            // A short, differentiated glottal pulse: positive opening followed
            // by a softer negative closing phase, with a small aspiration tail.
            let pulse = if phase < 0.18 {
                (phase / 0.18 * std::f32::consts::PI).sin()
            } else if phase < 0.35 {
                -0.35 * ((phase - 0.18) / 0.17 * std::f32::consts::PI).sin()
            } else {
                0.0
            };
            let excitation = pulse + 0.012 * (2.0 * std::f32::consts::PI * phase).sin();

            let mut shaped = 0.0;
            for (i, state) in states.iter_mut().enumerate() {
                let c = coefficients[i];
                let y = c[0] * excitation + c[1] * state[0] + c[2] * state[1];
                state[1] = state[0];
                state[0] = y;
                shaped += y * [1.0, 0.72, 0.42][i];
            }
            *sample = (shaped * 0.22).clamp(-1.0, 1.0);
            phase += phase_step;
            if phase >= 1.0 {
                phase -= phase.floor();
            }
        }
        output
    }

    pub fn audit_vocal_synth(&self) -> bool {
        self.sample_rate.is_finite() && self.sample_rate > 100.0
    }
}
