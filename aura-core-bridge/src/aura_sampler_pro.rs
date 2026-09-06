use crate::adsr_envelope::AdsrEnvelopeEngine;

pub struct SamplerVoice {
    pub active: bool,
    pub note: u32,
    pub playback_pos: f64,
    pub current_speed: f32,
    pub target_speed: f32,
    pub slide_rate: f32,
    pub velocity: f32,
    pub envelope: AdsrEnvelopeEngine,
    pub filter_l_z1: f32,
    pub filter_l_z2: f32,
    pub filter_r_z1: f32,
    pub filter_r_z2: f32,
    pub filter_cutoff: f32,
    pub filter_resonance: f32,
    pub start_time: u64,
}

#[cfg(test)]
mod tests {
    use super::AuraSamplerProEngine;

    #[test]
    fn loop_region_updates_atomically_and_can_be_disabled() {
        let mut engine = AuraSamplerProEngine::new(48_000.0);
        assert!(engine.set_loop_region(2, 8, true));
        assert!(engine.is_looping);
        assert_eq!((engine.loop_start, engine.loop_end), (2, 8));
        assert!(!engine.set_loop_region(8, 8, true));
        assert_eq!((engine.loop_start, engine.loop_end), (2, 8));
        assert!(engine.set_loop_region(0, 0, false));
        assert!(!engine.is_looping);
        assert_eq!((engine.loop_start, engine.loop_end), (0, 0));
    }
}

pub struct AuraSamplerProEngine {
    pub sample_rate: f64,
    pub voices: Vec<SamplerVoice>,
    pub global_time: u64,
    pub is_looping: bool,
    pub loop_start: u64,
    pub loop_end: u64,
}

