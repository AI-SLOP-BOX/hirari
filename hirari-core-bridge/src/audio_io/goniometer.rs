use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub const HISTORY_SIZE: usize = 1024;

pub struct GoniometerEngine {
    correlation: AtomicU32,
    balance: AtomicU32,
    history_idx: AtomicUsize,
    active_buffer: AtomicUsize,
    history_l: [[AtomicU32; HISTORY_SIZE]; 2],
    history_r: [[AtomicU32; HISTORY_SIZE]; 2],
}

impl Default for GoniometerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GoniometerEngine {
    pub fn new() -> Self {
        Self {
            correlation: AtomicU32::new(1.0_f32.to_bits()),
            balance: AtomicU32::new(0.0_f32.to_bits()),
            history_idx: AtomicUsize::new(0),
            active_buffer: AtomicUsize::new(0),
            history_l: std::array::from_fn(|_| {
                std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits()))
            }),
            history_r: std::array::from_fn(|_| {
                std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits()))
            }),
        }
    }

    pub fn process(&self, left: &[f32], right: &[f32]) {
        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }
        let mut sum_l = 0.0_f64;
        let mut sum_r = 0.0_f64;
        let mut sum_lr = 0.0_f64;
        let mut history_idx = self.history_idx.load(Ordering::Relaxed);
        let mut active = self.active_buffer.load(Ordering::Relaxed).min(1);

        for frame in 0..frames {
            let sample_l = left[frame];
            let sample_r = right[frame];
            if frame % 4 == 0 {
                self.history_l[active][history_idx].store(sample_l.to_bits(), Ordering::Relaxed);
                self.history_r[active][history_idx].store(sample_r.to_bits(), Ordering::Relaxed);
                history_idx += 1;
                if history_idx >= HISTORY_SIZE {
                    history_idx = 0;
                    active = 1 - active;
                    self.active_buffer.store(active, Ordering::Relaxed);
                }
            }
            sum_l += f64::from(sample_l * sample_l);
            sum_r += f64::from(sample_r * sample_r);
            sum_lr += f64::from(sample_l * sample_r);
        }
        self.history_idx.store(history_idx, Ordering::Relaxed);

        let denominator = (sum_l * sum_r).sqrt() + 1.0e-12;
        let correlation = (sum_lr / denominator) as f32;
        let old_correlation = f32::from_bits(self.correlation.load(Ordering::Relaxed));
        let next_correlation =
            old_correlation + 0.05 * (correlation.clamp(-1.0, 1.0) - old_correlation);
        self.correlation
            .store(next_correlation.to_bits(), Ordering::Relaxed);

        let total_energy = (sum_l + sum_r) as f32 + 1.0e-12;
        let balance = ((sum_r - sum_l) as f32) / total_energy;
        let old_balance = f32::from_bits(self.balance.load(Ordering::Relaxed));
        let next_balance = old_balance + 0.1 * (balance.clamp(-1.0, 1.0) - old_balance);
        self.balance
            .store(next_balance.to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(
        &self,
        left: &mut [f32; HISTORY_SIZE],
        right: &mut [f32; HISTORY_SIZE],
    ) -> (f32, f32) {
        let active = self.active_buffer.load(Ordering::Relaxed).min(1);
        let stable = 1 - active;
        for index in 0..HISTORY_SIZE {
            left[index] = f32::from_bits(self.history_l[stable][index].load(Ordering::Relaxed));
            right[index] = f32::from_bits(self.history_r[stable][index].load(Ordering::Relaxed));
        }
        (
            f32::from_bits(self.correlation.load(Ordering::Relaxed)),
            f32::from_bits(self.balance.load(Ordering::Relaxed)),
        )
    }
}

#[no_mangle]
pub extern "C" fn hirari_goniometer_create() -> *mut c_void {
    Box::into_raw(Box::new(GoniometerEngine::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_goniometer_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<GoniometerEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_goniometer_process(
    state: *const c_void,
    left: *const f32,
    right: *const f32,
    frames: usize,
) {
    if state.is_null() || left.is_null() || right.is_null() {
        return;
    }
    let engine = &*state.cast::<GoniometerEngine>();
    engine.process(
        std::slice::from_raw_parts(left, frames),
        std::slice::from_raw_parts(right, frames),
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_goniometer_snapshot(
    state: *const c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    correlation: *mut f32,
    balance: *mut f32,
) -> bool {
    if state.is_null()
        || left.is_null()
        || right.is_null()
        || frames != HISTORY_SIZE
        || correlation.is_null()
        || balance.is_null()
    {
        return false;
    }
    let engine = &*state.cast::<GoniometerEngine>();
    let left_out = &mut *(left.cast::<[f32; HISTORY_SIZE]>());
    let right_out = &mut *(right.cast::<[f32; HISTORY_SIZE]>());
    let (corr, bal) = engine.snapshot(left_out, right_out);
    *correlation = corr;
    *balance = bal;
    true
}
