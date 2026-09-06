pub struct ElasticWarpEngine {
    pub sample_rate: f64,
    pub grain_size: usize,
    pub overlap_size: usize,
    pub window: Vec<f32>,
    pub ola_buffer_l: Vec<f32>,
    pub ola_buffer_r: Vec<f32>,
    pub ola_weight: Vec<f32>,
    pub write_idx: usize,
    pub samples_since_last_grain: usize,
    pub current_read_pos: usize,
    pub last_read_pos: usize,
    pub source_pos_acc: f64,
    pub prev_energy: f32,
}

/// Group warp facade for multi-microphone recordings. Every member uses the
/// same fractional read position, so transient timing and inter-channel phase
/// remain identical while the group is stretched. The implementation is
/// intentionally allocation-controlled and deterministic for offline renders.
#[derive(Debug, Clone)]
pub struct PhaseCoherentWarpGroup {
    pub channels: usize,
    pub time_ratio: f64,
}

#[inline]
fn cubic_sample(track: &[f32], position: f64) -> f32 {
    if track.is_empty() || !position.is_finite() {
        return 0.0;
    }
    let base = position.floor() as isize;
    let t = (position - base as f64) as f32;
    let sample = |index: isize| -> f32 {
        let index = index.clamp(0, track.len() as isize - 1) as usize;
        let value = track[index];
        if value.is_finite() {
            value
        } else {
            0.0
        }
    };
    let p0 = sample(base - 1);
    let p1 = sample(base);
    let p2 = sample(base + 1);
    let p3 = sample(base + 2);
    let t2 = t * t;
    let t3 = t2 * t;
    let value = 0.5
        * ((2.0 * p1)
            + (-p0 + p2) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

impl PhaseCoherentWarpGroup {
    pub fn new(channels: usize, time_ratio: f64) -> Option<Self> {
        if channels == 0
            || channels > 256
            || !time_ratio.is_finite()
            || !(0.125..=8.0).contains(&time_ratio)
        {
            return None;
        }
        Some(Self {
            channels,
            time_ratio,
        })
    }

    /// Stretches a group of mono tracks with one shared source-time map.
    /// Inputs must have identical lengths; short groups are rejected rather
    /// than silently losing a microphone.
    pub fn process(&self, inputs: &[Vec<f32>], output_len: usize) -> Option<Vec<Vec<f32>>> {
        if inputs.len() != self.channels
            || inputs.iter().any(|track| {
                track.is_empty() || track.len() > 16_000_000 || track.len() != inputs[0].len()
            })
            || output_len == 0
            || output_len > 16_000_000
        {
            return None;
        }
        let source_len = inputs[0].len();
        let mut output = vec![vec![0.0; output_len]; self.channels];
        for (index, tracks) in output.iter_mut().enumerate() {
            for (frame, sample) in tracks.iter_mut().enumerate() {
                let position = frame as f64 * self.time_ratio;
                if position >= (source_len - 1) as f64 {
                    *sample = inputs[index][source_len - 1];
                    continue;
                }
                *sample = cubic_sample(&inputs[index], position);
            }
        }
        Some(output)
    }

    /// Same warp as [`process`], with per-channel sample corrections returned
    /// by `estimate_phase_offsets`.  Fractional interpolation is shared across
    /// channels; integer offsets only alter the read origin.
    pub fn process_with_phase_offsets(
        &self,
        inputs: &[Vec<f32>],
        offsets: &[i32],
        output_len: usize,
    ) -> Option<Vec<Vec<f32>>> {
        if inputs.len() != self.channels
            || offsets.len() != self.channels
            || inputs.iter().any(|track| {
                track.is_empty() || track.len() > 16_000_000 || track.len() != inputs[0].len()
            })
            || output_len == 0
            || output_len > 16_000_000
            || offsets.iter().any(|offset| offset.unsigned_abs() > 8192)
        {
            return None;
        }
        let source_len = inputs[0].len();
        let mut output = vec![vec![0.0; output_len]; self.channels];
        for (channel, track_output) in output.iter_mut().enumerate() {
            for (frame, sample) in track_output.iter_mut().enumerate() {
                let base_position = frame as f64 * self.time_ratio - f64::from(offsets[channel]);
                if !(0.0..source_len as f64).contains(&base_position) {
                    *sample = 0.0;
                    continue;
                }
                *sample = cubic_sample(&inputs[channel], base_position);
            }
        }
        Some(output)
    }

    /// Estimates per-microphone sample offsets against the first channel by
    /// bounded normalized cross-correlation.  Applying the same result before
    /// a warp keeps drum transients phase-coherent across the whole group.
    pub fn estimate_phase_offsets(
        &self,
        inputs: &[Vec<f32>],
        max_offset: usize,
    ) -> Option<Vec<i32>> {
        if inputs.len() != self.channels
            || inputs.is_empty()
            || max_offset > 8192
            || inputs.iter().any(|track| {
                track.is_empty() || track.len() != inputs[0].len() || track.len() > 16_000_000
            })
        {
            return None;
        }
        let reference = &inputs[0];
        let mut offsets = vec![0i32; self.channels];
        for (channel, track) in inputs.iter().enumerate().skip(1) {
            let mut best = (0i32, f64::NEG_INFINITY);
            for signed in -(max_offset as i32)..=(max_offset as i32) {
                let (start_ref, start_track, count) = if signed >= 0 {
                    (
                        signed as usize,
                        0usize,
                        reference.len().saturating_sub(signed as usize),
                    )
                } else {
                    (
                        0usize,
                        signed.unsigned_abs() as usize,
                        reference
                            .len()
                            .saturating_sub(signed.unsigned_abs() as usize),
                    )
                };
                if count == 0 {
                    continue;
                }
                let mut dot = 0.0f64;
                let mut ref_energy = 0.0f64;
                let mut track_energy = 0.0f64;
                let stride = if count < 4096 { 1 } else { 4 };
                for index in (0..count).step_by(stride) {
                    let a = reference[start_ref + index];
                    let b = track[start_track + index];
                    if !a.is_finite() || !b.is_finite() {
                        continue;
                    }
                    dot += f64::from(a) * f64::from(b);
                    ref_energy += f64::from(a) * f64::from(a);
                    track_energy += f64::from(b) * f64::from(b);
                }
                let denominator = (ref_energy * track_energy).sqrt();
                let score = if denominator > f64::EPSILON {
                    dot / denominator
                } else {
                    f64::NEG_INFINITY
                };
                if score > best.1 {
                    best = (signed, score);
                }
            }
            offsets[channel] = best.0;
        }
        Some(offsets)
    }

    pub fn validate(&self) -> bool {
        self.channels > 0
            && self.channels <= 256
            && self.time_ratio.is_finite()
            && (0.125..=8.0).contains(&self.time_ratio)
    }
}

impl ElasticWarpEngine {
    pub fn new(sr: f64) -> Self {
        let sample_rate = if sr.is_finite() && sr >= 1_000.0 {
            sr
        } else {
            48_000.0
        };
        let grain_size = (sample_rate * 0.050).round().max(2.0) as usize; // 50ms grains
        let overlap_size = grain_size / 2;
        let ola_size = (grain_size * 4).next_power_of_two();

        let mut window = vec![0.0; grain_size];
        for i in 0..grain_size {
            window[i] = 0.5
                * (1.0
                    - (2.0 * std::f64::consts::PI * i as f64 / (grain_size - 1) as f64).cos()
                        as f32);
        }

        Self {
            sample_rate,
            grain_size,
            overlap_size,
            window,
            ola_buffer_l: vec![0.0; ola_size],
            ola_buffer_r: vec![0.0; ola_size],
            ola_weight: vec![0.0; ola_size],
            write_idx: 0,
            samples_since_last_grain: 999999, // Force first grain
            current_read_pos: 0,
            last_read_pos: 0,
            source_pos_acc: 0.0,
            prev_energy: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.write_idx = 0;
        self.samples_since_last_grain = 999999;
        self.last_read_pos = 0;
        self.prev_energy = 0.0;
        self.ola_buffer_l.fill(0.0);
        self.ola_buffer_r.fill(0.0);
        self.ola_weight.fill(0.0);
        self.source_pos_acc = 0.0;
    }

    pub fn is_transient(&mut self, l: &[f32], r: &[f32]) -> bool {
        let len = l.len().min(r.len());
        if len == 0 {
            return false;
        }

        let mut energy = 0.0;
        for i in 0..len {
            let left = if l[i].is_finite() { l[i] } else { 0.0 };
            let right = if r[i].is_finite() { r[i] } else { 0.0 };
            energy += left.abs() + right.abs();
        }

        let diff = energy - self.prev_energy;
        self.prev_energy = energy;

        (energy > 0.01) && (diff > energy * 0.4)
    }

    pub fn find_best_match(
        &self,
        in_l: &[f32],
        in_r: &[f32],
        search_start: usize,
        target_pos: usize,
    ) -> usize {
        let max_len = in_l.len().min(in_r.len());
        if self.grain_size == 0
            || target_pos > max_len.saturating_sub(self.grain_size)
            || search_start > max_len.saturating_sub(self.grain_size)
        {
            return target_pos;
        }

        let mut best_pos = target_pos;
        let mut max_corr = -1e15;
        let range = self.grain_size / 4;

        let start = target_pos.saturating_sub(range);
        let end = target_pos
            .saturating_add(range)
            .min(max_len - self.grain_size);

        for test_pos in (start..end).step_by(2) {
            let mut dot = 0.0f64;
            let mut source_energy = 0.0f64;
            let mut candidate_energy = 0.0f64;
            // Only check a portion of the grain for speed
            for j in (0..self.grain_size / 8).step_by(4) {
                let s0 = f64::from(in_l[search_start + j]) + f64::from(in_r[search_start + j]);
                let s1 = f64::from(in_l[test_pos + j]) + f64::from(in_r[test_pos + j]);
                if s0.is_finite() && s1.is_finite() {
                    dot += s0 * s1;
                    source_energy += s0 * s0;
                    candidate_energy += s1 * s1;
                }
            }
            let denominator = (source_energy * candidate_energy).sqrt();
            let corr = if denominator > f64::EPSILON {
                dot / denominator
            } else {
                -1.0
            };
            if corr > max_corr {
                max_corr = corr;
                best_pos = test_pos;
            }
        }
        best_pos
    }

    /// INDUSTRIAL: Real-time stretch with Continuous Phase Accumulation.
    pub fn process_warp(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        time_ratio: f64,
    ) {
        let time_ratio = if time_ratio.is_finite() {
            time_ratio.clamp(0.5, 2.0)
        } else {
            1.0
        };
        let num_samples = out_l.len().min(out_r.len());
        let in_total_samples = in_l.len().min(in_r.len());
        if num_samples == 0 || in_total_samples < self.grain_size || self.ola_buffer_l.is_empty() {
            out_l[..num_samples].fill(0.0);
            out_r[..num_samples].fill(0.0);
            return;
        }
        let mask = self.ola_buffer_l.len() - 1;

        for i in 0..num_samples {
            if self.samples_since_last_grain >= self.overlap_size {
                // 1. INCREMENTAL SOURCE POSITION (Phase-correct)
                self.source_pos_acc += self.overlap_size as f64 * time_ratio;
                let mut target_pos = self.source_pos_acc.min(usize::MAX as f64) as usize;

                if target_pos > in_total_samples.saturating_sub(self.grain_size) {
                    self.source_pos_acc = 0.0;
                    target_pos = 0;
                }

                let search_pos = self.last_read_pos.min(in_total_samples - self.grain_size);

                let check_len = 128.min(in_total_samples - target_pos);
                let attack = self.is_transient(
                    &in_l[target_pos..target_pos + check_len],
                    &in_r[target_pos..target_pos + check_len],
                );

                // 2. PHASE-ALIGNED GRAIN SEARCH
                self.current_read_pos = if attack {
                    target_pos
                } else {
                    self.find_best_match(in_l, in_r, search_pos, target_pos)
                };

                // 3. OVERLAP-ADD (with Normalization)
                for g in 0..self.grain_size {
                    let out_idx = (self.write_idx + g) & mask;
                    let read_idx = self.current_read_pos + g;

                    if read_idx < in_total_samples {
                        self.ola_buffer_l[out_idx] += in_l[read_idx] * self.window[g];
                        self.ola_buffer_r[out_idx] += in_r[read_idx] * self.window[g];
                        self.ola_weight[out_idx] += self.window[g];
                    }
                }

                self.last_read_pos = self.current_read_pos;
                self.samples_since_last_grain = 0;
            }

            // 4. OUTPUT & OLA CLEAR
            let weight = self.ola_weight[self.write_idx];
            let inverse_weight = if weight > 1.0e-6 { 1.0 / weight } else { 0.0 };
            out_l[i] = self.ola_buffer_l[self.write_idx] * inverse_weight;
            self.ola_buffer_l[self.write_idx] = 0.0;
            out_r[i] = self.ola_buffer_r[self.write_idx] * inverse_weight;
            self.ola_buffer_r[self.write_idx] = 0.0;
            self.ola_weight[self.write_idx] = 0.0;

            self.write_idx = (self.write_idx + 1) & mask;
            self.samples_since_last_grain += 1;
        }
        out_l[num_samples..].fill(0.0);
        out_r[num_samples..].fill(0.0);
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Elastic Warp state.
    pub fn audit_elastic_warp(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.grain_size >= 2
            && self.overlap_size > 0
            && self.overlap_size <= self.grain_size
            && self.window.len() == self.grain_size
            && self.window.iter().all(|value| value.is_finite())
            && self.ola_weight.len() == self.ola_buffer_l.len()
            && self
                .ola_weight
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
            && !self.ola_buffer_l.is_empty()
            && self.ola_buffer_l.len() == self.ola_buffer_r.len()
            && self.ola_buffer_l.len().is_power_of_two()
            && self.write_idx < self.ola_buffer_l.len()
            && self.source_pos_acc.is_finite()
            && self.prev_energy.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::{ElasticWarpEngine, PhaseCoherentWarpGroup};

    #[test]
    fn invalid_public_state_fails_audit() {
        let mut engine = ElasticWarpEngine::new(48_000.0);
        assert!(engine.audit_elastic_warp());
        engine.source_pos_acc = f64::NAN;
        assert!(!engine.audit_elastic_warp());
    }

    #[test]
    fn group_warp_uses_one_time_map_for_all_microphones() {
        let group = PhaseCoherentWarpGroup::new(2, 0.5).unwrap();
        let output = group
            .process(&[vec![0.0, 1.0, 2.0, 3.0], vec![10.0, 11.0, 12.0, 13.0]], 4)
            .unwrap();
        assert_eq!(output[0], vec![0.0, 0.4375, 1.0, 1.5]);
        assert_eq!(output[1], vec![10.0, 10.4375, 11.0, 11.5]);
        assert!(group.validate());
    }

    #[test]
    fn phase_offset_estimation_finds_shifted_transient() {
        let group = PhaseCoherentWarpGroup::new(2, 1.0).unwrap();
        let reference = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let delayed = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let offsets = group
            .estimate_phase_offsets(&[reference, delayed], 2)
            .unwrap();
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[1], -1);
    }

    #[test]
    fn phase_offsets_are_applied_during_group_render() {
        let group = PhaseCoherentWarpGroup::new(2, 1.0).unwrap();
        let output = group
            .process_with_phase_offsets(
                &[vec![0.0, 0.0, 1.0, 0.0], vec![0.0, 0.0, 0.0, 1.0]],
                &[0, -1],
                4,
            )
            .unwrap();
        assert_eq!(output[0], output[1]);
    }

    #[test]
    fn ola_weight_normalization_preserves_steady_signal_level() {
        let mut engine = ElasticWarpEngine::new(48_000.0);
        let input_l = vec![0.5_f32; 4_096];
        let input_r = vec![0.5_f32; 4_096];
        let mut output_l = vec![0.0_f32; 2_048];
        let mut output_r = vec![0.0_f32; 2_048];
        engine.process_warp(&input_l, &input_r, &mut output_l, &mut output_r, 1.0);
        let steady = &output_l[512..1_536];
        let mean = steady.iter().copied().sum::<f32>() / steady.len() as f32;
        assert!((mean - 0.5).abs() < 0.02, "mean={mean}");
        assert!(steady
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
    }
}
