#[derive(Debug, Clone, Copy)]
pub enum FadeCurve {
    Linear,
    EqualPower,
    EaseInOut,
    Bezier,
}

pub struct FadeOrchestrator;

impl Default for FadeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl FadeOrchestrator {
    pub fn new() -> Self {
        Self
    }

    /// INDUSTRIAL: High-Precision Gain Calculation for Fades
    pub fn get_fade_factor(
        &self,
        pos: usize,
        length: usize,
        is_fade_in: bool,
        curve_type: FadeCurve,
        curvature: f32,
    ) -> f32 {
        if length <= 1 {
            return if is_fade_in { 1.0 } else { 0.0 };
        }
        let t = (pos as f32 / (length - 1) as f32).clamp(0.0, 1.0);
        match curve_type {
            FadeCurve::EqualPower => {
                let phase = t * std::f32::consts::FRAC_PI_2;
                if is_fade_in {
                    phase.sin()
                } else {
                    phase.cos()
                }
            }
            FadeCurve::EaseInOut => {
                let curved = t * t * (3.0 - 2.0 * t);
                if is_fade_in {
                    curved
                } else {
                    1.0 - curved
                }
            }
            FadeCurve::Bezier => {
                let c = curvature.clamp(0.0, 1.0);
                let curved = (1.0 - c) * t * t + c * (1.0 - (1.0 - t) * (1.0 - t));
                if is_fade_in {
                    curved
                } else {
                    1.0 - curved
                }
            }
            FadeCurve::Linear => {
                if is_fade_in {
                    t
                } else {
                    1.0 - t
                }
            }
        }
    }

    /// INDUSTRIAL: Applies a crossfade between two buffers.
    pub fn apply(&self, out: &mut [f32], in1: &[f32], in2: &[f32]) {
        // Crossfades are fed by regions with independently edited lengths.
        // Never index past the shorter source and avoid 0/0 for empty edits.
        let num_frames = out.len().min(in1.len()).min(in2.len());
        if num_frames == 0 {
            out.fill(0.0);
            return;
        }
        for i in 0..num_frames {
            let t = if num_frames == 1 {
                1.0
            } else {
                i as f32 / (num_frames - 1) as f32
            };
            let gain1 = (t * std::f32::consts::PI * 0.5).cos();
            let gain2 = (t * std::f32::consts::PI * 0.5).sin();
            out[i] = (in1[i] * gain1) + (in2[i] * gain2);
        }
        out[num_frames..].fill(0.0);
    }

    /// INDUSTRIAL: Automatically prevents clicks at region boundaries.
    pub fn apply_micro_fade(&self, buffer: &mut [f32], is_fade_in: bool) {
        let num_frames = buffer.len();
        for i in 0..num_frames {
            let t = if num_frames > 1 {
                i as f32 / (num_frames - 1) as f32
            } else {
                1.0
            };
            buffer[i] *= if is_fade_in { t } else { 1.0 - t };
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fade_apply(
    output: *mut f32,
    input_a: *const f32,
    input_b: *const f32,
    frames: usize,
) {
    if output.is_null() || input_a.is_null() || input_b.is_null() {
        return;
    }
    let orchestrator = FadeOrchestrator::new();
    let output = std::slice::from_raw_parts_mut(output, frames);
    let input_a = std::slice::from_raw_parts(input_a, frames);
    let input_b = std::slice::from_raw_parts(input_b, frames);
    orchestrator.apply(output, input_a, input_b);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fade_apply_micro(buffer: *mut f32, frames: usize, fade_in: bool) {
    if buffer.is_null() {
        return;
    }
    FadeOrchestrator::new()
        .apply_micro_fade(std::slice::from_raw_parts_mut(buffer, frames), fade_in);
}

#[no_mangle]
pub extern "C" fn hirari_fade_factor(
    position: usize,
    length: usize,
    fade_in: bool,
    curve: u8,
    curvature: f32,
) -> f32 {
    let curve = match curve {
        1 => FadeCurve::EqualPower,
        2 => FadeCurve::EaseInOut,
        3 => FadeCurve::Bezier,
        _ => FadeCurve::Linear,
    };
    FadeOrchestrator::new().get_fade_factor(position, length, fade_in, curve, curvature)
}

#[cfg(test)]
mod tests {
    use super::FadeOrchestrator;

    #[test]
    fn crossfade_handles_empty_and_short_sources() {
        let fades = FadeOrchestrator::new();
        let mut empty = [1.0f32];
        fades.apply(&mut empty, &[], &[]);
        assert_eq!(empty, [0.0]);

        let mut output = [9.0f32; 4];
        fades.apply(&mut output, &[1.0, 1.0], &[0.0, 0.0]);
        assert!(output[0].is_finite());
        assert!(output[1].is_finite());
        assert_eq!(&output[2..], &[0.0, 0.0]);
    }

    #[test]
    fn single_sample_crossfade_is_finite() {
        let fades = FadeOrchestrator::new();
        let mut output = [0.0f32];
        fades.apply(&mut output, &[1.0], &[1.0]);
        assert!(output[0].is_finite());
    }
}
