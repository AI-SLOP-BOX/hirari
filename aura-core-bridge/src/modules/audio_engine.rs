use std::sync::Arc;
use parking_lot::Mutex;
use crate::math::ParameterSmoother;

// ---------------------------------------------------------------------------
// Industrial Audio Metrics
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct AudioSpectrum {
    pub bands: Vec<f32>,
    pub peak_amplitude: f32,
    pub rms_level: f32,
    pub spectral_centroid: f32,
    pub is_beat: bool,
}

// ---------------------------------------------------------------------------
// Industrial DSP Engine [64-bit Precision]
// ---------------------------------------------------------------------------

pub struct AudioEngine {
    pub sample_rate: u32,
    pub lookahead_ms: f32,
    pub output_buffer: Mutex<Vec<f32>>,
    pub scratch_buffer: Mutex<Vec<f32>>,
}

impl AudioEngine {
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = if (8_000..=384_000).contains(&sample_rate) {
            sample_rate
        } else {
            48_000
        };
        Self { 
            sample_rate,
            lookahead_ms: 2.0,
            output_buffer: Mutex::new(vec![0.0; 1024]),
            scratch_buffer: Mutex::new(vec![0.0; 1024]),
        }
    }

    pub fn analyze_spectrum(&self, samples: &[f32]) -> AudioSpectrum {
        if samples.is_empty() { return AudioSpectrum::default(); }

        const BAND_COUNT: usize = 64;
        let mut bands = [0.0f32; BAND_COUNT];
        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        let mut total_energy = 0.0f32;
        let mut weighted_frequency = 0.0f32;

        for &sample in samples {
            let s = if sample.is_finite() { sample } else { 0.0 };
            peak = peak.max(s.abs());
            sum_sq += (s as f64) * (s as f64);
        }

        // Goertzel is used here instead of a new FFT dependency: it provides
        // real frequency analysis with fixed storage and no per-band vectors.
        let sr = self.sample_rate.max(1) as f32;
        let nyquist = (sr * 0.5).max(21.0);
        let min_freq = 20.0f32.min(nyquist * 0.5);
        for (band, magnitude) in bands.iter_mut().enumerate() {
            let normalized = band as f32 / (BAND_COUNT - 1) as f32;
            let frequency = min_freq * (nyquist / min_freq).powf(normalized);
            let omega = 2.0 * std::f32::consts::PI * frequency / sr;
            let coeff = 2.0 * omega.cos();
            let mut state1 = 0.0f32;
            let mut state2 = 0.0f32;
            for &sample in samples {
                let s = if sample.is_finite() { sample } else { 0.0 };
                let state0 = s + coeff * state1 - state2;
                state2 = state1;
                state1 = state0;
            }
            let power = (state1 * state1 + state2 * state2 - coeff * state1 * state2)
                .max(0.0);
            *magnitude = (power / samples.len() as f32).sqrt();
            total_energy += *magnitude;
            weighted_frequency += frequency * *magnitude;
        }

        AudioSpectrum {
            bands: bands.to_vec(),
            peak_amplitude: peak,
            rms_level: (sum_sq / samples.len() as f64).sqrt() as f32,
            spectral_centroid: if total_energy > 0.0 { weighted_frequency / total_energy } else { 0.0 },
            is_beat: peak > 0.8 && (bands[0] + bands[1]) > (total_energy * 0.1),
        }
    }

    pub fn apply_curing_eq(&self, samples: &mut [f32], target_profile: &[f32]) {
        if target_profile.is_empty() { return; }
        for (i, s) in samples.iter_mut().enumerate() {
            let gain = target_profile[i % target_profile.len()];
            let gain = if gain.is_finite() { gain.clamp(0.5, 2.0) } else { 1.0 };
            let input = if s.is_finite() { *s } else { 0.0 };
            *s = input * gain;
        }
    }

    pub fn apply_master_limiter(&self, samples: &mut [f32], threshold_db: f32) {
        let threshold_db = if threshold_db.is_finite() { threshold_db } else { -0.1 };
        let threshold = 10.0f32.powf(threshold_db / 20.0).clamp(1.0e-6, 1.0);
        for s in samples.iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
                continue;
            }
            let abs_s = s.abs();
            if abs_s > threshold {
                let gain_reduction = threshold / abs_s;
                *s *= gain_reduction;
            }
        }
    }

    /// 3D Surround Panner [Spatial Sovereignty]
    pub fn apply_3d_panner(&self, samples: &mut [f32], source_pos: crate::math::Vector3, listener_pos: crate::math::Vector3) {
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        let dx = finite(source_pos.x) - finite(listener_pos.x);
        let dy = finite(source_pos.y) - finite(listener_pos.y);
        let dz = finite(source_pos.z) - finite(listener_pos.z);
        let distance = (dx*dx + dy*dy + dz*dz).sqrt().max(1.0);
        
        let attenuation = 1.0 / distance;
        let pan = (dx / distance).clamp(-1.0, 1.0);

        for (i, s) in samples.iter_mut().enumerate() {
            let channel_gain = if i % 2 == 0 { (1.0 - pan) * 0.5 } else { (1.0 + pan) * 0.5 };
            let input = if s.is_finite() { *s } else { 0.0 };
            *s = input * attenuation * channel_gain;
        }
    }
}

