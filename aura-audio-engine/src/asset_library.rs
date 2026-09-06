use std::collections::HashMap;
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum AssetType {
    AudioSample,
    VideoClip,
    PluginPatch,
    ImpulseResponse,
    NeuralWeights,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct AssetMetadata {
    pub id: u64,
    pub name: String,
    pub asset_type: AssetType,
    pub path: PathBuf,
    pub tags: Vec<String>,
    pub author: String,
}

pub struct AssetLibraryEngine {
    pub registry: HashMap<u64, AssetMetadata>,
    pub factory_path: PathBuf,
}

impl AssetLibraryEngine {
    pub fn new(factory_path: PathBuf) -> Self {
        Self { registry: HashMap::new(), factory_path }
    }
    pub fn scan_factory_library(&mut self) -> Result<(), String> {
        let bootstrap = vec![
            (1, "Industrial Kick 01", AssetType::AudioSample, "samples/drums/kick_01.wav", vec!["drum", "kick"]),
            (2, "Celestial Pad", AssetType::PluginPatch, "patches/synths/celestial.aura", vec!["synth", "pad"]),
        ];
        for (id, name, kind, rel_path, tags) in bootstrap {
            self.registry.insert(id, AssetMetadata {
                id, name: name.to_string(), asset_type: kind,
                path: self.factory_path.join(rel_path),
                tags: tags.into_iter().map(|t| t.to_string()).collect(),
                author: "Aura Industrial".into(),
            });
        }
        Ok(())
    }
}