impl AuraSamplerProEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut voices = Vec::with_capacity(64);
        for _ in 0..64 {
            voices.push(SamplerVoice {
                active: false,
                note: 0,
                playback_pos: 0.0,
                current_speed: 1.0,
                target_speed: 1.0,
                slide_rate: 0.0,
                velocity: 1.0,
                envelope: AdsrEnvelopeEngine::new(sample_rate),
                filter_l_z1: 0.0,
                filter_l_z2: 0.0,
                filter_r_z1: 0.0,
                filter_r_z2: 0.0,
                filter_cutoff: 1000.0,
                filter_resonance: 0.1,
                start_time: 0,
            });
        }
        Self {
            sample_rate,
            voices,
            global_time: 0,
            is_looping: false,
            loop_start: 0,
            loop_end: 0,
        }
    }

    /// Configure the fallback loop used by samples without zone metadata.
    /// Disabling the loop clears the previous range so stale settings cannot
    /// affect a later render.
    pub fn set_loop_region(&mut self, start: u64, end: u64, enabled: bool) -> bool {
        if !enabled {
            self.is_looping = false;
            self.loop_start = 0;
            self.loop_end = 0;
            return true;
        }
        if start >= end || end - start < 2 {
            return false;
        }
        self.loop_start = start;
        self.loop_end = end;
        self.is_looping = true;
        true
    }

    /// INDUSTRIAL: Processes an audio block with TPT SVF filtering and Cubic interpolation.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], data_l: &[f32], data_r: &[f32]) {
        if l.len() != r.len()
            || data_l.len() != data_r.len()
            || data_l.len() < 4
            || (self.is_looping
                && (self.loop_end > data_l.len() as u64 || self.loop_start >= self.loop_end))
            || !self.audit_aura_sampler_pro()
        {
            return;
        }
        let num_frames = l.len();
        let max_idx = data_l.len();
        const SUB_BLOCK_SIZE: usize = 8;

        for v in self.voices.iter_mut() {
            if !v.active {
                continue;
            }

            // Pre-calculate TPT SVF Coeffs for the block
            let g = (std::f64::consts::PI * v.filter_cutoff as f64 / self.sample_rate).tan() as f32;
            let k = 2.0 - 2.0 * v.filter_resonance;
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;

            let mut i = 0;
            while i < num_frames {
                let block_len = (num_frames - i).min(SUB_BLOCK_SIZE);

                // Update Slide and Envelope at Sub-block boundaries
                if (v.start_time + i as u64).is_multiple_of(32) {
                    if (v.current_speed - v.target_speed).abs() >= 1e-4 {
                        v.current_speed += (v.target_speed - v.current_speed) * v.slide_rate;
                    } else {
                        v.current_speed = v.target_speed;
                    }
                }

                let env_start = v.envelope.current_level * v.velocity;
                let env_end = {
                    for _ in 0..block_len {
                        v.envelope.get_next();
                    }
                    v.envelope.current_level * v.velocity
                };

                if v.envelope.state == crate::adsr_envelope::AdsrState::Off {
                    v.active = false;
                    break;
                }

                let env_step = (env_end - env_start) / block_len as f32;
                let mut current_env = env_start;

                for j in 0..block_len {
                    let idx_global = i + j;
                    let pos = v.playback_pos;
                    let idx = pos as usize;

                    if idx >= 1 && idx < max_idx - 2 {
                        let f = (pos - idx as f64) as f32;

                        // Optimized 4-point Cubic Hermite (using Horner-like form)
                        let s0 = data_l[idx - 1];
                        let s1 = data_l[idx];
                        let s2 = data_l[idx + 1];
                        let s3 = data_l[idx + 2];

                        let c0 = s1;
                        let c1 = 0.5 * (s2 - s0);
                        let c2 = s0 - 2.5 * s1 + 2.0 * s2 - 0.5 * s3;
                        let c3 = 1.5 * (s1 - s2) + 0.5 * (s3 - s0);
                        let sl = ((c3 * f + c2) * f + c1) * f + c0;

                        let s0 = data_r[idx - 1];
                        let s1 = data_r[idx];
                        let s2 = data_r[idx + 1];
                        let s3 = data_r[idx + 2];

                        let c1_r = 0.5 * (s2 - s0);
                        let c2_r = s0 - 2.5 * s1 + 2.0 * s2 - 0.5 * s3;
                        let c3_r = 1.5 * (s1 - s2) + 0.5 * (s3 - s0);
                        let sr = ((c3_r * f + c2_r) * f + c1_r) * f + s1;

                        let sl_env = sl * current_env;
                        let sr_env = sr * current_env;

                        // TPT SVF FILTER
                        let v3l = sl_env - v.filter_l_z2;
                        let v1l = a1 * v.filter_l_z1 + a2 * v3l;
                        let v2l = v.filter_l_z2 + a2 * v.filter_l_z1 + a3 * v3l;
                        v.filter_l_z1 = 2.0 * v1l - v.filter_l_z1;
                        v.filter_l_z2 = 2.0 * v2l - v.filter_l_z2;

                        let v3r = sr_env - v.filter_r_z2;
                        let v1r = a1 * v.filter_r_z1 + a2 * v3r;
                        let v2r = v.filter_r_z2 + a2 * v.filter_r_z1 + a3 * v3r;
                        v.filter_r_z1 = 2.0 * v1r - v.filter_r_z1;
                        v.filter_r_z2 = 2.0 * v2r - v.filter_r_z2;

                        l[idx_global] = (l[idx_global] + if v2l.is_finite() { v2l } else { 0.0 })
                            .clamp(-4.0, 4.0);
                        r[idx_global] = (r[idx_global] + if v2r.is_finite() { v2r } else { 0.0 })
                            .clamp(-4.0, 4.0);

                        v.playback_pos += v.current_speed as f64;
                        current_env += env_step;

                        if self.is_looping && v.playback_pos >= self.loop_end as f64 {
                            let loop_len = (self.loop_end - self.loop_start) as f64;
                            if loop_len > 0.0 {
                                v.playback_pos = self.loop_start as f64
                                    + (v.playback_pos - self.loop_start as f64)
                                        .rem_euclid(loop_len);
                            } else {
                                v.active = false;
                                break;
                            }
                        }
                    } else {
                        v.active = false;
                        break;
                    }
                }

                if !v.active {
                    break;
                }
                i += block_len;
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sampler state.
    pub fn audit_aura_sampler_pro(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.voices.len() <= 256
            && (!self.is_looping || self.loop_end > self.loop_start)
            && self.voices.iter().all(|v| {
                v.note <= 127
                    && v.playback_pos.is_finite()
                    && v.playback_pos >= 0.0
                    && v.current_speed.is_finite()
                    && (0.0..=16.0).contains(&v.current_speed)
                    && v.target_speed.is_finite()
                    && (0.0..=16.0).contains(&v.target_speed)
                    && v.slide_rate.is_finite()
                    && (0.0..=1.0).contains(&v.slide_rate)
                    && v.velocity.is_finite()
                    && (0.0..=1.0).contains(&v.velocity)
                    && v.filter_cutoff.is_finite()
                    && v.filter_cutoff >= 0.0
                    && v.filter_resonance.is_finite()
                    && (0.0..=1.0).contains(&v.filter_resonance)
            })
    }
}