// ---------------------------------------------------------------------------
// Advanced DSP Modules [Industrial Grade]
// ---------------------------------------------------------------------------

/// High-fidelity algorithmic reverb [Freeverb Architecture]
pub struct ReverbCore {
    pub mix_smoother: ParameterSmoother,
    pub room_size_smoother: ParameterSmoother,
    pub comb_filters: Vec<CombFilter>,
    pub allpass_filters: Vec<AllpassFilter>,
}

pub struct CombFilter {
    buffer: Vec<f32>,
    ptr: usize,
    feedback: f32,
    filter_state: f32,
    damp: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        Self { buffer: vec![0.0; size], ptr: 0, feedback: 0.8, filter_state: 0.0, damp: 0.2 }
    }
    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.ptr];
        self.filter_state = (output * (1.0 - self.damp)) + (self.filter_state * self.damp);
        self.buffer[self.ptr] = input + (self.filter_state * self.feedback);
        self.ptr = (self.ptr + 1) % self.buffer.len();
        output
    }
}

pub struct AllpassFilter {
    buffer: Vec<f32>,
    ptr: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self { buffer: vec![0.0; size], ptr: 0 }
    }
    fn process(&mut self, input: f32) -> f32 {
        let buf_out = self.buffer[self.ptr];
        let output = -input + buf_out;
        self.buffer[self.ptr] = input + (buf_out * 0.5);
        self.ptr = (self.ptr + 1) % self.buffer.len();
        output
    }
}

impl ReverbCore {
    pub fn new(sample_rate: u32) -> Self {
        let comb_sizes = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let allpass_sizes = [556, 441, 341, 225];
        
        Self {
            mix_smoother: ParameterSmoother::new(0.3, sample_rate as f32, 20.0),
            room_size_smoother: ParameterSmoother::new(0.5, sample_rate as f32, 50.0),
            comb_filters: comb_sizes.iter().map(|&s| CombFilter::new(s)).collect(),
            allpass_filters: allpass_sizes.iter().map(|&s| AllpassFilter::new(s)).collect(),
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        let mix = self.mix_smoother.next();
        let room = self.room_size_smoother.next();
        
        for comb in &mut self.comb_filters {
            comb.feedback = room * 0.28 + 0.7; // Map room size to feedback
        }

        for s in samples.iter_mut() {
            let input = *s;
            let mut out = 0.0;
            
            // Parallel Comb Filters
            for comb in &mut self.comb_filters {
                out += comb.process(input);
            }
            
            // Series Allpass Filters
            for allpass in &mut self.allpass_filters {
                out = allpass.process(out);
            }
            
            *s = input * (1.0 - mix) + out * 0.1 * mix;
        }
    }
}

/// Nodal Sampler Engine with Linear Interpolation.
pub struct SamplerEngine {
    pub sample_ptr: f32,
    pub is_looping: bool,
    pub playback_rate: f32,
    pub envelope_gain: f32,
}

impl SamplerEngine {
    pub fn new() -> Self {
        Self { 
            sample_ptr: 0.0, 
            is_looping: true,
            playback_rate: 1.0,
            envelope_gain: 1.0,
        }
    }

