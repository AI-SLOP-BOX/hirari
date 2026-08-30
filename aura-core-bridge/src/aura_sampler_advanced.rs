pub struct SampleLayer {
    pub data: Vec<f32>,
    pub min_velocity: u8,
    pub max_velocity: u8,
}

pub struct AuraSamplerAdvancedEngine {
    pub sample_rate: f64,
    pub layers: Vec<SampleLayer>,
    pub active_layer_idx: Option<usize>,
    pub playback_pos: usize,
}

impl AuraSamplerAdvancedEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            layers: Vec::new(),
            active_layer_idx: None,
            playback_pos: 0,
        }
    }

    pub fn add_layer(&mut self, data: Vec<f32>, min_v: u8, max_v: u8) {
        self.layers.push(SampleLayer {
            data,
            min_velocity: min_v,
            max_velocity: max_v,
        });
    }

    pub fn trigger(&mut self, velocity: u8) {
        for (idx, layer) in self.layers.iter().enumerate() {
            if velocity >= layer.min_velocity && velocity <= layer.max_velocity {
                self.active_layer_idx = Some(idx);
                self.playback_pos = 0;
                return;
            }
        }
        self.active_layer_idx = None;
    }

    /// INDUSTRIAL: Processes an audio block with multisampled audio.
    pub fn process(&mut self, out: &mut [f32]) {
        if let Some(idx) = self.active_layer_idx {
            if idx >= self.layers.len() {
                self.active_layer_idx = None;
                self.playback_pos = 0;
                return;
            }
            let layer = &self.layers[idx];
            let num_frames = out.len();

            for i in 0..num_frames {
                if self.playback_pos < layer.data.len() {
                    let sample = layer.data[self.playback_pos];
                    out[i] += if sample.is_finite() { sample } else { 0.0 };
                    self.playback_pos += 1;
                } else {
                    self.active_layer_idx = None;
                    break;
                }
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Aura Sampler Advanced state.
    pub fn audit_aura_sampler_advanced(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 100.0
            && self.layers.iter().all(|layer| {
                layer.min_velocity <= layer.max_velocity
                    && layer.data.iter().all(|sample| sample.is_finite())
            })
            && self.active_layer_idx.is_none_or(|idx| {
                idx < self.layers.len() && self.playback_pos <= self.layers[idx].data.len()
            })
    }
}
