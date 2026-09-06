use crate::forensics::{ForensicModule, ForensicSeverity};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum AssetType {
    AudioSample,
    VideoClip,
    PluginPatch, // Logic Pro style .patch
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
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub sample_rate: u32,
    #[serde(default)]
    pub channels: u16,
    #[serde(default)]
    pub frame_count: u64,
}

/// Industrial Asset Library Engine [Resource Sovereignty]
/// Manages the factory and user libraries, indexing assets for high-speed retrieval.
pub struct AssetLibraryEngine {
    pub registry: HashMap<u64, AssetMetadata>,
    pub factory_path: PathBuf,
    decoded_audio: HashMap<u64, Arc<Vec<f32>>>,
}

impl AssetLibraryEngine {
    pub fn new(factory_path: PathBuf) -> Self {
        Self {
            registry: HashMap::new(),
            factory_path,
            decoded_audio: HashMap::new(),
        }
    }

    /// Scans the factory directory and manifests industrial assets.
    pub fn scan_factory_library(&mut self) -> Result<(), String> {
        crate::aura_log!(
            ForensicSeverity::Info,
            ForensicModule::Io,
            "ASSET: Scanning sovereign library at {:?}",
            self.factory_path
        );

        let root = fs::symlink_metadata(&self.factory_path).map_err(|error| {
            format!(
                "unable to access factory library {:?}: {error}",
                self.factory_path
            )
        })?;
        if !root.is_dir() {
            return Err(format!(
                "factory library is not a directory: {:?}",
                self.factory_path
            ));
        }

        let mut files = Vec::new();
        collect_assets(&self.factory_path, &mut files).map_err(|error| {
            format!(
                "unable to scan factory library {:?}: {error}",
                self.factory_path
            )
        })?;
        files.sort();

        self.registry.clear();
        self.decoded_audio.clear();
        let mut ids = HashSet::new();
        for path in files {
            let Some(asset_type) = asset_type_for(&path) else {
                continue;
            };
            let bytes = fs::read(&path).unwrap_or_default();
            let content_hash = stable_content_hash(&bytes);
            let id = stable_asset_id(&content_hash, &mut ids);
            let (sample_rate, channels, frame_count) =
                if matches!(&asset_type, AssetType::AudioSample) {
                    probe_wav_metadata(&bytes).unwrap_or((0, 0, 0))
                } else {
                    (0, 0, 0)
                };
            self.registry.insert(
                id,
                AssetMetadata {
                    id,
                    name: path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    asset_type,
                    path,
                    tags: Vec::new(),
                    author: String::new(),
                    content_hash,
                    sample_rate,
                    channels,
                    frame_count,
                },
            );
        }

        Ok(())
    }

    /// Decodes a PCM WAV asset off the audio thread and caches mono samples.
    pub fn preload_audio(&mut self, id: u64) -> Result<(), String> {
        let meta = self
            .registry
            .get(&id)
            .ok_or_else(|| "asset not found".to_string())?;
        if !matches!(meta.asset_type, AssetType::AudioSample) {
            return Err("asset is not audio".into());
        }
        let bytes = fs::read(&meta.path).map_err(|e| e.to_string())?;
        let samples = decode_pcm_wav(&bytes)?;
        self.decoded_audio.insert(id, Arc::new(samples));
        Ok(())
    }

    pub fn audio_samples(&self, id: u64) -> Option<Arc<Vec<f32>>> {
        self.decoded_audio.get(&id).cloned()
    }

    /// Performs a high-speed search across the sovereign registry.
    pub fn search(&self, query: &str) -> Vec<AssetMetadata> {
        let query = query.to_lowercase();
        self.registry
            .values()
            .filter(|a| {
                a.name.to_lowercase().contains(&query) || a.tags.iter().any(|t| t.contains(&query))
            })
            .cloned()
            .collect()
    }
}

fn collect_assets(directory: &Path, assets: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let _ = collect_assets(&path, assets);
        } else if metadata.is_file() && asset_type_for(&path).is_some() {
            assets.push(path);
        }
    }
    Ok(())
}

fn asset_type_for(path: &Path) -> Option<AssetType> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "wav" | "wave" | "aif" | "aiff" | "flac" | "mp3" | "ogg" | "oga" | "m4a" | "aac" => {
            Some(AssetType::AudioSample)
        }
        "patch" | "aura" | "aupreset" | "fxp" => Some(AssetType::PluginPatch),
        _ => None,
    }
}

fn stable_content_hash(bytes: &[u8]) -> String {
    // FNV is retained only as a compact UI/cache key. The full content hash
    // is represented as two independent 64-bit lanes so the asset identity
    // changes when a file is replaced at the same path.
    let mut a = 0xcbf29ce484222325u64;
    let mut b = 0x84222325cbf29ceu64;
    for &byte in bytes {
        a = (a ^ byte as u64).wrapping_mul(0x100000001b3);
        b = (b ^ byte.reverse_bits() as u64)
            .rotate_left(5)
            .wrapping_mul(0x9e3779b185ebca87);
    }
    format!("{a:016x}{b:016x}")
}

fn stable_asset_id(hash: &str, used: &mut HashSet<u64>) -> u64 {
    let mut id = 0xcbf29ce484222325u64;
    for byte in hash.as_bytes() {
        id = (id ^ *byte as u64).wrapping_mul(0x100000001b3);
    }
    if id == 0 {
        id = 1;
    }
    let start = id;
    while !used.insert(id) {
        id = id.wrapping_add(1);
        if id == start {
            return 0;
        }
    }
    id
}

