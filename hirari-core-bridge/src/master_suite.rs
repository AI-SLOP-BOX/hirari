use crate::fet_compressor::FetCompressorEngine;
use crate::tpdf_dither::TpdfDitherEngine;
use crate::true_peak_limiter::TruePeakLimiterEngine;
use crate::virtuoso_pultec::VirtuosoPultecEngine;
use crate::virtuoso_tape::VirtuosoTapeEngine;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

struct MasterSuiteDsp {
    auto_gain_offset: f32,
    pultec: VirtuosoPultecEngine,
    tape: VirtuosoTapeEngine,
    compressor: FetCompressorEngine,
    limiter: TruePeakLimiterEngine,
    dither_l: TpdfDitherEngine,
    dither_r: TpdfDitherEngine,
}

impl MasterSuiteDsp {
    fn new(sample_rate: f64) -> Self {
        let compressor = FetCompressorEngine::new(sample_rate);
        compressor.set_threshold(-20.0);
        compressor.set_ratio(2);
        compressor.set_attack(30.0);
        compressor.set_release(100.0);
        Self {
            auto_gain_offset: 0.0,
            pultec: VirtuosoPultecEngine::new(sample_rate),
            tape: VirtuosoTapeEngine::new(sample_rate),
            compressor,
            // MasterSuite's C++ limiter was default-constructed and never
            // prepared, so preserve its 44.1 kHz delay until that contract changes.
            limiter: TruePeakLimiterEngine::new(44_100.0),
            dither_l: TpdfDitherEngine::new(),
            dither_r: TpdfDitherEngine::new(),
        }
    }

    fn prepare_to_play(&mut self, sample_rate: f64) {
        self.pultec.prepare_to_play(sample_rate);
        self.tape.prepare_to_play(sample_rate);
        self.compressor.prepare_to_play(sample_rate);
    }

    fn reset(&mut self) {
        self.pultec.reset();
        self.tape.reset();
        self.compressor.reset();
        self.limiter.reset();
        self.auto_gain_offset = 0.0;
    }

    fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        auto_gain_enabled: bool,
        target_lufs: f32,
        measured_short_term_lufs: f32,
        mid_side_enabled: bool,
        stereo_width: f32,
        dither_enabled: bool,
    ) {
        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }

        if auto_gain_enabled
            && measured_short_term_lufs.is_finite()
            && measured_short_term_lufs > -100.0
        {
            let target_offset = (target_lufs - measured_short_term_lufs).clamp(-12.0, 12.0);
            self.auto_gain_offset += 0.02 * (target_offset - self.auto_gain_offset);
            let gain = 10.0_f32.powf(self.auto_gain_offset / 20.0);
            for frame in 0..frames {
                left[frame] *= gain;
                right[frame] *= gain;
            }
        }

        self.pultec.process(left, Some(right));
        self.tape.process(left, Some(right));
        self.compressor.process_stereo(left, right);

        if mid_side_enabled {
            let width = stereo_width.clamp(0.0, 2.0);
            const INV_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;
            for frame in 0..frames {
                let mid = (left[frame] + right[frame]) * INV_SQRT_2;
                let side = (left[frame] - right[frame]) * INV_SQRT_2 * width;
                left[frame] = (mid + side) * INV_SQRT_2;
                right[frame] = (mid - side) * INV_SQRT_2;
            }
        }

        self.limiter.process(left, right, 0.0, -0.1);
        if dither_enabled {
            self.dither_l.process(&mut left[..frames]);
            self.dither_r.process(&mut right[..frames]);
        }
    }
}

struct MasterSuiteHandle {
    dsp: UnsafeCell<MasterSuiteDsp>,
    auto_gain_bits: AtomicU32,
    auto_gain_enabled: AtomicBool,
    target_lufs_bits: AtomicU32,
    mid_side_enabled: AtomicBool,
    stereo_width_bits: AtomicU32,
    dither_enabled: AtomicBool,
    meter: *mut c_void,
}

