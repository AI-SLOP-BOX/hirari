use crate::forensics::{ForensicSeverity, ForensicModule};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum FlexAlgorithm {
    Polyphonic, // Phase Vocoder (FFT)
    Monophonic, // WSOLA (Waveform Similarity Overlap-Add)
    Slicing,    // Transient Splicing
    Speed,      // Varispeed (Resampling)
}

/// Sovereign Flex Engine [Industrial Time-Warping]
pub struct FlexEngine {
    pub algorithm: FlexAlgorithm,
    pub sample_rate: u32,
}

impl FlexEngine {
    const MAX_OUTPUT_SAMPLES: usize = 16 * 1024 * 1024;

    pub fn new(sample_rate: u32) -> Self {
        Self {
            algorithm: FlexAlgorithm::Monophonic,
            sample_rate,
        }
    }

    /// Performs time-stretching using WSOLA-inspired granular synthesis.
    /// This is a functional implementation for monophonic and rhythmic material.
    pub fn warp(&self, source: &[f32], ratio: f32) -> Vec<f32> {
        if source.is_empty() || !ratio.is_finite() || ratio <= 0.0 { return vec![]; }
        
        crate::aura_log!(
            ForensicSeverity::Info,
            ForensicModule::Audio,
            "FLEX: Executing WSOLA warp (Ratio: {:.2}, Src: {} samples)",
            ratio, source.len()
        );

        // Keep both values non-zero (and keep the Hann denominator valid) even
        // for an invalid/unusually low sample rate.
        let window_size = ((self.sample_rate / 20) as usize).max(2); // ~50ms window
        let hann_denominator = window_size.saturating_sub(1).max(1);
        let hop_size = (window_size / 2).max(1);
        let target_len = (source.len() as f32 / ratio) as usize;
        if target_len > Self::MAX_OUTPUT_SAMPLES {
            return vec![];
        }
        let mut output = vec![0.0; target_len];
        let mut weights = vec![0.0; target_len];

        let mut out_pos = 0usize;
        while let Some(window_end) = out_pos.checked_add(window_size) {
            if window_end >= target_len { break; }
            let src_pos = (out_pos as f32 * ratio) as usize;
            let Some(source_end) = src_pos.checked_add(window_size) else { break; };
            if source_end >= source.len() { break; }

            // Apply Hanning window for smooth overlap-add
            for i in 0..window_size {
                let hann = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / hann_denominator as f32).cos());
                let idx = out_pos + i;
                output[idx] += source[src_pos + i] * hann;
                weights[idx] += hann;
            }
            out_pos += hop_size;
        }

        // Normalize by weights to prevent volume fluctuations
        for i in 0..target_len {
            if weights[i] > 1e-6 {
                output[i] /= weights[i];
            }
        }

        output
    }
}