    pub fn render_to_buffer(&mut self, source: &[f32], output: &mut [f32]) {
        if source.is_empty() || output.is_empty() || !self.playback_rate.is_finite() || self.playback_rate <= 0.0 {
            return;
        }

        // Keep an invalid/negative position from reaching the usize index conversion.
        if !self.sample_ptr.is_finite() || self.sample_ptr < 0.0 {
            self.sample_ptr = 0.0;
        }

        let source_len = source.len() as f32;
        if self.is_looping {
            // Keep the phase bounded so repeated looping cannot exhaust f32's
            // integer precision.  Reducing the rate as well avoids an
            // overflowing addition for very large, but finite, rates.
            self.sample_ptr = self.sample_ptr.rem_euclid(source_len);
        }

        for s in output.iter_mut() {
            if !self.is_looping && self.sample_ptr >= source.len() as f32 {
                self.sample_ptr = 0.0;
                break;
            }

            let idx0 = self.sample_ptr as usize % source.len();
            let idx1 = if self.is_looping {
                (idx0 + 1) % source.len()
            } else {
                (idx0 + 1).min(source.len() - 1)
            };
            let frac = self.sample_ptr - self.sample_ptr.floor();
            
            // Linear Interpolation
            let val = source[idx0] * (1.0 - frac) + source[idx1] * frac;
            
            *s += val * 0.5 * self.envelope_gain;
            self.sample_ptr += if self.is_looping {
                self.playback_rate.rem_euclid(source_len)
            } else {
                self.playback_rate
            };

            if self.is_looping {
                self.sample_ptr = self.sample_ptr.rem_euclid(source_len);
            }
            
            if !self.is_looping && self.sample_ptr >= source.len() as f32 {
                self.sample_ptr = 0.0;
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Industrial Nodal Audio Orchestrator
// ---------------------------------------------------------------------------

pub struct AudioOrchestrator {
    pub engine: Arc<AudioEngine>,
    pub reverb: Mutex<ReverbCore>,
    pub sampler: Mutex<SamplerEngine>,
    pub synth: Mutex<crate::synth_engine::SovereignSynth>,
    pub drum_machine: Mutex<crate::drum_machine::DrumMachineEngine>,
    pub asset_library: Mutex<crate::asset_library::AssetLibraryEngine>,
}

impl AudioOrchestrator {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            engine: Arc::new(AudioEngine::new(sample_rate)),
            reverb: Mutex::new(ReverbCore::new(sample_rate)),
            sampler: Mutex::new(SamplerEngine::new()),
            synth: Mutex::new(crate::synth_engine::SovereignSynth::new(sample_rate)),
            drum_machine: Mutex::new(crate::drum_machine::DrumMachineEngine::new()),
            asset_library: Mutex::new(crate::asset_library::AssetLibraryEngine::new(std::path::PathBuf::new())),
        }
    }

    pub fn scan_asset_library(&self, path: std::path::PathBuf) -> Result<(), String> {
        let mut library = self.asset_library.lock();
        library.factory_path = path;
        library.scan_factory_library()
    }

    pub fn preload_audio_asset(&self, asset_id: u64) -> Result<(), String> {
        self.asset_library.lock().preload_audio(asset_id)
    }

    pub fn set_drum_pad_asset(&self, pad_idx: usize, asset_id: Option<u64>) -> bool {
        self.drum_machine.lock().set_pad_asset(pad_idx, asset_id)
    }

    pub fn process_graph(&self, state: &crate::WorkspaceState) -> AudioSpectrum {
        let has_audio_nodes = state.nodes.iter().any(|n| {
            matches!(n.node_kind.as_str(), "audio" | "synth" | "eq" | "sampler" | "reverb" | "drum_machine")
        });
        if !has_audio_nodes { return AudioSpectrum::default(); }

        // Reuse one fixed-size graph workspace instead of allocating a Vec for
        // every graph evaluation. This method is not the hardware callback, but
        // keeping its buffer stable prevents avoidable jitter when the UI polls it.
        let mut scratch = self.engine.scratch_buffer.lock();
        if scratch.len() != 1024 {
            scratch.resize(1024, 0.0);
        }
        scratch.fill(0.0);
        let master_buffer = &mut *scratch;
        
        for node in &state.nodes {
            if !matches!(node.node_kind.as_str(), "audio" | "synth" | "eq" | "sampler" | "reverb" | "drum_machine") {
                continue;
            }
            match node.node_kind.as_str() {
                "sampler" => {
                    let mut s = self.sampler.lock();
                    // A sampler without an explicitly loaded asset must remain
                    // silent; never allocate or synthesize a fake source on the
                    // audio path.
                    s.render_to_buffer(&[], master_buffer);
                    
                    // Apply Spatial Panning based on node position
                    let pos = crate::math::Vector3 { x: node.x / 1000.0, y: node.y / 1000.0, z: 0.0 };
                    self.engine.apply_3d_panner(master_buffer, pos, crate::math::Vector3 { x: 0.5, y: 0.5, z: 0.0 });
                }
                "reverb" => {
                    let mut r = self.reverb.lock();
                    r.process(master_buffer);
                }
                "synth" => {
                    let mut sy = self.synth.lock();
                    sy.render(master_buffer);
                }
                "drum_machine" => {
                    let library = self.asset_library.lock();
                    self.drum_machine.lock().render(&library, master_buffer);
                }
                "eq" => {
                    const UNITY_PROFILE: [f32; 1] = [1.0];
                    self.engine.apply_curing_eq(master_buffer, &UNITY_PROFILE);
                }
                _ => {}
            }
        }

        self.engine.apply_master_limiter(master_buffer, -0.1);
        let spectrum = self.engine.analyze_spectrum(master_buffer);
        let mut out = self.engine.output_buffer.lock();
        std::mem::swap(&mut *out, &mut *scratch);
        spectrum
    }
}
