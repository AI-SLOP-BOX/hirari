use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

const FIR: [[f32; 12]; 4] = [
    [
        -0.0017, 0.0076, -0.0223, 0.0531, -0.1130, 0.5763, 0.5763, -0.1130, 0.0531, -0.0223,
        0.0076, -0.0017,
    ],
    [
        -0.0007, 0.0033, -0.0104, 0.0264, -0.0645, 0.8123, 0.2812, -0.0711, 0.0354, -0.0157,
        0.0055, -0.0012,
    ],
    [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [
        -0.0012, 0.0055, -0.0157, 0.0354, -0.0711, 0.2812, 0.8123, -0.0645, 0.0264, -0.0104,
        0.0033, -0.0007,
    ],
];

#[derive(Default)]
struct MeterDspState {
    sample_rate: f64,
    history_l: [f32; 6],
    history_r: [f32; 6],
}

struct MeterCore {
    dsp: UnsafeCell<MeterDspState>,
    analysis: *mut c_void,
    goniometer: *mut c_void,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    true_peak_l: AtomicU32,
    true_peak_r: AtomicU32,
    rms_l: AtomicU32,
    rms_r: AtomicU32,
}

impl MeterDspState {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            ..Self::default()
        }
    }

    fn reset(&mut self) {
        self.history_l.fill(0.0);
        self.history_r.fill(0.0);
    }

    fn process(&mut self, left: &[f32], right: &[f32], old: [f32; 6]) -> [f32; 6] {
        let frames = left.len().min(right.len());
        if frames == 0 || !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            return old;
        }
        let [old_peak_l, old_peak_r, old_true_l, old_true_r, old_rms_l, old_rms_r] = old;
        let mut sum_l = 0.0_f32;
        let mut sum_r = 0.0_f32;
        let mut max_l = 0.0_f32;
        let mut max_r = 0.0_f32;
        let mut true_max_l = 0.0_f32;
        let mut true_max_r = 0.0_f32;

        for frame in 0..frames {
            let sample_l = if left[frame].is_finite() {
                left[frame].abs()
            } else {
                0.0
            };
            let sample_r = if right[frame].is_finite() {
                right[frame].abs()
            } else {
                0.0
            };
            sum_l += sample_l * sample_l;
            sum_r += sample_r * sample_r;
            max_l = max_l.max(sample_l);
            max_r = max_r.max(sample_r);

            for coefficients in &FIR {
                let mut poly_l = 0.0_f32;
                let mut poly_r = 0.0_f32;
                for (tap, coefficient) in coefficients.iter().enumerate() {
                    let index = frame as isize - tap as isize + 6;
                    if index >= -6 && index < frames as isize {
                        let (input_l, input_r) = if index < 0 {
                            let history_index = (index + 6) as usize;
                            (self.history_l[history_index], self.history_r[history_index])
                        } else {
                            let index = index as usize;
                            let l = if left[index].is_finite() {
                                left[index]
                            } else {
                                0.0
                            };
                            let r = if right[index].is_finite() {
                                right[index]
                            } else {
                                0.0
                            };
                            (l, r)
                        };
                        poly_l += input_l * coefficient;
                        poly_r += input_r * coefficient;
                    }
                }
                true_max_l = true_max_l.max(poly_l.abs());
                true_max_r = true_max_r.max(poly_r.abs());
            }
        }
        true_max_l = true_max_l.max(max_l);
        true_max_r = true_max_r.max(max_r);

        let peak_l = max_l.max(old_peak_l * 0.999);
        let peak_r = max_r.max(old_peak_r * 0.999);
        let true_peak_l = true_max_l.max(old_true_l * 0.999);
        let true_peak_r = true_max_r.max(old_true_r * 0.999);

        let rms_block_l = (sum_l / (frames as f32 + 1.0e-10)).sqrt();
        let rms_block_r = (sum_r / (frames as f32 + 1.0e-10)).sqrt();
        let alpha = 1.0 - (-(frames as f32) / (self.sample_rate * 0.3) as f32).exp();
        let rms_l = old_rms_l + alpha * (rms_block_l - old_rms_l);
        let rms_r = old_rms_r + alpha * (rms_block_r - old_rms_r);

        let history_count = 6.min(frames);
        for index in 0..history_count {
            let source = frames - history_count + index;
            self.history_l[6 - history_count + index] = if left[source].is_finite() {
                left[source]
            } else {
                0.0
            };
            self.history_r[6 - history_count + index] = if right[source].is_finite() {
                right[source]
            } else {
                0.0
            };
        }
        [peak_l, peak_r, true_peak_l, true_peak_r, rms_l, rms_r]
    }
}

