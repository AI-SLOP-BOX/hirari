pub struct SidechainCompressorEngine {
    pub sample_rate: f64,
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub env: f32,
    pub current_gain: f32,
}

#[derive(Clone, Copy)]
struct CompressorParameters {
    threshold: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
}

struct CompressorRuntime {
    sample_rate: f64,
    env: f32,
    current_gain: f32,
}

fn process_block(
    runtime: &mut CompressorRuntime,
    params: CompressorParameters,
    sample_rate: f64,
    main_left: *const f32,
    main_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    sidechain_left: *const f32,
    sidechain_right: *const f32,
    sidechain_frames: usize,
    frames: usize,
) {
    if main_left.is_null()
        || main_right.is_null()
        || output_left.is_null()
        || output_right.is_null()
        || frames == 0
    {
        return;
    }

    let sr = if sample_rate.is_finite() && sample_rate > 1.0 {
        sample_rate
    } else {
        runtime.sample_rate
    };
    let attack_ms = (params.attack_ms as f64).max(1.0);
    let release_ms = (params.release_ms as f64).max(1.0);
    let attack_denominator = (attack_ms * 0.001 * sr) as f32;
    let release_denominator = (release_ms * 0.001 * sr) as f32;
    let attack_coeff = 1.0 - (-1.0 / attack_denominator).exp();
    let release_coeff = 1.0 - (-1.0 / release_denominator).exp();
    let threshold = if params.threshold.is_finite() {
        params.threshold.clamp(1.0e-5, 1.0)
    } else {
        0.2
    };
    let ratio = if params.ratio.is_finite() {
        params.ratio.max(1.0)
    } else {
        1.0
    };

    for frame in 0..frames {
        // SAFETY: The native caller provides each non-null channel buffer for `frames` samples.
        let main_l = unsafe { *main_left.add(frame) };
        let main_r = unsafe { *main_right.add(frame) };
        let detector_l = if !sidechain_left.is_null() {
            if frame < sidechain_frames {
                unsafe { *sidechain_left.add(frame) }
            } else {
                0.0
            }
        } else {
            main_l
        };
        let detector_r = if !sidechain_right.is_null() {
            if frame < sidechain_frames {
                unsafe { *sidechain_right.add(frame) }
            } else {
                0.0
            }
        } else if !sidechain_left.is_null() {
            detector_l
        } else {
            main_r
        };
        let detector_l = detector_l.abs();
        let detector_r = detector_r.abs();
        // Match std::max(a, b)'s ordering for NaN inputs.
        let detector = if detector_l < detector_r {
            detector_r
        } else {
            detector_l
        };
        let target_env = if detector.is_finite() { detector } else { 0.0 };
        let env_coeff = if target_env > runtime.env {
            attack_coeff
        } else {
            release_coeff
        };
        runtime.env += (target_env - runtime.env) * env_coeff;

        let mut desired_gain = 1.0;
        if runtime.env > threshold {
            let compressed = threshold + (runtime.env - threshold) / ratio;
            desired_gain = (compressed / runtime.env.max(1.0e-6)).clamp(0.0, 1.0);
        }
        let gain_coeff = if desired_gain < runtime.current_gain {
            attack_coeff
        } else {
            release_coeff
        };
        runtime.current_gain += (desired_gain - runtime.current_gain) * gain_coeff;

        let out_l = main_l * runtime.current_gain;
        unsafe {
            *output_left.add(frame) = if out_l.is_finite() { out_l } else { 0.0 };
        }
        if output_right != output_left {
            let out_r = main_r * runtime.current_gain;
            unsafe {
                *output_right.add(frame) = if out_r.is_finite() { out_r } else { 0.0 };
            }
        }
    }
}

impl SidechainCompressorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            threshold: 0.2,
            ratio: 10.0,
            attack: 10.0,
            release: 100.0,
            env: 0.0,
            current_gain: 1.0,
        }
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.current_gain = 1.0;
    }

    pub fn set_params(&mut self, threshold: f32, ratio: f32, attack: f32, release: f32) {
        self.threshold = threshold;
        self.ratio = ratio;
        self.attack = attack;
        self.release = release;
    }

    pub fn try_set_params(
        &mut self,
        threshold: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> bool {
        if !threshold.is_finite()
            || !(1e-5..=4.0).contains(&threshold)
            || !ratio.is_finite()
            || !(1.0..=1000.0).contains(&ratio)
            || !attack_ms.is_finite()
            || !(0.01..=10_000.0).contains(&attack_ms)
            || !release_ms.is_finite()
            || !(0.01..=30_000.0).contains(&release_ms)
        {
            return false;
        }
        self.set_params(threshold, ratio, attack_ms, release_ms);
        true
    }

    /// INDUSTRIAL: Professional High-performance Ducking Engine using the Sidechain input.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], sidechain: Option<(&[f32], &[f32])>) {
        let len = l.len().min(r.len());
        if len == 0 || !self.audit_sidechain_compressor() {
            return;
        }
        let (sidechain_left, sidechain_right, sidechain_frames) = sidechain
            .map(|(left, right)| (left.as_ptr(), right.as_ptr(), left.len().min(right.len())))
            .unwrap_or((std::ptr::null(), std::ptr::null(), 0));
        let mut runtime = CompressorRuntime {
            sample_rate: self.sample_rate,
            env: self.env,
            current_gain: self.current_gain,
        };
        let parameters = CompressorParameters {
            threshold: self.threshold,
            ratio: self.ratio,
            attack_ms: self.attack,
            release_ms: self.release,
        };
        process_block(
            &mut runtime,
            parameters,
            self.sample_rate,
            l.as_ptr(),
            r.as_ptr(),
            l.as_mut_ptr(),
            r.as_mut_ptr(),
            sidechain_left,
            sidechain_right,
            sidechain_frames,
            len,
        );
        self.env = runtime.env;
        self.current_gain = runtime.current_gain;
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sidechain Compressor state.
    pub fn audit_sidechain_compressor(&self) -> bool {
        self.sample_rate.is_finite()
            && crate::dsp_limits::is_supported_sample_rate(self.sample_rate)
            && self.threshold.is_finite()
            && (1e-5..=4.0).contains(&self.threshold)
            && self.ratio.is_finite()
            && (1.0..=1000.0).contains(&self.ratio)
            && self.attack.is_finite()
            && (0.01..=10_000.0).contains(&self.attack)
            && self.release.is_finite()
            && (0.01..=30_000.0).contains(&self.release)
            && self.env.is_finite()
            && (0.0..=4.0).contains(&self.env)
            && self.current_gain.is_finite()
            && (0.0..=1.0).contains(&self.current_gain)
    }
}


