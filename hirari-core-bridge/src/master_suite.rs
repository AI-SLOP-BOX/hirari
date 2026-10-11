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
        // The meter follows the suite's own sample rate. It used to be pinned
        // to 44.1 kHz here, which left the RMS ballistics (a 300 ms
        // integration scaled by sample rate) and the analysis engine's
        // frequency mapping wrong at 48 kHz and above, and a `prepare` that
        // reconfigured the DSP without the meter locked the mismatch in.
        meter: crate::master_meter::hirari_master_meter_create(sample_rate),
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
    // The meter carries its own sample-rate-dependent state (RMS integration
    // time, analysis engine), so a prepare that skips it leaves the meter
    // reporting for the rate it was created at.
    unsafe { crate::master_meter::hirari_master_meter_prepare(handle.meter, sample_rate) };
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

#[cfg(test)]
mod master_suite_tests {
    use super::*;

    const RMS_LEFT: u32 = 4;

    /// Feeds a constant full-scale signal for `blocks` blocks and returns the
    /// reported left RMS, in the same units the meter reports.
    fn rms_after(sample_rate: f64, blocks: usize) -> f32 {
        unsafe {
            let handle = hirari_master_suite_create(sample_rate);
            assert!(!handle.is_null());
            // The suite's DSP must not clip the measurement away: leave every
            // processor at its neutral default.
            let mut left = vec![0.5_f32; 512];
            let mut right = vec![0.5_f32; 512];
            for _ in 0..blocks {
                hirari_master_suite_process(
                    handle,
                    left.as_mut_ptr(),
                    right.as_mut_ptr(),
                    left.len(),
                );
            }
            let value = hirari_master_suite_meter_value(handle, RMS_LEFT);
            hirari_master_suite_destroy(handle);
            value
        }
    }

    #[test]
    fn the_meter_follows_the_suite_sample_rate() {
        // The RMS integrator is a 300 ms one-pole whose coefficient scales
        // with the sample rate, so the same number of frames must converge to
        // a different value at different rates. If the meter were pinned to
        // 44.1 kHz both readings would be identical.
        let at_44k = rms_after(44_100.0, 8);
        let at_96k = rms_after(96_000.0, 8);
        assert!(
            (at_44k - at_96k).abs() > 1.0e-4,
            "RMS ballistics must follow the sample rate: 44.1k={at_44k} 96k={at_96k}"
        );
        // The higher rate has seen less time for the same frame count, so it
        // must have converged less, not more.
        assert!(
            at_96k.abs() < at_44k.abs(),
            "96k has processed less wall-clock time: 44.1k={at_44k} 96k={at_96k}"
        );
    }

    #[test]
    fn an_out_of_range_rate_still_falls_back_rather_than_silencing() {
        // Both the suite and the meter clamp an unusable rate to their default,
        // so an invalid rate must not produce a dead meter.
        let value = rms_after(0.0, 4);
        assert!(value.is_finite());
    }

    #[test]
    fn prepare_reconfigures_the_meter_not_only_the_dsp() {
        unsafe {
            let handle = hirari_master_suite_create(44_100.0);
            assert!(!handle.is_null());
            let before = {
                let mut left = vec![0.5_f32; 512];
                let mut right = vec![0.5_f32; 512];
                for _ in 0..8 {
                    hirari_master_suite_process(
                        handle,
                        left.as_mut_ptr(),
                        right.as_mut_ptr(),
                        left.len(),
                    );
                }
                hirari_master_suite_meter_value(handle, RMS_LEFT)
            };
            // Reconfigure to a rate that needs twice the frames for the same
            // integration time, then measure again from a reset.
            hirari_master_suite_prepare(handle, 96_000.0);
            hirari_master_suite_reset(handle);
            let after = {
                let mut left = vec![0.5_f32; 512];
                let mut right = vec![0.5_f32; 512];
                for _ in 0..8 {
                    hirari_master_suite_process(
                        handle,
                        left.as_mut_ptr(),
                        right.as_mut_ptr(),
                        left.len(),
                    );
                }
                hirari_master_suite_meter_value(handle, RMS_LEFT)
            };
            hirari_master_suite_destroy(handle);
            assert!(
                (before - after).abs() > 1.0e-4,
                "prepare must reconfigure the meter: before={before} after={after}"
            );
        }
    }
}
