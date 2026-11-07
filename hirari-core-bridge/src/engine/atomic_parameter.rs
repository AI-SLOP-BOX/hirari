use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

const LINEAR: u8 = 0;
const EXPONENTIAL: u8 = 1;
const UNIPOLAR: u8 = 0;
const BIPOLAR: u8 = 1;

pub(crate) struct AtomicParameterState {
    target: AtomicU32,
    ai_offset: AtomicU32,
    reset_value: AtomicU32,
    should_reset: AtomicBool,
    dirty: AtomicBool,
    current: AtomicU32,
    coefficient: AtomicU32,
    step: AtomicU32,
    display_mode: AtomicU8,
    unit: AtomicU8,
    smoothing_type: AtomicU8,
    sample_rate: AtomicU64,
    smoothing_time_ms: AtomicU64,
}

impl AtomicParameterState {
    pub(crate) fn new(initial: f32, display_mode: u8) -> Self {
        Self {
            target: AtomicU32::new(initial.to_bits()),
            ai_offset: AtomicU32::new(0.0_f32.to_bits()),
            reset_value: AtomicU32::new(0.0_f32.to_bits()),
            should_reset: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
            current: AtomicU32::new(initial.to_bits()),
            coefficient: AtomicU32::new(0.01_f32.to_bits()),
            step: AtomicU32::new(0.0_f32.to_bits()),
            display_mode: AtomicU8::new(display_mode.min(BIPOLAR)),
            unit: AtomicU8::new(0),
            smoothing_type: AtomicU8::new(EXPONENTIAL),
            sample_rate: AtomicU64::new(44100.0_f64.to_bits()),
            smoothing_time_ms: AtomicU64::new(10.0_f64.to_bits()),
        }
    }

    pub(crate) fn get_next_value(&self) -> f32 {
        if self.dirty.load(Ordering::Acquire) {
            self.update_internal_state();
        }
        let target = self.load_f32(&self.target, Ordering::Relaxed);
        let mut current = self.load_f32(&self.current, Ordering::Relaxed);
        if (current - target).abs() < 1.0e-7 {
            current = target;
            self.store_f32(&self.current, current, Ordering::Relaxed);
            return self.clamp_normalized(
                self.normalized_for(current) + self.load_f32(&self.ai_offset, Ordering::Relaxed),
            );
        }
        match self.smoothing_type.load(Ordering::Relaxed) {
            EXPONENTIAL => {
                current += (target - current) * self.load_f32(&self.coefficient, Ordering::Relaxed);
                if (current - target).abs() < 1.0e-24 {
                    current = target;
                }
            }
            LINEAR => {
                let step = self.load_f32(&self.step, Ordering::Relaxed);
                current += step;
                if (step > 0.0 && current > target) || (step < 0.0 && current < target) {
                    current = target;
                }
            }
            _ => current = target,
        }
        self.store_f32(&self.current, current, Ordering::Relaxed);
        self.clamp_normalized(
            self.normalized_for(current) + self.load_f32(&self.ai_offset, Ordering::Relaxed),
        )
    }

    fn update_internal_state(&self) {
        if self.should_reset.swap(false, Ordering::AcqRel) {
            let reset_value = self.load_f32(&self.reset_value, Ordering::Acquire);
            self.store_f32(&self.current, reset_value, Ordering::Relaxed);
        }
        self.recalculate_coefficients();
        self.dirty.store(false, Ordering::Release);
    }

    fn recalculate_coefficients(&self) {
        let sample_rate = f64::from_bits(self.sample_rate.load(Ordering::Relaxed));
        let smoothing_time_ms = f64::from_bits(self.smoothing_time_ms.load(Ordering::Relaxed));
        if sample_rate > 0.0 {
            let samples = smoothing_time_ms * 0.001 * sample_rate;
            self.store_f32(
                &self.coefficient,
                (1.0 - (-1.0 / samples.max(1.0)).exp()) as f32,
                Ordering::Release,
            );
            self.update_step(sample_rate, smoothing_time_ms);
        }
    }

    fn update_step(&self, sample_rate: f64, smoothing_time_ms: f64) {
        let target = self.load_f32(&self.target, Ordering::Relaxed);
        let current = self.load_f32(&self.current, Ordering::Relaxed);
        let samples = smoothing_time_ms * 0.001 * sample_rate;
        let step = if samples.is_finite() && samples > 0.0 {
            (target - current) / samples.max(1.0) as f32
        } else {
            0.0
        };
        self.store_f32(
            &self.step,
            if step.is_finite() { step } else { 0.0 },
            Ordering::Release,
        );
    }

        fn normalized_for(&self, current: f32) -> f32 {
        if self.display_mode.load(Ordering::Relaxed) == UNIPOLAR {
            ((current + 1.0) * 0.5).clamp(0.0, 1.0)
        } else {
            current
        }
    }

    fn clamp_normalized(&self, value: f32) -> f32 {
        value.clamp(0.0, 1.0)
    }

    fn load_f32(&self, value: &AtomicU32, ordering: Ordering) -> f32 {
        f32::from_bits(value.load(ordering))
    }

    fn store_f32(&self, target: &AtomicU32, value: f32, ordering: Ordering) {
        target.store(value.to_bits(), ordering);
    }

    pub(crate) fn set_target(&self, value: f32) {
        if value.is_finite() {
            self.store_f32(&self.target, value, Ordering::Release);
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// The admitted target, not the smoothed value: this is what a control-plane
    /// reader gets back after `set_target`.
    pub(crate) fn target(&self) -> f32 {
        self.load_f32(&self.target, Ordering::Acquire)
    }

    pub(crate) fn set_sample_rate(&self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sample_rate
                .store(sample_rate.to_bits(), Ordering::Relaxed);
            self.recalculate_coefficients();
        }
    }

    pub(crate) fn set_smoothing_time(&self, milliseconds: f64) {
        if milliseconds.is_finite() && milliseconds >= 0.0 {
            self.smoothing_time_ms
                .store(milliseconds.to_bits(), Ordering::Relaxed);
            self.recalculate_coefficients();
        }
    }

    pub(crate) fn reset_to_target(&self) {
        let target = self.load_f32(&self.target, Ordering::Acquire);
        self.store_f32(&self.current, target, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
    }
}