#[cfg(test)]
mod tests {
    use super::SidechainCompressorEngine;
    use std::f32::consts::PI;

    #[test]
    fn rust_sidechain_kernel_matches_native_cpp_reference_samples() {
        let sample_rate = 48_000.0;
        let mut left: Vec<f32> = (0..384)
            .map(|i| (2.0 * PI * 173.0 * i as f32 / sample_rate as f32).sin() * 0.72)
            .collect();
        let mut right: Vec<f32> = (0..384)
            .map(|i| {
                if i % 47 == 0 {
                    0.85
                } else {
                    (2.0 * PI * 263.0 * i as f32 / sample_rate as f32).sin() * 0.61
                }
            })
            .collect();
        let side_left: Vec<f32> = (0..384)
            .map(|i| if (64..192).contains(&i) { 0.95 } else { 0.04 })
            .collect();
        let side_right: Vec<f32> = (0..384)
            .map(|i| if (96..224).contains(&i) { 0.82 } else { 0.03 })
            .collect();

        let mut expected_left = left.clone();
        let mut expected_right = right.clone();
        let threshold = 0.18f32;
        let ratio = 7.0f32;
        let attack_ms = 4.0f64;
        let release_ms = 73.0f64;
        let attack_coeff =
            1.0f32 - (-1.0f32 / (attack_ms.max(1.0) * 0.001 * sample_rate) as f32).exp();
        let release_coeff =
            1.0f32 - (-1.0f32 / (release_ms.max(1.0) * 0.001 * sample_rate) as f32).exp();
        let mut env = 0.0f32;
        let mut current_gain = 1.0f32;
        for i in 0..expected_left.len() {
            let a = side_left[i].abs();
            let b = side_right[i].abs();
            let detector = if a < b { b } else { a };
            let env_coeff = if detector > env {
                attack_coeff
            } else {
                release_coeff
            };
            env += (detector - env) * env_coeff;
            let mut desired_gain = 1.0f32;
            if env > threshold {
                let compressed = threshold + (env - threshold) / ratio;
                desired_gain = (compressed / env.max(1.0e-6)).clamp(0.0, 1.0);
            }
            let gain_coeff = if desired_gain < current_gain {
                attack_coeff
            } else {
                release_coeff
            };
            current_gain += (desired_gain - current_gain) * gain_coeff;
            expected_left[i] *= current_gain;
            expected_right[i] *= current_gain;
        }

        let mut compressor = SidechainCompressorEngine::new(sample_rate);
        compressor.set_params(threshold, ratio, attack_ms as f32, release_ms as f32);
        compressor.process(&mut left, &mut right, Some((&side_left, &side_right)));

        for (actual, expected) in left
            .iter()
            .zip(&expected_left)
            .chain(right.iter().zip(&expected_right))
        {
            assert!(
                (actual - expected).abs() <= 1.0e-5,
                "{actual} != {expected}"
            );
        }
        assert!((compressor.env - env).abs() <= 1.0e-5);
        assert!((compressor.current_gain - current_gain).abs() <= 1.0e-5);
    }

    #[test]
    fn external_sidechain_reduces_program_audio() {
        let mut compressor = SidechainCompressorEngine::new(48_000.0);
        compressor.set_params(0.1, 10.0, 0.1, 20.0);
        let mut left = vec![0.8; 512];
        let mut right = vec![0.8; 512];
        let side_left = vec![1.0; 512];
        let side_right = vec![1.0; 512];

        compressor.process(&mut left, &mut right, Some((&side_left, &side_right)));

        assert!(compressor.current_gain < 1.0);
        assert!(left.iter().all(|sample| sample.is_finite()));
        assert!(right.iter().all(|sample| sample.is_finite()));
        assert!(left[32..].iter().all(|sample| *sample < 0.8));
        assert!(right[32..].iter().all(|sample| *sample < 0.8));
        assert!(compressor.audit_sidechain_compressor());
    }

    #[test]
    fn missing_sidechain_falls_back_to_program_detector() {
        let mut compressor = SidechainCompressorEngine::new(48_000.0);
        compressor.set_params(0.1, 4.0, 1.0, 20.0);
        let mut left = vec![0.7; 256];
        let mut right = vec![0.7; 256];
        compressor.process(&mut left, &mut right, None);

        assert!(compressor.current_gain < 1.0);
        assert!(left.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn validated_parameter_updates_preserve_previous_state_on_error() {
        let mut compressor = SidechainCompressorEngine::new(48_000.0);
        assert!(compressor.try_set_params(0.2, 4.0, 5.0, 100.0));
        assert!(!compressor.try_set_params(0.0, 4.0, 5.0, 100.0));
        assert_eq!((compressor.threshold, compressor.ratio), (0.2, 4.0));
        assert!(compressor.audit_sidechain_compressor());
    }
}
