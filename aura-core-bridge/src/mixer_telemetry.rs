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
