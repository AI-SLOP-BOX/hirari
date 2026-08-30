pub struct Voice {
    pub active: bool,
    pub position: f64,
    pub pitch_ratio: f64,
    pub velocity: f32,
    pub env_level: f32,
    pub env_state: u32, // 0=Idle, 1=Attack, 2=Decay, 3=Sustain, 4=Release
    pub note: u8,
    pub is_streaming: bool,
    pub brightness: f32,
}

/// Audio owned by the sampler.  The right channel may be omitted for mono data.
pub struct SampleData {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub loop_start: usize,
    pub loop_end: usize,
    pub looping: bool,
}

pub struct SamplerEngineEngine {
    pub sample_rate: f64,
    pub voices: Vec<Voice>,
    pub free_voices: Vec<usize>,
    pub sample: Option<SampleData>,
}

impl SamplerEngineEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut voices = Vec::with_capacity(64);
        let mut free_voices = Vec::with_capacity(64);
        for i in 0..64 {
            voices.push(Voice {
                active: false,
                position: 0.0,
                pitch_ratio: 1.0,
                velocity: 1.0,
                env_level: 0.0,
                env_state: 0,
                note: 0,
                is_streaming: false,
                brightness: 1.0,
            });
            free_voices.push(i);
        }
        Self {
            sample_rate,
            voices,
            free_voices,
            sample: None,
        }
    }

    pub fn set_sample(&mut self, sample: Option<SampleData>) {
        self.sample = sample.filter(|sample| {
            !sample.left.is_empty()
                && sample.left.len() <= 64 * 1024 * 1024
                && (sample.right.is_empty() || sample.right.len() == sample.left.len())
                && sample.loop_start <= sample.loop_end
                && sample.loop_end <= sample.left.len()
        });
    }

    pub fn note_on(&mut self, note: u8, velocity: u8, root_note: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }
        let voice_idx = if let Some(idx) = self.free_voices.pop() {
            idx
        } else {
            // Priority-based stealing
            self.find_voice_to_steal()
        };

        self.start_voice(voice_idx, note, velocity, root_note);
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.active && v.note == note {
                v.env_state = 4;
            }
        }
    }

    /// INDUSTRIAL: Processes an audio block with Multi-Voice Sample Playback.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let num_samples = l.len().min(r.len());

        for v_idx in 0..self.voices.len() {
            let v = &mut self.voices[v_idx];
            if !v.active {
                continue;
            }

            // LOD Threshold: Professional quality vs Performance
            let use_high_quality = (v.env_level * v.velocity > 0.15) || (v.brightness > 0.8);

            for s in 0..num_samples {
                self.update_envelope(v_idx);
                let v = &self.voices[v_idx]; // Re-borrow after mutation
                if !v.active {
                    self.free_voices.push(v_idx);
                    break;
                }

                let (val_l, val_r) = self.sample.as_ref().map_or((0.0, 0.0), |sample| {
                    let position = if v.position.is_finite() {
                        v.position
                    } else {
                        0.0
                    };
                    let l = Self::interpolate(&sample.left, position, sample, use_high_quality);
                    let r_source = if sample.right.is_empty() {
                        &sample.left
                    } else {
                        &sample.right
                    };
                    let r = Self::interpolate(r_source, position, sample, use_high_quality);
                    (l, r)
                });

                let master_gain = v.velocity * v.env_level;
                l[s] = ((if l[s].is_finite() { l[s] } else { 0.0 }) + val_l * master_gain)
                    .clamp(-1.0e6, 1.0e6);
                r[s] = ((if r[s].is_finite() { r[s] } else { 0.0 }) + val_r * master_gain)
                    .clamp(-1.0e6, 1.0e6);

                let v_mut = &mut self.voices[v_idx];
                let step = if v_mut.pitch_ratio.is_finite() {
                    v_mut.pitch_ratio
                } else {
                    0.0
                };
                v_mut.position += step.max(0.0);
                if !v_mut.position.is_finite() {
                    v_mut.position = 0.0;
                }
            }
        }
    }

    fn interpolate(data: &[f32], position: f64, sample: &SampleData, hermite: bool) -> f32 {
        if data.is_empty() || !position.is_finite() {
            return 0.0;
        }
        let len = data.len();
        let loop_start = sample.loop_start.min(len.saturating_sub(1));
        let loop_end = sample.loop_end.min(len);
        let valid_loop = sample.looping && loop_end > loop_start + 1;
        let mut p = position;
        if valid_loop && p >= loop_end as f64 {
            p = loop_start as f64
                + (p - loop_start as f64).rem_euclid((loop_end - loop_start) as f64);
        }
        if p < 0.0 || p >= len as f64 {
            return 0.0;
        }
        let i = p.floor() as usize;
        let frac = (p - i as f64) as f32;
        let at = |index: isize| -> f32 {
            let idx = if valid_loop && index >= loop_end as isize {
                loop_start + (index - loop_start as isize) as usize % (loop_end - loop_start)
            } else if index < 0 {
                0
            } else {
                (index as usize).min(len - 1)
            };
            let value = data[idx];
            if value.is_finite() {
                value
            } else {
                0.0
            }
        };
        let a = at(i as isize);
        let b = at(i as isize + 1);
        if !hermite {
            return a + (b - a) * frac;
        }
        let y0 = at(i as isize - 1);
        let y1 = a;
        let y2 = b;
        let y3 = at(i as isize + 2);
        let c0 = y1;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        (c0 + frac * (c1 + frac * (c2 + frac * c3))).clamp(-1.0e6, 1.0e6)
    }

    fn find_voice_to_steal(&self) -> usize {
        let mut best_idx = 0;
        let mut min_level = 2.0f32;
        for i in 0..self.voices.len() {
            if self.voices[i].env_level < min_level {
                min_level = self.voices[i].env_level;
                best_idx = i;
            }
        }
        best_idx
    }

    fn start_voice(&mut self, idx: usize, note: u8, velocity: u8, root_note: u8) {
        let v = &mut self.voices[idx];
        v.active = true;
        v.note = note;
        v.velocity = velocity as f32 / 127.0;
        v.position = 0.0;
        v.pitch_ratio = 2.0f64.powf((note as f64 - root_note as f64) / 12.0);
        v.env_state = 1;
        v.env_level = 0.0;
    }

    fn update_envelope(&mut self, idx: usize) {
        let v = &mut self.voices[idx];
        let attack_step = 0.002f32;
        let release_step = 0.001f32;
        let sustain_level = 0.8f32;

        match v.env_state {
            1 => {
                v.env_level += attack_step;
                if v.env_level >= 1.0 {
                    v.env_level = 1.0;
                    v.env_state = 2;
                }
            }
            2 => {
                v.env_level -= 0.0005;
                if v.env_level <= sustain_level {
                    v.env_level = sustain_level;
                    v.env_state = 3;
                }
            }
            4 => {
                v.env_level -= release_step;
                if v.env_level <= 0.0 {
                    v.env_level = 0.0;
                    v.active = false;
                    v.env_state = 0;
                }
            }
            _ => {}
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sampler state.
    pub fn audit_sampler_engine(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.voices.len() == 64
            && self.voices.iter().all(|voice| {
                voice.position.is_finite()
                    && voice.pitch_ratio.is_finite()
                    && voice.velocity.is_finite()
                    && voice.env_level.is_finite()
                    && voice.brightness.is_finite()
                    && voice.env_level >= 0.0
                    && voice.env_level <= 1.0
            })
            && self.sample.as_ref().is_none_or(|sample| {
                !sample.left.is_empty()
                    && (sample.right.is_empty() || sample.right.len() == sample.left.len())
                    && sample.loop_start <= sample.loop_end
                    && sample.loop_end <= sample.left.len()
                    && sample.left.iter().all(|value| value.is_finite())
                    && sample.right.iter().all(|value| value.is_finite())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_sample() -> SampleData {
        SampleData {
            left: vec![1.0, 0.5, 0.25, 0.0],
            right: vec![],
            loop_start: 0,
            loop_end: 0,
            looping: false,
        }
    }

    #[test]
    fn renders_loaded_sample_data() {
        let mut engine = SamplerEngineEngine::new(48_000.0);
        engine.set_sample(Some(impulse_sample()));
        engine.note_on(60, 127, 60);
        let mut left = vec![0.0; 16];
        let mut right = vec![0.0; 16];
        engine.process(&mut left, &mut right);

        assert!(left.iter().any(|sample| sample.abs() > 0.001));
        assert_eq!(left, right);
    }

    #[test]
    fn non_looping_sample_falls_silent_after_end() {
        let mut engine = SamplerEngineEngine::new(48_000.0);
        engine.set_sample(Some(impulse_sample()));
        engine.note_on(60, 127, 60);
        let mut first = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        engine.process(&mut first, &mut right);
        let mut tail = vec![0.0; 8];
        let mut tail_right = vec![0.0; 8];
        engine.process(&mut tail, &mut tail_right);

        assert!(tail.iter().all(|sample| sample.abs() < 1.0e-6));
    }
}
