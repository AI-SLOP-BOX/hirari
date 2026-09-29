#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum FlexAlgorithm {
    Polyphonic,
    Monophonic,
    Slicing,
    Speed,
}

pub struct FlexEngine {
    pub algorithm: FlexAlgorithm,
    pub sample_rate: u32,
}

impl FlexEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self { algorithm: FlexAlgorithm::Monophonic, sample_rate }
    }
    pub fn warp(&self, source: &[f32], ratio: f32) -> Vec<f32> {
        if source.is_empty() || ratio <= 0.0 { return vec![]; }
        let window_size = (self.sample_rate / 20) as usize;
        let hop_size = window_size / 2;
        let target_len = (source.len() as f32 / ratio) as usize;
        let mut output = vec![0.0; target_len];
        let mut weights = vec![0.0; target_len];
        let mut out_pos = 0usize;
        while out_pos + window_size < target_len {
            let src_pos = (out_pos as f32 * ratio) as usize;
            if src_pos + window_size >= source.len() { break; }
            for i in 0..window_size {
                let hann = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos());
                let idx = out_pos + i;
                output[idx] += source[src_pos + i] * hann;
                weights[idx] += hann;
            }
            out_pos += hop_size;
        }
        for i in 0..target_len {
            if weights[i] > 1e-6 { output[i] /= weights[i]; }
        }
        output
    }
}
