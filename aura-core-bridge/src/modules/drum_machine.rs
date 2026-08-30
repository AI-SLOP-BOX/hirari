use crate::audio_engine::SamplerEngine;
use crate::forensics::{ForensicSeverity, ForensicModule};
use parking_lot::Mutex;

/// Industrial Drum Machine Pad [Nodal Instrument]
pub struct DrumPad {
    pub sampler: SamplerEngine,
    pub asset_id: Option<u64>,
    pub volume: f32,
    pub pitch: f32,
}

impl DrumPad {
    pub fn new() -> Self {
        let mut sampler = SamplerEngine::new();
        // Drum pads are one-shot instruments; looping is opt-in for sustained samples.
        sampler.is_looping = false;
        Self {
            sampler,
            asset_id: None,
            volume: 0.8,
            pitch: 1.0,
        }
    }
}

/// Sovereign Drum Machine Designer [Grid-based Sampling]
/// Manages a 16-pad grid, each acting as an independent nodal sampler.
pub struct DrumMachineEngine {
    pub pads: Vec<Mutex<DrumPad>>,
}

impl DrumMachineEngine {
    pub fn new() -> Self {
        let mut pads = Vec::with_capacity(16);
        for _ in 0..16 {
            pads.push(Mutex::new(DrumPad::new()));
        }
        Self { pads }
    }

    pub fn set_pad_asset(&self, pad_idx: usize, asset_id: Option<u64>) -> bool {
        let Some(pad) = self.pads.get(pad_idx) else { return false };
        pad.lock().asset_id = asset_id;
        true
    }

    /// Triggers a specific pad (0-15) with a given velocity.
    pub fn trigger_pad(&self, pad_idx: usize, velocity: u8) {
        if let Some(pad_mutex) = self.pads.get(pad_idx) {
            let mut pad = pad_mutex.lock();
            pad.sampler.sample_ptr = 0.0; // Reset playback
            pad.sampler.envelope_gain = velocity as f32 / 127.0;
            
            crate::aura_log!(
                ForensicSeverity::Info,
                ForensicModule::Audio,
                "DMD: Triggered Pad {} (Velocity: {})",
                pad_idx, velocity
            );
        }
    }

    /// Renders the combined output of all pads into the master buffer.
    pub fn render(&self, asset_registry: &crate::asset_library::AssetLibraryEngine, output: &mut [f32]) {
        for pad_mutex in &self.pads {
            let mut pad = pad_mutex.lock();
            if let Some(asset_id) = pad.asset_id {
                if let Some(source) = asset_registry.audio_samples(asset_id) {
                    let old_gain = pad.sampler.envelope_gain;
                    pad.sampler.envelope_gain *= pad.volume;
                    pad.sampler.render_to_buffer(source.as_slice(), output);
                    pad.sampler.envelope_gain = old_gain;
                }
            }
        }
    }
}
