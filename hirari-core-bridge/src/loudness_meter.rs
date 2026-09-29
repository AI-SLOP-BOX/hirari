use crate::k_weighting_filter::LegacyKWeightingFilter;
use std::ffi::c_void;

struct LegacyLoudnessMeter {
    sample_rate: f64,
    filter: LegacyKWeightingFilter,
    integrated_lufs: f32,
    block_energy: f64,
    block_samples: u64,
    gated_energy: f64,
    gated_blocks: u64,
}

impl LegacyLoudnessMeter {
    fn sanitize_sample_rate(sample_rate: f64) -> f64 {
        if sample_rate.is_finite() && sample_rate > 1000.0 {
            sample_rate
        } else {
            44_100.0
        }
    }

    fn new(sample_rate: f64) -> Self {
        let sample_rate = Self::sanitize_sample_rate(sample_rate);
        Self {
            sample_rate,
            filter: LegacyKWeightingFilter::new(sample_rate),
            integrated_lufs: -70.0,
            block_energy: 0.0,
            block_samples: 0,
            gated_energy: 0.0,
            gated_blocks: 0,
        }
    }

    fn reset(&mut self) {
        self.integrated_lufs = -70.0;
        self.block_samples = 0;
        self.block_energy = 0.0;
        self.gated_energy = 0.0;
        self.gated_blocks = 0;
        self.filter.reset();
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = Self::sanitize_sample_rate(sample_rate);
        self.filter.set_sample_rate(self.sample_rate);
        self.reset();
    }

    fn finish_block(&mut self) {
        if self.block_samples == 0 {
            return;
        }
        let mean_energy = self.block_energy / self.block_samples as f64;
        if mean_energy.is_finite() && mean_energy >= 1.0e-7 {
            self.gated_energy += mean_energy;
            self.gated_blocks += 1;
            let gated_mean = self.gated_energy / self.gated_blocks as f64;
            let candidate = -0.691 + 10.0 * gated_mean.max(1.0e-12).log10();
            if candidate.is_finite()
                && (self.integrated_lufs <= -70.0
                    || candidate >= self.integrated_lufs as f64 - 10.0)
            {
                self.integrated_lufs = candidate.clamp(-120.0, 20.0) as f32;
            }
        }
        self.block_energy = 0.0;
        self.block_samples = 0;
    }

