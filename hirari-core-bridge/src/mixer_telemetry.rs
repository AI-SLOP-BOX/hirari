pub struct TelemetryDataRust {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
    pub clipping_count: u32,
    pub dc_offset_l: f32,
    pub dc_offset_r: f32,
    pub phase_correlation: f32,
    pub loudness_lufs: f32,
    pub spectrum_rms: [f32; 8],
}

/// Computes per-track telemetry and the legacy linear peaks published by the
/// native callback. Pointer tables are prepared by the host; this allocates
/// nothing. Peak publication remains available when telemetry cannot index a
/// long-lived track ID.
#[no_mangle]
pub unsafe extern "C" fn hirari_mixer_track_peaks(
    telemetry_state: *const c_void,
    track_ids: *const u32,
    left_channels: *const *const f32,
    right_channels: *const *const f32,
    track_count: u32,
    frames: u32,
    peaks_left: *mut f32,
    peaks_right: *mut f32,
) {
    if track_ids.is_null()
        || left_channels.is_null()
        || right_channels.is_null()
        || peaks_left.is_null()
        || peaks_right.is_null()
    {
        return;
    }
    let count = track_count as usize;
    let frames = frames as usize;
    let track_ids = unsafe { std::slice::from_raw_parts(track_ids, count) };
    let lefts = unsafe { std::slice::from_raw_parts(left_channels, count) };
    let rights = unsafe { std::slice::from_raw_parts(right_channels, count) };
    let output_left = unsafe { std::slice::from_raw_parts_mut(peaks_left, count) };
    let output_right = unsafe { std::slice::from_raw_parts_mut(peaks_right, count) };
    let hub = unsafe { telemetry_state.cast::<RealtimeTelemetryHub>().as_ref() };
    for index in 0..count {
        if !lefts[index].is_null() && !rights[index].is_null() {
            let left = unsafe { std::slice::from_raw_parts(lefts[index], frames) };
            let right = unsafe { std::slice::from_raw_parts(rights[index], frames) };
            let snapshot = hub.and_then(|hub| hub.push(track_ids[index], left, right));
            if let Some(metrics) = snapshot {
                output_left[index] = metrics.peak_l;
                output_right[index] = metrics.peak_r;
            } else {
                output_left[index] = left.iter().fold(0.0f32, |peak, sample| {
                    if sample.is_finite() {
                        peak.max(sample.abs())
                    } else {
                        peak
                    }
                });
                output_right[index] = right.iter().fold(0.0f32, |peak, sample| {
                    if sample.is_finite() {
                        peak.max(sample.abs())
                    } else {
                        peak
                    }
                });
            }
        } else {
            output_left[index] = 0.0;
            output_right[index] = 0.0;
        }
    }
}

impl TelemetryDataRust {
    pub fn headroom_dbfs(&self) -> f32 {
        let peak = self.peak_l.abs().max(self.peak_r.abs());
        if !peak.is_finite() || peak <= 0.0 {
            120.0
        } else {
            (-20.0 * peak.log10()).clamp(-120.0, 120.0)
        }
    }
    pub fn clipping(&self) -> bool {
        let peak = self.peak_l.abs().max(self.peak_r.abs());
        peak.is_finite() && peak > 1.0
    }
}

pub struct TelemetryOrchestrator {
    pub data: Vec<TelemetryDataRust>,
}

impl Default for TelemetryOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryOrchestrator {
    pub fn new() -> Self {
        let mut data = Vec::with_capacity(4096);
        for _ in 0..4096 {
            data.push(TelemetryDataRust {
                peak_l: 0.0,
                peak_r: 0.0,
                rms_l: 0.0,
                rms_r: 0.0,
                clipping_count: 0,
                dc_offset_l: 0.0,
                dc_offset_r: 0.0,
                phase_correlation: 0.0,
                loudness_lufs: -120.0,
                spectrum_rms: [0.0; 8],
            });
        }
        Self { data }
    }