#[no_mangle]
pub extern "C" fn hirari_master_meter_create(sample_rate: f64) -> *mut c_void {
    let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    };
    Box::into_raw(Box::new(MeterCore {
        dsp: UnsafeCell::new(MeterDspState::new(sample_rate)),
        analysis: crate::analysis_engine::hirari_analysis_engine_create(sample_rate),
        goniometer: crate::goniometer::hirari_goniometer_create(),
        peak_l: AtomicU32::new(0.0_f32.to_bits()),
        peak_r: AtomicU32::new(0.0_f32.to_bits()),
        true_peak_l: AtomicU32::new(0.0_f32.to_bits()),
        true_peak_r: AtomicU32::new(0.0_f32.to_bits()),
        rms_l: AtomicU32::new(0.0_f32.to_bits()),
        rms_r: AtomicU32::new(0.0_f32.to_bits()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_destroy(state: *mut c_void) {
    if !state.is_null() {
        let core = unsafe { Box::from_raw(state.cast::<MeterCore>()) };
        unsafe {
            crate::analysis_engine::hirari_analysis_engine_destroy(core.analysis);
            crate::goniometer::hirari_goniometer_destroy(core.goniometer);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_prepare(state: *mut c_void, sample_rate: f64) {
    if state.is_null() {
        return;
    }
    let core = unsafe { &*state.cast::<MeterCore>() };
    let dsp = unsafe { &mut *core.dsp.get() };
    dsp.sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    };
    dsp.reset();
    unsafe { hirari_master_meter_reset(state) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_reset(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let core = unsafe { &*state.cast::<MeterCore>() };
    unsafe { (*core.dsp.get()).reset() };
    core.peak_l.store(0.0_f32.to_bits(), Ordering::Relaxed);
    core.peak_r.store(0.0_f32.to_bits(), Ordering::Relaxed);
    core.true_peak_l.store(0.0_f32.to_bits(), Ordering::Relaxed);
    (*core)
        .true_peak_r
        .store(0.0_f32.to_bits(), Ordering::Relaxed);
    core.rms_l.store(0.0_f32.to_bits(), Ordering::Relaxed);
    core.rms_r.store(0.0_f32.to_bits(), Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_process(
    state: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: usize,
) {
    if state.is_null() || left.is_null() || right.is_null() {
        return;
    }
    let core = unsafe { &*state.cast::<MeterCore>() };
    let dsp = unsafe { &mut *core.dsp.get() };
    let left = unsafe { std::slice::from_raw_parts(left, frames) };
    let right = unsafe { std::slice::from_raw_parts(right, frames) };
    let old = [
        f32::from_bits(core.peak_l.load(Ordering::Relaxed)),
        f32::from_bits(core.peak_r.load(Ordering::Relaxed)),
        f32::from_bits(core.true_peak_l.load(Ordering::Relaxed)),
        f32::from_bits(core.true_peak_r.load(Ordering::Relaxed)),
        f32::from_bits(core.rms_l.load(Ordering::Relaxed)),
        f32::from_bits(core.rms_r.load(Ordering::Relaxed)),
    ];
    let next = dsp.process(left, right, old);
    core.peak_l.store(next[0].to_bits(), Ordering::Relaxed);
    core.peak_r.store(next[1].to_bits(), Ordering::Relaxed);
    core.true_peak_l.store(next[2].to_bits(), Ordering::Relaxed);
    core.true_peak_r.store(next[3].to_bits(), Ordering::Relaxed);
    core.rms_l.store(next[4].to_bits(), Ordering::Relaxed);
    core.rms_r.store(next[5].to_bits(), Ordering::Relaxed);
    unsafe {
        crate::analysis_engine::hirari_analysis_engine_update(
            core.analysis,
            left.as_ptr(),
            right.as_ptr(),
            frames as u32,
            dsp.sample_rate,
        );
        crate::goniometer::hirari_goniometer_process(
            core.goniometer,
            left.as_ptr(),
            right.as_ptr(),
            frames,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_get(state: *const c_void, field: u32) -> f32 {
    if state.is_null() {
        return 0.0;
    }
    let core = unsafe { &*state.cast::<MeterCore>() };
    let value = match field {
        0 => core.peak_l.load(Ordering::Relaxed),
        1 => core.peak_r.load(Ordering::Relaxed),
        2 => core.true_peak_l.load(Ordering::Relaxed),
        3 => core.true_peak_r.load(Ordering::Relaxed),
        4 => core.rms_l.load(Ordering::Relaxed),
        5 => core.rms_r.load(Ordering::Relaxed),
        _ => return 0.0,
    };
    f32::from_bits(value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_analysis_stats(
    state: *const c_void,
    output: *mut f32,
) {
    if let Some(core) = unsafe { state.cast::<MeterCore>().as_ref() } {
        unsafe { crate::analysis_engine::hirari_analysis_engine_get_stats(core.analysis, output) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_spectrum_band(
    state: *const c_void,
    channel: u32,
    band: u32,
) -> f32 {
    let Some(core) = (unsafe { state.cast::<MeterCore>().as_ref() }) else {
        return 0.0;
    };
    unsafe { crate::analysis_engine::hirari_analysis_engine_get_band(core.analysis, channel, band) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_meter_goniometer(
    state: *const c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    correlation: *mut f32,
    balance: *mut f32,
) -> bool {
    let Some(core) = (unsafe { state.cast::<MeterCore>().as_ref() }) else {
        return false;
    };
    unsafe {
        crate::goniometer::hirari_goniometer_snapshot(
            core.goniometer,
            left,
            right,
            frames,
            correlation,
            balance,
        )
    }
}