#[no_mangle]
pub extern "C" fn hirari_master_suite_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(MasterSuiteHandle {
        dsp: UnsafeCell::new(MasterSuiteDsp::new(sample_rate)),
        auto_gain_bits: AtomicU32::new(1.0_f32.to_bits()),
        auto_gain_enabled: AtomicBool::new(false),
        target_lufs_bits: AtomicU32::new((-14.0_f32).to_bits()),
        mid_side_enabled: AtomicBool::new(false),
        stereo_width_bits: AtomicU32::new(1.0_f32.to_bits()),
        dither_enabled: AtomicBool::new(false),
        meter: crate::master_meter::hirari_master_meter_create(44_100.0),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        let handle = unsafe { Box::from_raw(handle.cast::<MasterSuiteHandle>()) };
        unsafe { crate::master_meter::hirari_master_meter_destroy(handle.meter) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_prepare(handle: *mut c_void, sample_rate: f64) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*handle.cast::<MasterSuiteHandle>() };
    let dsp = unsafe { &mut *handle.dsp.get() };
    dsp.prepare_to_play(sample_rate);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_reset(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { &*handle.cast::<MasterSuiteHandle>() };
    let dsp = unsafe { &mut *handle.dsp.get() };
    dsp.reset();
    handle
        .auto_gain_bits
        .store(1.0_f32.to_bits(), Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_process(
    handle: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if handle.is_null() || left.is_null() || right.is_null() {
        return;
    }
    let handle = unsafe { &*handle.cast::<MasterSuiteHandle>() };
    let dsp = unsafe { &mut *handle.dsp.get() };
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
    let auto_gain_enabled = handle.auto_gain_enabled.load(Ordering::Relaxed);
    let mut meter_stats = [-70.0_f32, -70.0, -70.0, -100.0];
    unsafe {
        crate::master_meter::hirari_master_meter_analysis_stats(
            handle.meter,
            meter_stats.as_mut_ptr(),
        );
    }
    let measured_short_term_lufs = if auto_gain_enabled {
        meter_stats[0]
    } else {
        -120.0
    };
    dsp.process(
        left,
        right,
        auto_gain_enabled,
        f32::from_bits(handle.target_lufs_bits.load(Ordering::Relaxed)),
        measured_short_term_lufs,
        handle.mid_side_enabled.load(Ordering::Relaxed),
        f32::from_bits(handle.stereo_width_bits.load(Ordering::Relaxed)),
        handle.dither_enabled.load(Ordering::Relaxed),
    );
    unsafe {
        crate::master_meter::hirari_master_meter_process(
            handle.meter,
            left.as_ptr(),
            right.as_ptr(),
            frames,
        );
    }
    handle.auto_gain_bits.store(
        10.0_f32.powf(dsp.auto_gain_offset / 20.0).to_bits(),
        Ordering::Relaxed,
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_gain(handle: *const c_void) -> f32 {
    if handle.is_null() {
        return 1.0;
    }
    let handle = unsafe { &*handle.cast::<MasterSuiteHandle>() };
    f32::from_bits(handle.auto_gain_bits.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_set_auto_gain(
    handle: *const c_void,
    enabled: bool,
    target_lufs: f32,
) {
    let Some(handle) = (unsafe { handle.cast::<MasterSuiteHandle>().as_ref() }) else {
        return;
    };
    handle.target_lufs_bits.store(
        if target_lufs.is_finite() {
            target_lufs.clamp(-60.0, 0.0)
        } else {
            -14.0
        }
        .to_bits(),
        Ordering::Relaxed,
    );
    handle.auto_gain_enabled.store(enabled, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_set_mid_side(handle: *const c_void, enabled: bool) {
    if let Some(handle) = unsafe { handle.cast::<MasterSuiteHandle>().as_ref() } {
        handle.mid_side_enabled.store(enabled, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_set_width(handle: *const c_void, width: f32) {
    if let Some(handle) = unsafe { handle.cast::<MasterSuiteHandle>().as_ref() } {
        let width = if width.is_finite() {
            width.clamp(0.0, 2.0)
        } else {
            1.0
        };
        handle
            .stereo_width_bits
            .store(width.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_set_dither(handle: *const c_void, enabled: bool) {
    if let Some(handle) = unsafe { handle.cast::<MasterSuiteHandle>().as_ref() } {
        handle.dither_enabled.store(enabled, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_meter_value(handle: *const c_void, field: u32) -> f32 {
    let Some(handle) = (unsafe { handle.cast::<MasterSuiteHandle>().as_ref() }) else {
        return 0.0;
    };
    unsafe { crate::master_meter::hirari_master_meter_get(handle.meter, field) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_meter_analysis_stats(
    handle: *const c_void,
    output: *mut f32,
) {
    if let Some(handle) = unsafe { handle.cast::<MasterSuiteHandle>().as_ref() } {
        unsafe { crate::master_meter::hirari_master_meter_analysis_stats(handle.meter, output) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_meter_spectrum_band(
    handle: *const c_void,
    channel: u32,
    band: u32,
) -> f32 {
    let Some(handle) = (unsafe { handle.cast::<MasterSuiteHandle>().as_ref() }) else {
        return 0.0;
    };
    unsafe { crate::master_meter::hirari_master_meter_spectrum_band(handle.meter, channel, band) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_suite_meter_goniometer(
    handle: *const c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    correlation: *mut f32,
    balance: *mut f32,
) -> bool {
    let Some(handle) = (unsafe { handle.cast::<MasterSuiteHandle>().as_ref() }) else {
        return false;
    };
    unsafe {
        crate::master_meter::hirari_master_meter_goniometer(
            handle.meter,
            left,
            right,
            frames,
            correlation,
            balance,
        )
    }
}