    /// Analyzes an audio block and stores its peak, RMS, and clipping metrics.
    pub fn push_audio_block(&mut self, track_id: u32, l: &[f32], r: &[f32]) {
        let Some(target) = self.data.get_mut(track_id as usize) else {
            return;
        };

        // A stereo block is defined by the frames present in both channels.  This
        // also prevents a short channel from causing an out-of-bounds access.
        let frame_count = l.len().min(r.len());
        if frame_count == 0 {
            return;
        }

        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        let mut sum_sq_l = 0.0f64;
        let mut sum_sq_r = 0.0f64;
        let mut sum_l = 0.0f64;
        let mut sum_r = 0.0f64;
        let mut valid_l = 0usize;
        let mut valid_r = 0usize;
        let mut clipping_count = 0u32;
        let mut cross = 0.0f64;
        let mut band_energy = [0.0f64; 8];

        for (&sample_l, &sample_r) in l.iter().zip(r.iter()).take(frame_count) {
            if sample_l.is_finite() {
                let magnitude = sample_l.abs();
                peak_l = peak_l.max(magnitude);
                sum_sq_l += f64::from(sample_l) * f64::from(sample_l);
                sum_l += f64::from(sample_l);
                valid_l += 1;
                if magnitude > 1.0 {
                    clipping_count = clipping_count.saturating_add(1);
                }
            }
            if sample_r.is_finite() {
                let magnitude = sample_r.abs();
                peak_r = peak_r.max(magnitude);
                sum_sq_r += f64::from(sample_r) * f64::from(sample_r);
                sum_r += f64::from(sample_r);
                if sample_l.is_finite() {
                    cross += f64::from(sample_l) * f64::from(sample_r);
                }
                valid_r += 1;
                if magnitude > 1.0 {
                    clipping_count = clipping_count.saturating_add(1);
                }
            }
        }

        target.peak_l = peak_l;
        target.peak_r = peak_r;
        target.rms_l = if valid_l == 0 {
            0.0
        } else {
            (sum_sq_l / valid_l as f64).sqrt() as f32
        };
        target.rms_r = if valid_r == 0 {
            0.0
        } else {
            (sum_sq_r / valid_r as f64).sqrt() as f32
        };
        target.dc_offset_l = if valid_l == 0 {
            0.0
        } else {
            (sum_l / valid_l as f64) as f32
        };
        target.dc_offset_r = if valid_r == 0 {
            0.0
        } else {
            (sum_r / valid_r as f64) as f32
        };
        target.clipping_count = clipping_count;
        let denom = (sum_sq_l * sum_sq_r).sqrt();
        target.phase_correlation = if denom > 1.0e-12 {
            (cross / denom).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };
        let mean_square =
            ((sum_sq_l + sum_sq_r) / (valid_l.max(valid_r).max(1) as f64 * 2.0)).max(1.0e-12);
        target.loudness_lufs = (10.0 * mean_square.log10() - 0.691).max(-120.0) as f32;
        // Eight coarse energy bands are inexpensive and deterministic; the UI
        // can render a spectrum without allocating an FFT on the audio path.
        for (index, (&a, &b)) in l.iter().zip(r.iter()).enumerate().take(frame_count) {
            if a.is_finite() && b.is_finite() {
                band_energy[index * 8 / frame_count.max(1)] +=
                    ((a as f64 * a as f64) + (b as f64 * b as f64)) * 0.5;
            }
        }
        for (index, energy) in band_energy.into_iter().enumerate() {
            target.spectrum_rms[index] = (energy / frame_count.max(1) as f64).sqrt() as f32;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide diagnostic state.
    pub fn audit_mixer_telemetry(&self) -> bool {
        self.data.iter().all(|item| {
            item.peak_l.is_finite()
                && item.peak_r.is_finite()
                && item.rms_l.is_finite()
                && item.rms_r.is_finite()
                && item.dc_offset_l.is_finite()
                && item.dc_offset_r.is_finite()
                && item.phase_correlation.is_finite()
                && item.loudness_lufs.is_finite()
                && item.spectrum_rms.iter().all(|v| v.is_finite() && *v >= 0.0)
                && item.peak_l >= 0.0
                && item.peak_r >= 0.0
                && item.rms_l >= 0.0
                && item.rms_r >= 0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_reports_clipping_and_dc_offset() {
        let mut telemetry = TelemetryOrchestrator::new();
        telemetry.push_audio_block(0, &[1.2, -0.5, 0.5], &[0.25, 0.25, 0.25]);
        let item = &telemetry.data[0];
        assert_eq!(item.clipping_count, 1);
        assert!((item.peak_l - 1.2).abs() < f32::EPSILON);
        assert!((item.rms_r - 0.25).abs() < f32::EPSILON);
        assert!(telemetry.audit_mixer_telemetry());
    }

    #[test]
    fn non_finite_input_does_not_poison_telemetry() {
        let mut telemetry = TelemetryOrchestrator::new();
        telemetry.push_audio_block(0, &[f32::NAN, f32::INFINITY], &[f32::NEG_INFINITY, 0.5]);
        let item = &telemetry.data[0];
        assert_eq!(item.peak_l, 0.0);
        assert!((item.peak_r - 0.5).abs() < f32::EPSILON);
        assert_eq!(item.rms_l, 0.0);
        assert!(telemetry.audit_mixer_telemetry());
    }

    #[test]
    fn telemetry_audit_rejects_non_finite_diagnostic_state() {
        let mut telemetry = TelemetryOrchestrator::new();
        telemetry.data[0].dc_offset_l = f32::NAN;
        assert!(!telemetry.audit_mixer_telemetry());
    }
}
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariMixerTelemetrySnapshot {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
    pub clipping_count: u32,
    pub dc_offset_l: f32,
    pub dc_offset_r: f32,
    pub phase_correlation: f32,
    pub loudness_lufs: f32,
    pub spectrum_rms: [f32; 8],
}

struct RealtimeTelemetrySlot {
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    rms_l: AtomicU32,
    rms_r: AtomicU32,
    clipping_count: AtomicU32,
    dc_offset_l: AtomicU32,
    dc_offset_r: AtomicU32,
    phase_correlation: AtomicU32,
    loudness_lufs: AtomicU32,
    spectrum_rms: [AtomicU32; 8],
}

impl RealtimeTelemetrySlot {
    fn new() -> Self {
        Self {
            peak_l: AtomicU32::new(0.0f32.to_bits()),
            peak_r: AtomicU32::new(0.0f32.to_bits()),
            rms_l: AtomicU32::new(0.0f32.to_bits()),
            rms_r: AtomicU32::new(0.0f32.to_bits()),
            clipping_count: AtomicU32::new(0),
            dc_offset_l: AtomicU32::new(0.0f32.to_bits()),
            dc_offset_r: AtomicU32::new(0.0f32.to_bits()),
            phase_correlation: AtomicU32::new(0.0f32.to_bits()),
            loudness_lufs: AtomicU32::new((-120.0f32).to_bits()),
            spectrum_rms: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
        }
    }

    fn publish(&self, snapshot: HirariMixerTelemetrySnapshot) {
        self.peak_l
            .store(snapshot.peak_l.to_bits(), Ordering::Relaxed);
        self.peak_r
            .store(snapshot.peak_r.to_bits(), Ordering::Relaxed);
        self.rms_l
            .store(snapshot.rms_l.to_bits(), Ordering::Relaxed);
        self.rms_r
            .store(snapshot.rms_r.to_bits(), Ordering::Relaxed);
        self.clipping_count
            .store(snapshot.clipping_count, Ordering::Relaxed);
        self.dc_offset_l
            .store(snapshot.dc_offset_l.to_bits(), Ordering::Relaxed);
        self.dc_offset_r
            .store(snapshot.dc_offset_r.to_bits(), Ordering::Relaxed);
        self.phase_correlation
            .store(snapshot.phase_correlation.to_bits(), Ordering::Relaxed);
        self.loudness_lufs
            .store(snapshot.loudness_lufs.to_bits(), Ordering::Relaxed);
        for (target, value) in self.spectrum_rms.iter().zip(snapshot.spectrum_rms) {
            target.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> HirariMixerTelemetrySnapshot {
        HirariMixerTelemetrySnapshot {
            peak_l: f32::from_bits(self.peak_l.load(Ordering::Relaxed)),
            peak_r: f32::from_bits(self.peak_r.load(Ordering::Relaxed)),
            rms_l: f32::from_bits(self.rms_l.load(Ordering::Relaxed)),
            rms_r: f32::from_bits(self.rms_r.load(Ordering::Relaxed)),
            clipping_count: self.clipping_count.load(Ordering::Relaxed),
            dc_offset_l: f32::from_bits(self.dc_offset_l.load(Ordering::Relaxed)),
            dc_offset_r: f32::from_bits(self.dc_offset_r.load(Ordering::Relaxed)),
            phase_correlation: f32::from_bits(self.phase_correlation.load(Ordering::Relaxed)),
            loudness_lufs: f32::from_bits(self.loudness_lufs.load(Ordering::Relaxed)),
            spectrum_rms: std::array::from_fn(|index| {
                f32::from_bits(self.spectrum_rms[index].load(Ordering::Relaxed))
            }),
        }
    }
}

struct RealtimeTelemetryHub {
    tracks: Box<[RealtimeTelemetrySlot]>,
}

impl RealtimeTelemetryHub {
    fn new() -> Self {
        let tracks = (0..4096)
            .map(|_| RealtimeTelemetrySlot::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self { tracks }
    }

    fn push(
        &self,
        track_id: u32,
        left: &[f32],
        right: &[f32],
    ) -> Option<HirariMixerTelemetrySnapshot> {
        let Some(slot) = self.tracks.get(track_id as usize) else {
            return None;
        };
        let frames = left.len().min(right.len());
        if frames == 0 {
            return None;
        }
        let mut result = HirariMixerTelemetrySnapshot {
            loudness_lufs: -120.0,
            ..HirariMixerTelemetrySnapshot::default()
        };
        let mut sum_sq_l = 0.0f64;
        let mut sum_sq_r = 0.0f64;
        let mut sum_l = 0.0f64;
        let mut sum_r = 0.0f64;
        let mut valid_l = 0usize;
        let mut valid_r = 0usize;
        let mut cross = 0.0f64;
        let mut bands = [0.0f64; 8];
        for (&l, &r) in left.iter().zip(right).take(frames) {
            if l.is_finite() {
                result.peak_l = result.peak_l.max(l.abs());
                sum_sq_l += f64::from(l) * f64::from(l);
                sum_l += f64::from(l);
                valid_l += 1;
                if l.abs() > 1.0 {
                    result.clipping_count = result.clipping_count.saturating_add(1);
                }
            }
            if r.is_finite() {
                result.peak_r = result.peak_r.max(r.abs());
                sum_sq_r += f64::from(r) * f64::from(r);
                sum_r += f64::from(r);
                valid_r += 1;
                if l.is_finite() {
                    cross += f64::from(l) * f64::from(r);
                }
                if r.abs() > 1.0 {
                    result.clipping_count = result.clipping_count.saturating_add(1);
                }
            }
        }
        result.rms_l = if valid_l == 0 {
            0.0
        } else {
            (sum_sq_l / valid_l as f64).sqrt() as f32
        };
        result.rms_r = if valid_r == 0 {
            0.0
        } else {
            (sum_sq_r / valid_r as f64).sqrt() as f32
        };
        result.dc_offset_l = if valid_l == 0 {
            0.0
        } else {
            (sum_l / valid_l as f64) as f32
        };
        result.dc_offset_r = if valid_r == 0 {
            0.0
        } else {
            (sum_r / valid_r as f64) as f32
        };
        let denominator = (sum_sq_l * sum_sq_r).sqrt();
        result.phase_correlation = if denominator > 1.0e-12 {
            (cross / denominator).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };
        let mean_square =
            ((sum_sq_l + sum_sq_r) / (valid_l.max(valid_r).max(1) as f64 * 2.0)).max(1.0e-12);
        result.loudness_lufs = (10.0 * mean_square.log10() - 0.691).max(-120.0) as f32;
        for (index, (&l, &r)) in left.iter().zip(right).take(frames).enumerate() {
            if l.is_finite() && r.is_finite() {
                bands[index * 8 / frames] +=
                    (f64::from(l) * f64::from(l) + f64::from(r) * f64::from(r)) * 0.5;
            }
        }
        for (index, energy) in bands.into_iter().enumerate() {
            result.spectrum_rms[index] = (energy / frames as f64).sqrt() as f32;
        }
        slot.publish(result);
        Some(result)
    }
}

#[no_mangle]
pub extern "C" fn hirari_mixer_telemetry_create() -> *mut c_void {
    Box::into_raw(Box::new(RealtimeTelemetryHub::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mixer_telemetry_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<RealtimeTelemetryHub>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mixer_telemetry_push(
    state: *const c_void,
    track_id: u32,
    left: *const f32,
    right: *const f32,
    frames: u32,
) {
    let Some(hub) = (unsafe { state.cast::<RealtimeTelemetryHub>().as_ref() }) else {
        return;
    };
    if frames == 0 || left.is_null() || right.is_null() {
        return;
    }
    let left = unsafe { std::slice::from_raw_parts(left, frames as usize) };
    let right = unsafe { std::slice::from_raw_parts(right, frames as usize) };
    let _ = hub.push(track_id, left, right);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mixer_telemetry_read(
    state: *const c_void,
    track_id: u32,
    output: *mut HirariMixerTelemetrySnapshot,
) -> bool {
    let Some(hub) = (unsafe { state.cast::<RealtimeTelemetryHub>().as_ref() }) else {
        return false;
    };
    let Some(slot) = hub.tracks.get(track_id as usize) else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    unsafe { output.write(slot.snapshot()) };
    true
}
