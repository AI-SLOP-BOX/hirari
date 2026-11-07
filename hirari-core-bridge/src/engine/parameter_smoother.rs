use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[no_mangle]
pub extern "C" fn hirari_parameter_smoother_create(initial: f32) -> *mut c_void {
    Box::into_raw(Box::new(SmootherOrchestrator::new(initial))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SmootherOrchestrator>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_set_target(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = state.cast::<SmootherOrchestrator>().as_ref() {
        state.set_target(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_reset(state: *const c_void, value: f32) {
    if !value.is_finite() {
        return;
    }
    if let Some(state) = state.cast::<SmootherOrchestrator>().as_ref() {
        state.reset(value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_set_time(
    state: *const c_void,
    milliseconds: f32,
    sample_rate: f32,
) {
    let Some(state) = state.cast::<SmootherOrchestrator>().as_ref() else {
        return;
    };
    state.set_smoothing_time(milliseconds, sample_rate);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_process(
    state: *const c_void,
    buffer: *mut f32,
    length: u32,
) {
    let Some(state) = state.cast::<SmootherOrchestrator>().as_ref() else {
        return;
    };
    if buffer.is_null() {
        return;
    }
    state.process_raw(buffer, length as usize);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_next(state: *const c_void) -> f32 {
    let Some(state) = state.cast::<SmootherOrchestrator>().as_ref() else {
        return 0.0;
    };
    state.next_value()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_parameter_smoother_current(state: *const c_void) -> f32 {
    state
        .cast::<SmootherOrchestrator>()
        .as_ref()
        .map_or(0.0, SmootherOrchestrator::current)
}

pub struct SmootherOrchestrator {
    target_bits: AtomicU32,
    current_bits: AtomicU32,
    base_coeff_bits: AtomicU32,
    sample_rate_bits: AtomicU32,
}

impl SmootherOrchestrator {
    pub fn new(initial_value: f32) -> Self {
        Self {
            target_bits: AtomicU32::new(initial_value.to_bits()),
            current_bits: AtomicU32::new(initial_value.to_bits()),
            base_coeff_bits: AtomicU32::new(0.01_f32.to_bits()),
            sample_rate_bits: AtomicU32::new(44100.0_f32.to_bits()),
        }
    }

    /// INDUSTRIAL: Sets the smoothing target with absolute memory precision and curve sovereignty.
    pub fn set_target(&self, value: f32) {
        if value.is_finite() {
            self.target_bits.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// INDUSTRIAL: Resets the smoother state with absolute temporal precision and sync sovereignty.
    pub fn reset(&self, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.target_bits.store(value.to_bits(), Ordering::Relaxed);
        self.current_bits.store(value.to_bits(), Ordering::Relaxed);
    }

    /// INDUSTRIAL: Configures smoothing time with absolute non-linear curve precision and signal sovereignty.
    pub fn set_smoothing_time(&self, ms: f32, sr: f32) {
        if !sr.is_finite() || sr <= 0.0 {
            self.base_coeff_bits
                .store(1.0_f32.to_bits(), Ordering::Relaxed);
            return;
        }
        self.sample_rate_bits.store(sr.to_bits(), Ordering::Relaxed);
        let coefficient = if !ms.is_finite() || ms <= 0.0 {
            1.0
        } else {
            let tau = ms * 0.001;
            (1.0 - (-1.0 / (sr * tau)).exp()).clamp(0.0, 1.0)
        };
        self.base_coeff_bits
            .store(coefficient.to_bits(), Ordering::Relaxed);
    }

    /// INDUSTRIAL: Processes a block of samples with absolute adaptive anti-zipper precision and signal sovereignty.
    pub fn process(&self, buffer: &mut [f32]) {
        if buffer.is_empty() {
            return;
        }
        unsafe { self.process_raw(buffer.as_mut_ptr(), buffer.len()) };
    }

    unsafe fn process_raw(&self, buffer: *mut f32, length: usize) {
        let mut current = self.current();
        let requested_target = self.target();
        let target = if requested_target.is_finite() {
            requested_target
        } else {
            current
        };
        let coeff = self.base_coeff().clamp(0.0, 1.0);
        for index in 0..length {
            current += (target - current) * coeff;
            if !current.is_finite() {
                current = target;
            }
            buffer.add(index).write(current);
        }
        self.current_bits
            .store(current.to_bits(), Ordering::Relaxed);
    }

    pub fn next_value(&self) -> f32 {
        let target = self.target();
        let coefficient = self.base_coeff().clamp(0.0, 1.0);
        let mut current = self.current();
        current += (target - current) * coefficient;
        if !current.is_finite() && target.is_finite() {
            current = target;
        }
        self.current_bits
            .store(current.to_bits(), Ordering::Relaxed);
        current
    }

    fn target(&self) -> f32 {
        f32::from_bits(self.target_bits.load(Ordering::Relaxed))
    }

    fn current(&self) -> f32 {
        f32::from_bits(self.current_bits.load(Ordering::Relaxed))
    }

    fn base_coeff(&self) -> f32 {
        f32::from_bits(self.base_coeff_bits.load(Ordering::Relaxed))
    }

    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate_bits.load(Ordering::Relaxed))
    }

    pub fn current_value(&self) -> f32 {
        self.current()
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide parameter smoothing state.
    pub fn audit_parameter_smoother(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic curve auditing logic.
        let target = self.target();
        let current = self.current();
        let coefficient = self.base_coeff();
        let sample_rate = self.sample_rate();
        target.is_finite()
            && current.is_finite()
            && coefficient.is_finite()
            && (0.0..=1.0).contains(&coefficient)
            && sample_rate.is_finite()
            && sample_rate > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::SmootherOrchestrator;

    #[test]
    fn process_writes_a_monotonic_ramp_towards_target() {
        let mut smoother = SmootherOrchestrator::new(0.0);
        smoother.set_smoothing_time(10.0, 48_000.0);
        smoother.set_target(1.0);
        let mut buffer = [0.0; 8];
        smoother.process(&mut buffer);

        assert!(buffer.windows(2).all(|w| w[1] >= w[0]));
        assert!(buffer[0] > 0.0 && buffer[7] < 1.0);
    }

    #[test]
    fn non_finite_target_does_not_poison_output() {
        let mut smoother = SmootherOrchestrator::new(0.25);
        smoother.set_target(f32::NAN);
        let mut buffer = [0.0; 4];
        smoother.process(&mut buffer);
        assert!(buffer.iter().all(|value| value.is_finite()));
    }
}