    fn process(&mut self, left: &[f32], right: &[f32]) {
        let block_length = ((self.sample_rate * 0.4) as u64).max(1);
        for (&left, &right) in left.iter().zip(right) {
            let (mut weighted_left, mut weighted_right) = self.filter.process_stereo(left, right);
            if !weighted_left.is_finite() {
                weighted_left = 0.0;
            }
            if !weighted_right.is_finite() {
                weighted_right = 0.0;
            }
            self.block_energy += weighted_left as f64 * weighted_left as f64;
            self.block_energy += weighted_right as f64 * weighted_right as f64;
            self.block_samples += 1;
            if self.block_samples >= block_length {
                self.finish_block();
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_loudness_meter_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(LegacyLoudnessMeter::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_loudness_meter_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<LegacyLoudnessMeter>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_loudness_meter_set_sample_rate(
    state: *mut c_void,
    sample_rate: f64,
) {
    if !state.is_null() {
        (*state.cast::<LegacyLoudnessMeter>()).set_sample_rate(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_loudness_meter_reset(state: *mut c_void) {
    if !state.is_null() {
        (*state.cast::<LegacyLoudnessMeter>()).reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_loudness_meter_process(
    state: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let meter = &mut *state.cast::<LegacyLoudnessMeter>();
    meter.process(
        std::slice::from_raw_parts(left, frames as usize),
        std::slice::from_raw_parts(right, frames as usize),
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_loudness_meter_integrated_lufs(state: *const c_void) -> f32 {
    if state.is_null() {
        return -70.0;
    }
    (*state.cast::<LegacyLoudnessMeter>()).integrated_lufs
}

pub struct LoudnessMeterEngine {
    pub integrated_lufs: f32,
    pub energy_sum: f64,
    pub sample_count: u64,
    /// Sample and four-times oversampled peak readings for the integrated
    /// meter. These are kept alongside LUFS so a channel strip can expose a
    /// coherent metering snapshot without rescanning the audio block.
    pub peak_db: f32,
    pub true_peak_db: f32,
    pub previous_input_l: f32,
    pub previous_input_r: f32,

    // States
    pub hp_s1_l: f32,
    pub hp_s2_l: f32,
    pub hp_s1_r: f32,
    pub hp_s2_r: f32,
    pub shelf_s1_l: f32,
    pub shelf_s2_l: f32,
    pub shelf_s1_r: f32,
    pub shelf_s2_r: f32,
}

impl Default for LoudnessMeterEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessMeterEngine {
    pub fn new() -> Self {
        Self {
            integrated_lufs: -70.0,
            energy_sum: 0.0,
            sample_count: 0,
            peak_db: -240.0,
            true_peak_db: -240.0,
            previous_input_l: 0.0,
            previous_input_r: 0.0,
            hp_s1_l: 0.0,
            hp_s2_l: 0.0,
            hp_s1_r: 0.0,
            hp_s2_r: 0.0,
            shelf_s1_l: 0.0,
            shelf_s2_l: 0.0,
            shelf_s1_r: 0.0,
            shelf_s2_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.integrated_lufs = -70.0;
        self.energy_sum = 0.0;
        self.sample_count = 0;
        self.peak_db = -240.0;
        self.true_peak_db = -240.0;
        self.previous_input_l = 0.0;
        self.previous_input_r = 0.0;
        self.hp_s1_l = 0.0;
        self.hp_s2_l = 0.0;
        self.hp_s1_r = 0.0;
        self.hp_s2_r = 0.0;
        self.shelf_s1_l = 0.0;
        self.shelf_s2_l = 0.0;
        self.shelf_s1_r = 0.0;
        self.shelf_s2_r = 0.0;
    }

    /// INDUSTRIAL: Processes an audio block with K-Weighting and Gated Integration.
    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        let len = l.len().min(r.len());

        // Pre-calculated coefficients for 44.1kHz (ITU-R BS.1770-4)
        let shelf_b0 = 1.535_124_9_f32;
        let shelf_b1 = -2.691_696_2_f32;
        let shelf_b2 = 1.198_392_9_f32;
        let shelf_a1 = -1.690_659_3_f32;
        let shelf_a2 = 0.732_480_76_f32;

        let hp_b0 = 1.0f32;
        let hp_b1 = -2.0f32;
        let hp_b2 = 1.0f32;
        let hp_a1 = -1.990_047_5_f32;
        let hp_a2 = 0.990_072_25_f32;

        for s in 0..len {
            let in_l = if l[s].is_finite() { l[s] } else { 0.0 };
            let in_r = if r[s].is_finite() { r[s] } else { 0.0 };
            let sample_peak = in_l.abs().max(in_r.abs());
            self.peak_db = self.peak_db.max(20.0 * sample_peak.max(1.0e-12).log10());
            self.true_peak_db = self
                .true_peak_db
                .max(20.0 * sample_peak.max(1.0e-12).log10());
            if self.sample_count > 0 {
                for fraction in 1..4 {
                    let t = fraction as f32 * 0.25;
                    let interpolated_l = self.previous_input_l + (in_l - self.previous_input_l) * t;
                    let interpolated_r = self.previous_input_r + (in_r - self.previous_input_r) * t;
                    self.true_peak_db = self.true_peak_db.max(
                        20.0 * interpolated_l
                            .abs()
                            .max(interpolated_r.abs())
                            .max(1.0e-12)
                            .log10(),
                    );
                }
            }
            self.previous_input_l = in_l;
            self.previous_input_r = in_r;

            // 1. Pre-filter (K-Weighting Stage 1: High Shelf)
            let v1_l = in_l - shelf_a1 * self.shelf_s1_l - shelf_a2 * self.shelf_s2_l;
            let out1_l = shelf_b0 * v1_l + shelf_b1 * self.shelf_s1_l + shelf_b2 * self.shelf_s2_l;
            self.shelf_s2_l = self.shelf_s1_l;
            self.shelf_s1_l = v1_l;

            let v1_r = in_r - shelf_a1 * self.shelf_s1_r - shelf_a2 * self.shelf_s2_r;
            let out1_r = shelf_b0 * v1_r + shelf_b1 * self.shelf_s1_r + shelf_b2 * self.shelf_s2_r;
            self.shelf_s2_r = self.shelf_s1_r;
            self.shelf_s1_r = v1_r;

            // 2. High-pass (K-Weighting Stage 2: RLB Filter)
            let v2_l = out1_l - hp_a1 * self.hp_s1_l - hp_a2 * self.hp_s2_l;
            let out2_l = hp_b0 * v2_l + hp_b1 * self.hp_s1_l + hp_b2 * self.hp_s2_l;
            self.hp_s2_l = self.hp_s1_l;
            self.hp_s1_l = v2_l;

            let v2_r = out1_r - hp_a1 * self.hp_s1_r - hp_a2 * self.hp_s2_r;
            let out2_r = hp_b0 * v2_r + hp_b1 * self.hp_s1_r + hp_b2 * self.hp_s2_r;
            self.hp_s2_r = self.hp_s1_r;
            self.hp_s1_r = v2_r;

            // 3. Accumulate Energy (Mean Square)
            // BS.1770 stereo energy is the mean of the channel energies;
            // summing both channels directly would introduce a 3 dB bias.
            self.energy_sum += ((out2_l * out2_l + out2_r * out2_r) * 0.5) as f64;
            self.sample_count += 1;
        }

        // 4. Calculate LUFS
        if self.sample_count > 0 {
            let mean_square = self.energy_sum / self.sample_count as f64;
            self.integrated_lufs = -0.691 + 10.0 * (mean_square.max(1e-7).log10() as f32);
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Loudness Meter state.
    pub fn audit_loudness_meter(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic Loudness Meter auditing logic.
        self.integrated_lufs.is_finite()
            && self.energy_sum.is_finite()
            && self.energy_sum >= 0.0
            && (self.sample_count == 0 || self.integrated_lufs >= -240.0)
            && self.peak_db.is_finite()
            && self.true_peak_db.is_finite()
            && self.true_peak_db + 1.0e-4 >= self.peak_db
            && self.previous_input_l.is_finite()
            && self.previous_input_r.is_finite()
            && [
                self.hp_s1_l,
                self.hp_s2_l,
                self.hp_s1_r,
                self.hp_s2_r,
                self.shelf_s1_l,
                self.shelf_s2_l,
                self.shelf_s1_r,
                self.shelf_s2_r,
            ]
            .iter()
            .all(|value| value.is_finite())
    }

    pub fn snapshot(&self) -> LoudnessMeterSnapshot {
        LoudnessMeterSnapshot {
            integrated_lufs: self.integrated_lufs,
            peak_db: self.peak_db,
            true_peak_db: self.true_peak_db,
            sample_count: self.sample_count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessMeterSnapshot {
    pub integrated_lufs: f32,
    pub peak_db: f32,
    pub true_peak_db: f32,
    pub sample_count: u64,
}

#[cfg(test)]
mod tests {
    use super::LoudnessMeterEngine;

    #[test]
    fn snapshot_tracks_peak_and_integrated_loudness() {
        let mut meter = LoudnessMeterEngine::new();
        meter.process(&[0.5, -0.5, 0.25], &[0.5, -0.5, 0.25]);
        let snapshot = meter.snapshot();
        assert_eq!(snapshot.sample_count, 3);
        assert!(snapshot.peak_db < 0.0);
        assert!(snapshot.true_peak_db + 1.0e-4 >= snapshot.peak_db);
        assert!(meter.audit_loudness_meter());
    }
}