fn probe_wav_metadata(bytes: &[u8]) -> Option<(u32, u16, u64)> {
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut format = None;
    let mut data_bytes = None;
    while pos.checked_add(8)? <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        pos += 8;
        let end = pos.checked_add(len)?;
        if end > bytes.len() {
            return None;
        }
        if id == b"fmt " && len >= 16 {
            let channels = u16::from_le_bytes(bytes[pos + 2..pos + 4].try_into().ok()?);
            let rate = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?);
            let block_align = u16::from_le_bytes(bytes[pos + 12..pos + 14].try_into().ok()?);
            if channels == 0 || rate == 0 || block_align == 0 {
                return None;
            }
            format = Some((rate, channels, block_align));
        } else if id == b"data" {
            data_bytes = Some(len as u64);
        }
        pos = end.checked_add(len & 1)?;
    }
    let (rate, channels, block_align) = format?;
    let data = data_bytes?;
    Some((rate, channels, data / block_align as u64))
}

fn decode_pcm_wav(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return Err("invalid WAV header".into());
    }
    let mut pos = 12usize;
    let mut format = None;
    let mut data = None;
    let mut rf64_data_size = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let declared_len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap());
        pos += 8;
        let len = if id == b"data" && declared_len == u32::MAX {
            usize::try_from(rf64_data_size.ok_or("RF64 data size is missing")?)
                .map_err(|_| "RF64 data size is too large")?
        } else {
            declared_len as usize
        };
        let end = pos.checked_add(len).ok_or("WAV chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated WAV chunk".into());
        }
        if id == b"ds64" && len >= 16 {
            rf64_data_size = Some(u64::from_le_bytes(
                bytes[pos + 8..pos + 16]
                    .try_into()
                    .map_err(|_| "invalid RF64 ds64 chunk")?,
            ));
        } else if id == b"fmt " && len >= 16 {
            let audio_format = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap());
            let channels = u16::from_le_bytes(bytes[pos + 2..pos + 4].try_into().unwrap());
            let bits = u16::from_le_bytes(bytes[pos + 14..pos + 16].try_into().unwrap());
            format = Some((audio_format, channels, bits));
        } else if id == b"data" {
            data = Some((pos, end));
        }
        pos = end
            .checked_add(len & 1)
            .ok_or("WAV chunk alignment overflow")?;
        if pos > bytes.len() {
            return Err("truncated WAV chunk padding".into());
        }
    }
    let (kind, channels, bits) = format.ok_or("WAV fmt chunk missing")?;
    let (start, end) = data.ok_or("WAV data chunk missing")?;
    if channels == 0 || !matches!(bits, 16 | 24 | 32) || !matches!(kind, 1 | 3) {
        return Err("unsupported WAV format".into());
    }
    let bytes_per_sample = (bits / 8) as usize;
    let frame_bytes = bytes_per_sample * channels as usize;
    if frame_bytes == 0 || (end - start) % frame_bytes != 0 {
        return Err("invalid WAV frame alignment".into());
    }
    let frames = (end - start) / frame_bytes;
    let mut out = Vec::with_capacity(frames);
    for frame in 0..frames {
        let base = start + frame * frame_bytes;
        let mut sum = 0.0f32;
        for ch in 0..channels as usize {
            let p = base + ch * bytes_per_sample;
            let sample = match (kind, bits) {
                (1, 16) => i16::from_le_bytes(bytes[p..p + 2].try_into().unwrap()) as f32 / 32768.0,
                (1, 24) => {
                    let v = i32::from_le_bytes([
                        bytes[p],
                        bytes[p + 1],
                        bytes[p + 2],
                        if bytes[p + 2] & 0x80 != 0 { 0xff } else { 0 },
                    ]);
                    (v as f32) / 8388608.0
                }
                (1, 32) => {
                    i32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as f32 / 2147483648.0
                }
                (3, 32) => f32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()),
                _ => unreachable!(),
            };
            sum += if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        out.push(sum / channels as f32);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::decode_pcm_wav;

    #[test]
    fn decodes_rf64_pcm16_using_ds64_data_size() {
        let mut wav = vec![0u8; 76];
        wav[0..4].copy_from_slice(b"RF64");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[12..16].copy_from_slice(b"ds64");
        wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        wav[28..36].copy_from_slice(&8u64.to_le_bytes());
        wav[36..40].copy_from_slice(b"fmt ");
        wav[40..44].copy_from_slice(&16u32.to_le_bytes());
        wav[44..46].copy_from_slice(&1u16.to_le_bytes());
        wav[46..48].copy_from_slice(&2u16.to_le_bytes());
        wav[56..58].copy_from_slice(&4u16.to_le_bytes());
        wav[58..60].copy_from_slice(&16u16.to_le_bytes());
        wav[60..64].copy_from_slice(b"data");
        wav[64..68].copy_from_slice(&u32::MAX.to_le_bytes());
        wav[68..76].copy_from_slice(&[0, 0x40, 0, 0, 0, 0, 0, 0]);

        let samples = decode_pcm_wav(&wav).expect("RF64 asset must decode");
        assert_eq!(samples.len(), 1);
        assert!((samples[0] - 0.25).abs() < 1.0e-5);
    }
}
