use crate::audio_engine::SamplerEngine;
use parking_lot::Mutex;

pub struct DrumPad {
    pub sampler: SamplerEngine,
    pub asset_id: Option<u64>,
    pub volume: f32,
    pub pitch: f32,
}

impl DrumPad {
    pub fn new() -> Self {
        Self {
            sampler: SamplerEngine::new(),
            asset_id: None,
            volume: 0.8,
            pitch: 1.0,
        }
    }
}

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
    pub fn trigger_pad(&self, pad_idx: usize, velocity: u8) {
        if let Some(pad_mutex) = self.pads.get(pad_idx) {
            let mut pad = pad_mutex.lock();
            pad.sampler.sample_ptr = 0.0;
            pad.sampler.envelope_gain = velocity as f32 / 127.0;
        }
    }
    pub fn render(&self, _asset_registry: &crate::asset_library::AssetLibraryEngine, output: &mut [f32]) {
        for _pad_mutex in &self.pads {
            let mut pad = _pad_mutex.lock();
            if let Some(_asset_id) = pad.asset_id {
                let dummy_source = vec![0.5; 1024];
                pad.sampler.render_to_buffer(&dummy_source, output);
            }
        }
    }
}
