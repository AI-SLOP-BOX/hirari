use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

struct MasteringKernel {
    gain_reduction: AtomicU32,
    current_rms_dbfs: AtomicU32,
    target_rms: AtomicU32,
    write_left: AtomicUsize,
    write_right: AtomicUsize,
    dc_estimate_left: UnsafeCell<f64>,
    dc_estimate_right: UnsafeCell<f64>,
    previous_left: UnsafeCell<f32>,
    previous_right: UnsafeCell<f32>,
}

// The mutable signal history is written only by the audio processing caller;
// public meter and target access use atomics, matching the former C++ contract.
unsafe impl Sync for MasteringKernel {}

impl MasteringKernel {
    fn new() -> Self {
        Self {
            gain_reduction: AtomicU32::new(1.0f32.to_bits()),
            current_rms_dbfs: AtomicU32::new(f32::NEG_INFINITY.to_bits()),
            target_rms: AtomicU32::new(0.18f32.to_bits()),
            write_left: AtomicUsize::new(0), write_right: AtomicUsize::new(0),
            dc_estimate_left: UnsafeCell::new(0.0), dc_estimate_right: UnsafeCell::new(0.0),
            previous_left: UnsafeCell::new(0.0), previous_right: UnsafeCell::new(0.0),
        }
    }

    fn limit_true_peak(sample: f32, previous: f32) -> f32 {
        if !sample.is_finite() { return 0.0; }
        let midpoint = 0.5 * (previous + sample);
        let peak = sample.abs().max(midpoint.abs());
        if !peak.is_finite() || peak <= 0.99 { sample.clamp(-0.99, 0.99) }
        else { (sample * (0.99 / peak)).clamp(-0.99, 0.99) }
    }

    unsafe fn process(&self, left: *mut f32, right: *mut f32, samples: u32) {
        self.write_left.store(left as usize, Ordering::Relaxed);
        self.write_right.store(right as usize, Ordering::Relaxed);
        if left.is_null() || right.is_null() || samples == 0 { return; }
        let dc_l = &mut *self.dc_estimate_left.get();
        let dc_r = &mut *self.dc_estimate_right.get();
        let mut sum_sq = 0.0f64;
        for i in 0..samples as usize {
            let l_sample = *left.add(i);
            let r_sample = *right.add(i);
            if l_sample.is_finite() { let sample = l_sample as f64 - *dc_l; sum_sq += sample * sample; }
            if r_sample.is_finite() { let sample = r_sample as f64 - *dc_r; sum_sq += sample * sample; }
        }
        let rms = (sum_sq / (samples as f64 * 2.0)).sqrt() as f32;
        let rms_dbfs = 20.0 * rms.max(1.0e-6).log10();
        self.current_rms_dbfs.store(rms_dbfs.to_bits(), Ordering::Relaxed);
        let target = f32::from_bits(self.target_rms.load(Ordering::Relaxed));
        let target_gain = target / (rms + 1.0e-6);
        let mut current = f32::from_bits(self.gain_reduction.load(Ordering::Relaxed));
        current += (target_gain - current) * 0.01;
        self.gain_reduction.store(current.min(1.0).to_bits(), Ordering::Relaxed);
        let gain = f32::from_bits(self.gain_reduction.load(Ordering::Relaxed));
        let previous_l = &mut *self.previous_left.get();
        let previous_r = &mut *self.previous_right.get();
        for i in 0..samples as usize {
            let l = left.add(i);
            let r = right.add(i);
            if (*l).is_finite() {
                *dc_l += 0.0005 * (*l as f64 - *dc_l);
                *l = ((*l as f64 - *dc_l) * gain as f64) as f32;
            } else { *l = 0.0; }
            if (*r).is_finite() {
                *dc_r += 0.0005 * (*r as f64 - *dc_r);
                *r = ((*r as f64 - *dc_r) * gain as f64) as f32;
            } else { *r = 0.0; }
            *l = Self::limit_true_peak(*l, *previous_l);
            *r = Self::limit_true_peak(*r, *previous_r);
            *previous_l = *l;
            *previous_r = *r;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_mastering_kernel_create() -> *mut c_void {
    Box::into_raw(Box::new(MasteringKernel::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mastering_kernel_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<MasteringKernel>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mastering_kernel_set_target(state: *mut c_void, rms: f32) {
    let Some(state) = state.cast::<MasteringKernel>().as_ref() else { return };
    if rms.is_finite() { state.target_rms.store(rms.clamp(1.0e-6, 1.0).to_bits(), Ordering::Relaxed); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mastering_kernel_process(
    state: *mut c_void, left: *mut f32, right: *mut f32, samples: u32,
) {
    if let Some(state) = state.cast::<MasteringKernel>().as_ref() { state.process(left, right, samples); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mastering_kernel_rms_dbfs(state: *const c_void) -> f32 {
    state.cast::<MasteringKernel>().as_ref().map_or(f32::NEG_INFINITY, |s|
        f32::from_bits(s.current_rms_dbfs.load(Ordering::Relaxed)))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mastering_kernel_write_pointer(state: *const c_void, channel: i32) -> *mut f32 {
    let Some(s) = state.cast::<MasteringKernel>().as_ref() else { return std::ptr::null_mut() };
    match channel { 0 => s.write_left.load(Ordering::Relaxed) as *mut f32,
        1 => s.write_right.load(Ordering::Relaxed) as *mut f32, _ => std::ptr::null_mut() }
}
