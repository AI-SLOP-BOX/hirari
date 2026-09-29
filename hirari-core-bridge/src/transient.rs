use std::ffi::c_void;

struct LegacyTransientDetector {
    env_fast: f32,
    env_slow: f32,
    alpha_fast: f32,
    alpha_slow: f32,
    lookahead_samples: u64,
    results: Vec<TransientInfo>,
}

impl LegacyTransientDetector {
    fn new(sample_rate: f64, lookahead_ms: f32) -> Self {
        Self {
            env_fast: 0.0,
            env_slow: 0.0,
            alpha_fast: (-1.0 / (sample_rate * 0.005)).exp() as f32,
            alpha_slow: (-1.0 / (sample_rate * 0.050)).exp() as f32,
            lookahead_samples: (sample_rate * (lookahead_ms / 1000.0_f32) as f64) as u64,
            results: Vec::new(),
        }
    }

    fn analyze(&mut self, data: &[f32], threshold: f32) {
        self.results.clear();
        if data.len() < 2 {
            return;
        }
        let limit = if threshold.is_finite() {
            threshold.clamp(0.0, 1.0)
        } else {
            0.15
        };
        let mut previous_flux = 0.0_f32;
        for (index, &sample) in data.iter().enumerate() {
            let level = sample.abs();
            self.env_fast = self.alpha_fast * self.env_fast + (1.0 - self.alpha_fast) * level;
            self.env_slow = self.alpha_slow * self.env_slow + (1.0 - self.alpha_slow) * level;
            let flux = (self.env_fast - self.env_slow).max(0.0);
            if index as u64 > self.lookahead_samples
                && flux > limit
                && flux >= previous_flux
                && self
                    .results
                    .last()
                    .is_none_or(|last| index as u64 - last.sample_index > self.lookahead_samples)
            {
                self.results.push(TransientInfo {
                    sample_index: index as u64,
                    strength: flux,
                });
            }
            previous_flux = flux;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_transient_detector_create(
    sample_rate: f64,
    lookahead_ms: f32,
) -> *mut c_void {
    Box::into_raw(Box::new(LegacyTransientDetector::new(
        sample_rate,
        lookahead_ms,
    )))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_detector_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<LegacyTransientDetector>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_detector_analyze(
    state: *mut c_void,
    data: *const f32,
    length: usize,
    threshold: f32,
) {
    if state.is_null() {
        return;
    }
    let detector = &mut *state.cast::<LegacyTransientDetector>();
    if data.is_null() {
        detector.results.clear();
        return;
    }
    detector.analyze(std::slice::from_raw_parts(data, length), threshold);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_detector_result_count(state: *const c_void) -> usize {
    if state.is_null() {
        return 0;
    }
    (*state.cast::<LegacyTransientDetector>()).results.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_transient_detector_get_result(
    state: *const c_void,
    index: usize,
    sample_index: *mut u64,
    strength: *mut f32,
) -> bool {
    if state.is_null() || sample_index.is_null() || strength.is_null() {
        return false;
    }
    let detector = &*state.cast::<LegacyTransientDetector>();
    let Some(result) = detector.results.get(index) else {
        return false;
    };
    *sample_index = result.sample_index;
    *strength = result.strength;
    true
}

pub struct TransientInfo {
    pub sample_index: u64,
    pub strength: f32,
}

pub struct TransientOrchestrator {
    pub sample_rate: f64,
    pub env_fast: f32,
    pub env_slow: f32,
    pub alpha_fast: f32,
    pub alpha_slow: f32,
    pub last_transient_idx: Option<usize>,
}

impl TransientOrchestrator {
    pub fn new(sr: f64) -> Self {
        let sr = if sr.is_finite() {
            sr.clamp(8_000.0, 384_000.0)
        } else {
            48_000.0
        };
        let alpha_fast = (-1.0 / (sr * 0.005)) as f32; // 5ms
        let alpha_slow = (-1.0 / (sr * 0.050)) as f32; // 50ms

        Self {
            sample_rate: sr,
            env_fast: 0.0,
            env_slow: 0.0,
            alpha_fast: alpha_fast.exp(),
            alpha_slow: alpha_slow.exp(),
            last_transient_idx: None,
        }
    }

    /// INDUSTRIAL: Performs transient analysis with absolute precision and transient sovereignty.
    pub fn analyze_transients(&mut self, buffer: &[f32], threshold: f32) -> Vec<TransientInfo> {
        let len = buffer.len();
        let mut transients = Vec::new();
        let min_interval = (self.sample_rate * 0.020) as usize; // 20ms lockout
        let threshold = if threshold.is_finite() {
            threshold.max(0.0)
        } else {
            0.0
        };

        for i in 0..len {
            let val = if buffer[i].is_finite() {
                buffer[i].abs()
            } else {
                0.0
            };

            // Update envelopes
            self.env_fast = self.alpha_fast * self.env_fast + (1.0 - self.alpha_fast) * val;
            self.env_slow = self.alpha_slow * self.env_slow + (1.0 - self.alpha_slow) * val;

            // Transient detection (Energy jump)
            if self.env_fast > self.env_slow * (1.0 + threshold) {
                let should_trigger = match self.last_transient_idx {
                    Some(last_idx) => i - last_idx > min_interval,
                    None => true,
                };

                if should_trigger {
                    transients.push(TransientInfo {
                        sample_index: i as u64,
                        strength: self.env_fast / self.env_slow,
                    });
                    self.last_transient_idx = Some(i);
                }
            }
        }

        transients
    }

    pub fn audit_transients(&self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.env_fast.is_finite()
            && self.env_slow.is_finite()
            && self.alpha_fast.is_finite()
            && (0.0..1.0).contains(&self.alpha_fast)
            && self.alpha_slow.is_finite()
            && (0.0..1.0).contains(&self.alpha_slow)
    }
}
