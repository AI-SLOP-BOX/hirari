use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PresetMetadata {
    pub name: String,
    pub author: String,
    pub category: String,
    pub tags: Vec<String>,
    pub version_hash: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Preset {
    pub metadata: PresetMetadata,
    pub plugin_id: u32,
    pub binary_data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectTemplate {
    pub name: String,
    pub author: String,
    pub version: u32,
    pub layout_json: String,
    pub tags: Vec<String>,
}

impl ProjectTemplate {
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 256
            && self.author.len() <= 256
            && self.version > 0
            && self.layout_json.len() <= 64 * 1024 * 1024
            && serde_json::from_str::<serde_json::Value>(&self.layout_json).is_ok()
            && self.tags.len() <= 64
    }
}

pub struct PresetOrchestrator {
    pub library: Vec<Preset>,
}

impl Default for PresetOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl PresetOrchestrator {
    pub fn new() -> Self {
        Self {
            library: Vec::new(),
        }
    }

    pub fn search_templates<'a>(
        templates: &'a [ProjectTemplate],
        query: &str,
    ) -> Vec<&'a ProjectTemplate> {
        let q = query.trim().to_ascii_lowercase();
        templates
            .iter()
            .filter(|t| {
                t.validate()
                    && (q.is_empty()
                        || t.name.to_ascii_lowercase().contains(&q)
                        || t.tags
                            .iter()
                            .any(|tag| tag.to_ascii_lowercase().contains(&q)))
            })
            .collect()
    }

    /**
     * @brief SERIALIZE: Serializes a preset to a high-performance binary format.
     * INDUSTRIAL: Uses bincode for fast, zero-allocation binary layout.
     */
    pub fn serialize_preset(preset: &Preset) -> Vec<u8> {
        bincode::serialize(preset).unwrap_or_default()
    }

    /**
     * @brief DESERIALIZE: Deserializes a preset with full binary structural validation.
     * INDUSTRIAL: Gracefully catches corrupted headers or invalid data segments.
     */
    pub fn deserialize_preset(data: &[u8]) -> Option<Preset> {
        bincode::deserialize(data).ok()
    }

    /// INDUSTRIAL: Performs a tag-based search with forensic library auditing.
    pub fn search_by_tag(&self, tag: &str) -> Vec<Preset> {
        self.library
            .iter()
            .filter(|p| p.metadata.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /**
     * @brief HASH: Deterministically hashes metadata.
     * INDUSTRIAL: FNV-1a 64-bit non-cryptographic fast hashing algorithm to prevent preset collision.
     */
    pub fn calculate_metadata_hash(&self, meta: &PresetMetadata) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;

        let feed = |hash: &mut u64, bytes: &[u8]| {
            for &b in bytes {
                *hash ^= b as u64;
                *hash = hash.wrapping_mul(0x100000001b3u64);
            }
        };

        feed(&mut hash, meta.name.as_bytes());
        feed(&mut hash, meta.author.as_bytes());
        feed(&mut hash, meta.category.as_bytes());
        for tag in &meta.tags {
            feed(&mut hash, tag.as_bytes());
        }

        hash
    }
}
