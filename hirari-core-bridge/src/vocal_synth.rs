/// Borrowed note tuple consumed by the allocation-bounded vocal timeline
/// renderer. Keeping the shape named avoids repeating a hard-to-read nested
/// tuple across the public synthesis entry points.
pub type VocalNoteReference<'a> = (u64, f32, &'a str, usize, &'a [f32], f32);

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
    /// Render a lyric-sized vocal grain for UTAU/OpenUtau note previews.
    /// The phonetic layer intentionally stays deterministic and allocation
    /// bounded: callers provide the note length and receive one contiguous
    /// sample buffer suitable for a MIDI-note renderer.
    pub fn synthesize_lyric(&self, frequency: f32, lyric: &str, length: usize) -> Vec<f32> {
        self.synthesize_phoneme(frequency, lyric, length)
    }

    /// Render using an explicit UST/USTX phoneme string.  The final token is
    /// the sustained vowel (for example `k a` or `sh i`), while the lyric is
    /// used as a fallback for vowel-only input.
    pub fn synthesize_phoneme(&self, frequency: f32, phoneme: &str, length: usize) -> Vec<f32> {
        if length == 0 || !self.audit_vocal_synth() {
            return Vec::new();
        }
        let tokens: Vec<&str> = phoneme.split_whitespace().collect();
        let vowel = tokens
            .last()
            .map(|token| Self::vowel_index(token))
            .unwrap_or(5);
        let has_consonant_onset = tokens.len() > 1
            || tokens
                .first()
                .is_some_and(|token| token.len() > 1 && Self::vowel_index(token) == 5);
        let grain = self.synthesize_vowel_with_length(frequency, vowel, length);
        if grain.is_empty() {
            return vec![0.0; length];
        }
        let mut output = vec![0.0_f32; length];
        for (i, sample) in output.iter_mut().enumerate() {
            *sample = grain[i];
            let attack = (length / 100).max(1);
            let release = attack.min(length);
            let gain = if i < attack {
                i as f32 / attack as f32
            } else if i >= length - release {
                (length - 1 - i) as f32 / release as f32
            } else {
                1.0
            };
            let mut value = *sample * gain;
            if has_consonant_onset {
                let onset_len = (length / 12).max(1);
                if i < onset_len {
                    // Deterministic xorshift noise gives consonants a short
                    // unvoiced attack without pulling a random source into
                    // the real-time renderer.
                    let mut state = (i as u32)
                        .wrapping_mul(0x9e37_79b9)
                        .wrapping_add(0x6d2b_79f5);
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    let noise = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                    let fade = 1.0 - i as f32 / onset_len as f32;
                    value = (value + noise * 0.035 * fade).clamp(-1.0, 1.0);
                }
            }
            *sample = value;
        }
        output
    }

    /// Render a sequence of UTAU phonemes as one contiguous phrase. Adjacent
    /// notes receive a bounded equal-power crossfade to avoid clicks when the
    /// source score changes lyric or pitch at a sample boundary.
    pub fn synthesize_phrase(&self, notes: &[(f32, &str, usize)]) -> Vec<f32> {
        if notes.is_empty() || notes.len() > 4096 || !self.audit_vocal_synth() {
            return Vec::new();
        }
        let total = notes.iter().try_fold(0usize, |sum, (_, _, length)| {
            sum.checked_add((*length).min(2_000_000))
        });
        let Some(total) = total.filter(|size| *size > 0 && *size <= 2_000_000) else {
            return Vec::new();
        };
        let mut output = Vec::with_capacity(total);
        for (index, (frequency, phoneme, length)) in notes.iter().enumerate() {
            let grain = self.synthesize_phoneme(*frequency, phoneme, (*length).min(2_000_000));
            if grain.is_empty() {
                continue;
            }
            let crossfade = if index == 0 || output.is_empty() {
                0
            } else {
                32.min(output.len()).min(grain.len())
            };
            if crossfade > 0 {
                let start = output.len() - crossfade;
                for i in 0..crossfade {
                    let t = (i + 1) as f32 / (crossfade + 1) as f32;
                    output[start + i] = output[start + i] * (1.0 - t) + grain[i] * t;
                }
                output.extend_from_slice(&grain[crossfade..]);
            } else {
                output.extend_from_slice(&grain);
            }
            if output.len() > 2_000_000 {
                output.truncate(2_000_000);
                break;
            }
        }
        output
    }

    /// Render notes into a sample-timeline buffer. Entries are `(start,
    /// frequency, phoneme, length)` and are mixed with a short boundary
    /// crossfade, allowing an imported UST/USTX score to become audible in
    /// the engine without routing each note through a separate voice.
    pub fn synthesize_note_timeline(
        &self,
        notes: &[(u64, f32, &str, usize)],
        max_samples: usize,
    ) -> Vec<f32> {
        let with_empty_curves: Vec<(u64, f32, &str, usize, &[f32])> = notes
            .iter()
            .map(|(start, frequency, phoneme, length)| {
                (*start, *frequency, *phoneme, *length, &[][..])
            })
            .collect();
        self.synthesize_note_timeline_with_curves(&with_empty_curves, max_samples)
    }

    /// Render a timeline while applying each note's pitch curve in cents.
    pub fn synthesize_note_timeline_with_curves(
        &self,
        notes: &[(u64, f32, &str, usize, &[f32])],
        max_samples: usize,
    ) -> Vec<f32> {
        let with_unity_gain: Vec<VocalNoteReference<'_>> = notes
            .iter()
            .map(|(start, frequency, phoneme, length, curve)| {
                (*start, *frequency, *phoneme, *length, *curve, 1.0)
            })
            .collect();
        self.synthesize_note_timeline_with_curves_and_gain(&with_unity_gain, max_samples)
    }

    /// Timeline renderer with per-note pitch curves and linear gain values.
    pub fn synthesize_note_timeline_with_curves_and_gain(
        &self,
        notes: &[VocalNoteReference<'_>],
        max_samples: usize,
    ) -> Vec<f32> {
        if notes.is_empty()
            || notes.len() > 4096
            || max_samples == 0
            || max_samples > 2_000_000
            || !self.audit_vocal_synth()
        {
            return Vec::new();
        }
        let mut output = vec![0.0_f32; max_samples];
        for &(start, frequency, phoneme, length, pitch_curve, gain) in notes {
            let Ok(start) = usize::try_from(start) else {
                continue;
            };
            if start >= max_samples || length == 0 {
                continue;
            }
            let render_len = length.min(max_samples - start).min(2_000_000);
            let grain = self.synthesize_phoneme_with_pitch_curve(
                frequency,
                phoneme,
                render_len,
                pitch_curve,
            );
            let gain = if gain.is_finite() {
                gain.clamp(0.0, 1.0)
            } else {
                0.0
            };
            for (offset, sample) in grain.into_iter().enumerate() {
                let index = start + offset;
                if index >= max_samples {
                    break;
                }
                // Equal-power-ish bounded blend prevents stacked notes from
                // clipping while retaining legato overlaps.
                output[index] = (output[index] + sample * 0.78 * gain).clamp(-1.0, 1.0);
            }
        }
        output
    }

    /// Apply a per-note pitch curve in cents to an already synthesized
    /// phoneme. Linear interpolation keeps the preview bounded and avoids
    /// discontinuities when the curve has only a few control points.
    pub fn synthesize_phoneme_with_pitch_curve(
        &self,
        frequency: f32,
        phoneme: &str,
        length: usize,
        pitch_curve_cents: &[f32],
    ) -> Vec<f32> {
        let source = self.synthesize_phoneme(frequency, phoneme, length);
        if source.is_empty() || pitch_curve_cents.is_empty() {
            return source;
        }
        let mut output = vec![0.0_f32; source.len()];
        let mut source_position = 0.0_f32;
        for (index, sample) in output.iter_mut().enumerate() {
            let normalized = if source.len() > 1 {
                index as f32 / (source.len() - 1) as f32
            } else {
                0.0
            };
            let scaled = normalized * (pitch_curve_cents.len() - 1) as f32;
            let lower = scaled.floor() as usize;
            let upper = (lower + 1).min(pitch_curve_cents.len() - 1);
            let t = scaled - lower as f32;
            let a = pitch_curve_cents[lower];
            let b = pitch_curve_cents[upper];
            let cents = if a.is_finite() && b.is_finite() {
                (a + (b - a) * t).clamp(-2400.0, 2400.0)
            } else {
                0.0
            };
            let ratio = 2.0_f32.powf(cents / 1200.0).clamp(0.25, 4.0);
            let wrapped = source_position.rem_euclid(source.len() as f32);
            let left = wrapped.floor() as usize;
            let right = (left + 1) % source.len();
            let fraction = wrapped - left as f32;
            *sample = source[left] * (1.0 - fraction) + source[right] * fraction;
            source_position += ratio;
        }
        output
    }

    fn vowel_index(lyric: &str) -> u32 {
        let lower = lyric.trim().to_lowercase();
        let last = lower.chars().rev().find(|c| !c.is_whitespace());
        match last {
            Some('a') | Some('あ') | Some('か') | Some('さ') | Some('た') | Some('な')
            | Some('は') | Some('ま') | Some('や') | Some('ら') | Some('わ') | Some('ア')
            | Some('カ') | Some('サ') | Some('タ') | Some('ナ') | Some('ハ') | Some('マ')
            | Some('ヤ') | Some('ラ') | Some('ワ') => 0,
            Some('e') | Some('え') | Some('け') | Some('せ') | Some('て') | Some('ね')
            | Some('へ') | Some('め') | Some('れ') | Some('エ') | Some('ケ') | Some('セ')
            | Some('テ') | Some('ネ') | Some('ヘ') | Some('メ') | Some('レ') => 1,
            Some('i') | Some('い') | Some('き') | Some('し') | Some('ち') | Some('に')
            | Some('ひ') | Some('み') | Some('り') | Some('イ') | Some('キ') | Some('シ')
            | Some('チ') | Some('ニ') | Some('ヒ') | Some('ミ') | Some('リ') => 2,
            Some('o') | Some('お') | Some('こ') | Some('そ') | Some('と') | Some('の')
            | Some('ほ') | Some('も') | Some('よ') | Some('ろ') | Some('オ') | Some('コ')
            | Some('ソ') | Some('ト') | Some('ノ') | Some('ホ') | Some('モ') | Some('ヨ')
            | Some('ロ') => 3,
            Some('u') | Some('う') | Some('く') | Some('す') | Some('つ') | Some('ぬ')
            | Some('ふ') | Some('む') | Some('ゆ') | Some('る') | Some('ウ') | Some('ク')
            | Some('ス') | Some('ツ') | Some('ヌ') | Some('フ') | Some('ム') | Some('ユ')
            | Some('ル') => 4,
            _ => 5,
        }
    }

    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
                sr
            } else {
                44_100.0
            },
        }
    }

    /// Update the synthesis rate without allowing invalid filter or oscillator
    /// coefficients to enter the render path.
    pub fn set_sample_rate(&mut self, sr: f64) -> bool {
        if sr.is_finite() && (8_000.0..=384_000.0).contains(&sr) {
            self.sample_rate = sr;
            true
        } else {
            false
        }
    }

    /**
     * @brief SYNTHESIZE: Generates a vocal sound for a given pitch and vowel.
     * INDUSTRIAL: Beyond wavetable synthesis, this uses spectral shaping
     * to recreate the unique acoustic signature of human vowels.
     * Vowels: 0=A, 1=E, 2=I, 3=O, 4=U
     */
    pub fn synthesize_vowel(&self, frequency: f32, vowel_idx: u32) -> Vec<f32> {
        self.synthesize_vowel_with_length(frequency, vowel_idx, 1024)
    }

    fn synthesize_vowel_with_length(
        &self,
        frequency: f32,
        vowel_idx: u32,
        length: usize,
    ) -> Vec<f32> {
        // INDUSTRIAL: Formant frequency targets (Standard Human Reference).
        let formants: [f32; 3] = match vowel_idx {
            0 => [800.0, 1200.0, 2500.0], // A
            1 => [400.0, 2200.0, 3000.0], // E
            2 => [250.0, 2400.0, 3200.0], // I
            3 => [450.0, 800.0, 2800.0],  // O
            4 => [300.0, 700.0, 2600.0],  // U
            _ => [500.0, 1500.0, 2500.0], // Neutral
        };

        // A bad sample rate must not reach the trigonometric/filter coefficient
        // calculations.  Returning the correctly-sized buffer preserves the API
        // contract while keeping the result finite.
        if !self.sample_rate.is_finite() || self.sample_rate <= 100.0 {
            return vec![0.0; length];
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

        let mut output = vec![0.0; length];
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
        self.sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&self.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::VocalSynthKernel;

    #[test]
    fn lyric_render_is_bounded_and_finite() {
        let synth = VocalSynthKernel::new(44_100.0);
        let samples = synth.synthesize_lyric(220.0, "か", 4096);
        assert_eq!(samples.len(), 4096);
        assert!(samples
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
        assert_eq!(synth.synthesize_lyric(220.0, "", 0).len(), 0);
        assert_eq!(VocalSynthKernel::vowel_index("カ"), 0);
        assert_eq!(VocalSynthKernel::vowel_index("シ"), 2);
        assert_eq!(VocalSynthKernel::vowel_index("k a"), 0);
        assert_eq!(VocalSynthKernel::vowel_index("sh i"), 2);
        let vowel = synth.synthesize_phoneme(220.0, "a", 512);
        let consonant = synth.synthesize_phoneme(220.0, "k a", 512);
        assert!(vowel
            .iter()
            .zip(consonant.iter())
            .any(|(a, b)| (a - b).abs() > 1.0e-5));
        let phrase = synth.synthesize_phrase(&[(220.0, "k a", 128), (246.94, "sh i", 128)]);
        assert_eq!(phrase.len(), 224); // 32-sample boundary crossfade
        assert!(phrase.iter().all(|sample| sample.is_finite()));
        let curved =
            synth.synthesize_phoneme_with_pitch_curve(220.0, "a", 512, &[0.0, 100.0, -50.0]);
        assert_eq!(curved.len(), 512);
        assert!(curved.iter().all(|sample| sample.is_finite()));
        let timeline = synth
            .synthesize_note_timeline(&[(0, 220.0, "k a", 128), (96, 246.94, "sh i", 128)], 512);
        assert_eq!(timeline.len(), 512);
        assert!(timeline
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
        let curves: &[f32] = &[0.0, 100.0];
        let dynamics = synth.synthesize_note_timeline_with_curves_and_gain(
            &[
                (0, 220.0, "a", 128, curves, 1.0),
                (160, 220.0, "a", 128, curves, 0.2),
            ],
            512,
        );
        assert!(dynamics.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn invalid_sample_rate_uses_safe_default() {
        let synth = VocalSynthKernel::new(f64::NAN);
        assert!(synth.audit_vocal_synth());
        assert_eq!(synth.sample_rate, 44_100.0);
        let mut extreme = VocalSynthKernel::new(1_000_000.0);
        assert!(extreme.audit_vocal_synth());
        assert!(extreme.set_sample_rate(96_000.0));
        assert_eq!(extreme.sample_rate, 96_000.0);
        assert!(!extreme.set_sample_rate(1_000.0));
        extreme.sample_rate = f64::INFINITY;
        assert!(!extreme.audit_vocal_synth());
    }
}
