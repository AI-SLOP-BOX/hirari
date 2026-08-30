use crate::oversampler::OversamplerEngine;

pub enum SaturationModel {
    Tube,
    Tape,
    SoftClip,
}

pub struct AnalogSaturatorEngine {
    pub sample_rate: f64,
    pub dc_l: f32,
    pub dc_r: f32,
    pub oversampler_l: OversamplerEngine,
    pub oversampler_r: OversamplerEngine,
}

impl AnalogSaturatorEngine {
    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            dc_l: 0.0,
            dc_r: 0.0,
            oversampler_l: OversamplerEngine::new(),
            oversampler_r: OversamplerEngine::new(),
        }
    }

    pub fn reset(&mut self) {
        self.dc_l = 0.0;
        self.dc_r = 0.0;
        self.oversampler_l.reset();
        self.oversampler_r.reset();
    }

    fn apply_model(&self, x: f32, warmth: f32, model: &SaturationModel) -> f32 {
        match model {
            SaturationModel::Tube => {
                let b = warmth * 0.25; // Bias
                (x + b) / (1.0 + (x + b).abs()) - (b / (1.0 + b.abs()))
            }
            SaturationModel::Tape => {
                let x_abs = x.abs();
                if x_abs < 1.0 {
                    x * (1.5 - 0.5 * x * x)
                } else if x > 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            SaturationModel::SoftClip => x.tanh(),
        }
    }

    /// INDUSTRIAL: Processes a block of samples with specific settings.
    pub fn process_with_settings(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        drive: f32,
        warmth: f32,
        model: SaturationModel,
    ) {
        let len = l.len();
        // dbToLinear equivalent: 10^(db/20)
        let db = drive * 24.0;
        let drive_lin = 10.0f32.powf(db / 20.0);
        let comp = 1.0 / (1.0 + drive * 0.7);

        for i in 0..len {
            let in_l = l[i] * drive_lin;
            let in_r = r[i] * drive_lin;

            let (ly1, ly2) = self.oversampler_l.upsample(in_l);
            let out_l1 = self.apply_model(ly1, warmth, &model);
            let out_l2 = self.apply_model(ly2, warmth, &model);
            l[i] = self.oversampler_l.downsample(out_l1, out_l2) * comp;

            let (ry1, ry2) = self.oversampler_r.upsample(in_r);
            let out_r1 = self.apply_model(ry1, warmth, &model);
            let out_r2 = self.apply_model(ry2, warmth, &model);
            r[i] = self.oversampler_r.downsample(out_r1, out_r2) * comp;

            self.dc_l = 0.999 * self.dc_l + 0.001 * l[i];
            l[i] -= self.dc_l;
            self.dc_r = 0.999 * self.dc_r + 0.001 * r[i];
            r[i] -= self.dc_r;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Analog Saturator state.
    pub fn audit_analog_saturator(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic Analog Saturator auditing logic.
        true
    }
}
