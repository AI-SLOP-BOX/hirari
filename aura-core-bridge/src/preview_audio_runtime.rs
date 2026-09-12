use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

struct PreviewDecodeResult {
    generation: u64,
    samples: Result<(Vec<f32>, f64), String>,
}

static PREVIEW_GENERATION: AtomicU64 = AtomicU64::new(0);
static PREVIEW_RESULTS: OnceLock<Mutex<VecDeque<PreviewDecodeResult>>> = OnceLock::new();

fn preview_results() -> &'static Mutex<VecDeque<PreviewDecodeResult>> {
    PREVIEW_RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

#[derive(Clone, Debug)]
pub struct PreviewAudioAsset {
    pub id: u64,
    pub path: PathBuf,
    pub samples: Vec<f32>,
    pub sample_rate: f64,
    pub channels: u16,
    pub frame_count: u64,
    pub content_hash: String,
}

pub struct PreviewAudioRuntime {
    assets: HashMap<u64, PreviewAudioAsset>,
    pads: [Option<u64>; 16],
}

impl Default for PreviewAudioRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewAudioRuntime {
    pub fn new() -> Self {
        Self {
            assets: HashMap::new(),
            pads: [None; 16],
        }
    }

    /// Starts file probing/decoding on a worker thread. The engine itself is
    /// never moved across threads; only immutable decoded PCM crosses back
    /// through the bounded completion queue.
    pub fn queue_decode(path: PathBuf) -> u64 {
        let generation = PREVIEW_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
        std::thread::spawn(move || {
            let result = if !path.is_file() {
                Err("audio file not found".to_owned())
            } else {
                decode_wav(&path)
            };
            let mut queue = preview_results().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.push_back(PreviewDecodeResult { generation, samples: result });
            while queue.len() > 4 { queue.pop_front(); }
        });
        generation
    }

    /// Returns the newest completed decode and discards stale selections.
    pub fn take_completed_decode() -> Option<(u64, Result<(Vec<f32>, f64), String>)> {
        let mut queue = preview_results().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let newest = queue.pop_back()?;
        queue.clear();
        Some((newest.generation, newest.samples))
    }

    pub fn scan(&mut self, root: &Path) -> Result<usize, String> {
        if !root.is_dir() {
            // Keep filesystem details out of user-facing diagnostics. The
            // caller can still inspect the original path internally when
            // needed, but the UI must not expose the OS home directory.
            return Err("audio library is not a directory".to_owned());
        }
        self.assets.clear();
        // A scan replaces the asset namespace. Old pad assignments must not
        // survive and accidentally point at a different file after IDs are
        // regenerated.
        self.pads = [None; 16];
        let mut paths = Vec::new();
        collect_wavs(root, &mut paths)?;
        paths.sort();
        let mut used_ids = std::collections::HashSet::with_capacity(paths.len());
        for path in paths {
            // A recursive library scan is best-effort: one partial download or
            // mislabeled file must not poison the whole browser catalog. Direct
            // registration remains strict and reports the decode error.
            let Ok(metadata) = probe_wav(&path) else {
                continue;
            };
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            let content_hash = sha256_hex(&bytes);
            let id = stable_asset_id(content_hash.as_bytes(), &mut used_ids);
            self.assets.insert(
                id,
                PreviewAudioAsset {
                    id,
                    path,
                    samples: Vec::new(),
                    sample_rate: metadata.0,
                    channels: metadata.1,
                    frame_count: metadata.2,
                    content_hash,
                },
            );
        }
        Ok(self.assets.len())
    }

    pub fn preload(&mut self, id: u64) -> Result<(), String> {
        let asset = self
            .assets
            .get_mut(&id)
            .ok_or_else(|| "audio asset not found".to_string())?;
        let (samples, sample_rate) = decode_wav(&asset.path)?;
        asset.samples = samples;
        asset.sample_rate = sample_rate;
        asset.channels = asset.channels.max(1);
        asset.frame_count = asset.samples.len() as u64;
        if asset.content_hash.is_empty() {
            asset.content_hash = sha256_hex(&fs::read(&asset.path).map_err(|e| e.to_string())?);
        }
        Ok(())
    }

    pub fn register_file(&mut self, path: &Path) -> Result<u64, String> {
        if !path.is_file() {
            return Err("audio file not found".into());
        }
        if let Some(existing) = self
            .assets
            .values()
            .find(|asset| asset.path == path)
            .map(|asset| asset.id)
        {
            return Ok(existing);
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let content_hash = sha256_hex(&bytes);
        let mut used_ids = self.assets.keys().copied().collect();
        let (metadata_sample_rate, metadata_channels, metadata_frames) = probe_wav(path)?;
        let id = stable_asset_id(content_hash.as_bytes(), &mut used_ids);
        let mut asset = PreviewAudioAsset {
            id,
            path: path.to_path_buf(),
            samples: Vec::new(),
            sample_rate: metadata_sample_rate,
            channels: metadata_channels,
            frame_count: metadata_frames,
            content_hash,
        };
        let (samples, sample_rate) = decode_wav(path)?;
        asset.samples = samples;
        asset.sample_rate = sample_rate;
        asset.frame_count = asset.samples.len() as u64;
        self.assets.insert(id, asset);
        Ok(id)
    }

    pub fn assign_pad(&mut self, pad: usize, id: Option<u64>) -> bool {
        if pad >= self.pads.len() || id.is_some_and(|value| !self.assets.contains_key(&value)) {
            return false;
        }
        self.pads[pad] = id;
        true
    }

    pub fn pad_asset(&self, pad: usize) -> Option<u64> {
        self.pads.get(pad).copied().flatten()
    }

    pub fn pad_samples(&self, pad: usize) -> Option<Vec<f32>> {
        let id = self.pad_asset(pad)?;
        self.asset_samples(id)
    }

    pub fn asset_samples(&self, id: u64) -> Option<Vec<f32>> {
        self.asset_audio(id).map(|(samples, _)| samples)
    }

    pub fn pad_audio(&self, pad: usize) -> Option<(Vec<f32>, f64)> {
        let id = self.pad_asset(pad)?;
        self.asset_audio(id)
    }

    pub fn asset_audio(&self, id: u64) -> Option<(Vec<f32>, f64)> {
        let asset = self.assets.get(&id)?;
        if asset.samples.is_empty() {
            return None;
        }
        Some((asset.samples.clone(), asset.sample_rate))
    }

    /// Returns a preview rendered at the project tempo. The common source and
    /// project BPM values define one shared resampling ratio, so loop previews
    /// follow tempo without mutating the catalog's original audio.
    pub fn tempo_synced_audio(&self, id: u64, source_bpm: f64, project_bpm: f64) -> Option<Vec<f32>> {
        if !source_bpm.is_finite() || !project_bpm.is_finite() || source_bpm <= 0.0 || project_bpm <= 0.0 { return None; }
        let asset = self.assets.get(&id)?;
        if asset.samples.is_empty() { return None; }
        let ratio = (project_bpm / source_bpm).clamp(0.03125, 32.0);
        let output_len = ((asset.samples.len() as f64) / ratio).round();
        if !output_len.is_finite() || !(1.0..=16_000_000.0).contains(&output_len) { return None; }
        let output_len = output_len as usize;
        let mut output = vec![0.0; output_len];
        for (index, sample) in output.iter_mut().enumerate() {
            let position = index as f64 * ratio;
            let base = position.floor() as usize;
            if base + 1 >= asset.samples.len() { *sample = *asset.samples.last()?; continue; }
            let fraction = (position - base as f64) as f32;
            let a = asset.samples[base]; let b = asset.samples[base + 1];
            *sample = if a.is_finite() && b.is_finite() { a + (b - a) * fraction } else { 0.0 };
        }
        Some(output)
    }
    pub fn asset_count(&self) -> usize {
        self.assets.len()
    }

    /// Returns the lightweight browser catalog without copying decoded audio.
    /// The UI and the preview engine therefore observe the same registered
    /// asset namespace while large sample buffers remain off the control API.
    pub fn catalog(&self) -> Vec<PreviewAudioCatalogEntry> {
        let mut entries: Vec<_> = self
            .assets
            .values()
            .map(|asset| {
                let size = fs::metadata(&asset.path).map(|meta| meta.len()).unwrap_or(0);
                PreviewAudioCatalogEntry {
                    id: asset.id,
                    name: asset
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Audio")
                        .to_owned(),
                    path: asset.path.to_string_lossy().into_owned(),
                    size,
                    sample_rate: asset.sample_rate,
                    frames: if asset.frame_count != 0 {
                        asset.frame_count
                    } else {
                        asset.samples.len() as u64
                    },
                    loaded: !asset.samples.is_empty(),
                    content_hash: asset.content_hash.clone(),
                }
            })
            .collect();
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        entries
    }
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct PreviewAudioCatalogEntry {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub sample_rate: f64,
    pub frames: u64,
    pub loaded: bool,
    pub content_hash: String,
}

fn collect_wavs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_wavs(&path, out)?;
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "wav" | "wave"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn stable_asset_id(identity: &[u8], used: &mut std::collections::HashSet<u64>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut candidate = hash.max(1);
    while !used.insert(candidate) {
        candidate = candidate.wrapping_add(1).max(1);
    }
    candidate
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn probe_wav(path: &Path) -> Result<(f64, u16, u64), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return Err("unsupported WAV".into());
    }
    let mut pos = 12usize;
    let mut sample_rate = 0.0;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data_bytes = 0u64;
    let rf64 = &bytes[0..4] == b"RF64";
    let mut rf64_data_size = None;
    while pos.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[pos..pos + 4];
        let encoded_size =
            u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV chunk size")?);
        pos += 8;
        if id == b"ds64" && encoded_size >= 28 {
            rf64_data_size = Some(u64::from_le_bytes(read_array::<8>(
                &bytes,
                pos + 8,
                "RF64 data size",
            )?));
        }
        let size_u64 = if rf64 && id == b"data" && encoded_size == u32::MAX {
            rf64_data_size.ok_or("RF64 data size is missing")?
        } else {
            u64::from(encoded_size)
        };
        let size = usize::try_from(size_u64).map_err(|_| "WAV chunk is too large")?;
        let end = pos.checked_add(size).ok_or("WAV chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated WAV".into());
        }
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes(read_array::<2>(&bytes, pos + 2, "WAV channels")?);
            sample_rate = u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV sample rate")?) as f64;
            bits = u16::from_le_bytes(read_array::<2>(&bytes, pos + 14, "WAV bits")?);
        } else if id == b"data" {
            data_bytes = size_u64;
        }
        pos = end.checked_add(size & 1).ok_or("WAV alignment overflow")?;
    }
    let bytes_per_frame = u64::from(channels).saturating_mul(u64::from(bits / 8));
    if channels == 0 || bits == 0 || bytes_per_frame == 0 || data_bytes == 0 {
        return Err("WAV metadata is incomplete".into());
    }
    Ok((sample_rate, channels, data_bytes / bytes_per_frame))
}

fn decode_wav(path: &Path) -> Result<(Vec<f32>, f64), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return Err("unsupported WAV".into());
    }
    let rf64 = &bytes[0..4] == b"RF64";
    let riff_size = u32::from_le_bytes(read_array::<4>(&bytes, 4, "RIFF size")?) as usize;
    if !rf64 && (riff_size < 4 || riff_size.checked_add(8).is_none_or(|end| end > bytes.len())) {
        return Err("invalid RIFF size".into());
    }
    let mut pos = 12;
    let mut audio_format = 1u16;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut sample_rate = 44100.0f64;
    let mut data = None;
    let mut fmt_found = false;
    let mut rf64_data_size: Option<u64> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let encoded_size = u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV chunk size")?);
        pos += 8;
        if id == b"ds64" && encoded_size >= 28 {
            let ds64_end = pos.checked_add(28).ok_or("RF64 ds64 overflow")?;
            if ds64_end > bytes.len() {
                return Err("truncated RF64 ds64 chunk".into());
            }
            rf64_data_size = Some(u64::from_le_bytes(read_array::<8>(
                &bytes,
                pos + 8,
                "RF64 data size",
            )?));
        }
        let size = if rf64 && id == b"data" && encoded_size == u32::MAX {
            usize::try_from(rf64_data_size.ok_or("RF64 data size is missing")?)
                .map_err(|_| "RF64 data size is too large")?
        } else {
            encoded_size as usize
        };
        let end = pos.checked_add(size).ok_or("WAV chunk overflow")?;
        if end > bytes.len() {
            return Err("truncated WAV".into());
        }
        if id == b"fmt " && size >= 16 {
            fmt_found = true;
            audio_format = u16::from_le_bytes(read_array::<2>(&bytes, pos, "WAV format")?);
            channels = u16::from_le_bytes(read_array::<2>(&bytes, pos + 2, "WAV channel count")?);
            sample_rate =
                u32::from_le_bytes(read_array::<4>(&bytes, pos + 4, "WAV sample rate")?) as f64;
            bits = u16::from_le_bytes(read_array::<2>(&bytes, pos + 14, "WAV bit depth")?);
        }
        if id == b"data" {
            data = Some((pos, end));
        }
        let next = end
            .checked_add(size & 1)
            .ok_or("WAV chunk alignment overflow")?;
        if next > bytes.len() {
            return Err("truncated WAV chunk padding".into());
        }
        pos = next;
    }
    let (start, end) = data.ok_or("WAV data missing")?;
    if start == end {
        return Err("WAV data is empty".into());
    }
    if !fmt_found
        || channels == 0
        || channels > 32
        || !matches!(bits, 16 | 24 | 32)
        || !matches!(audio_format, 1 | 3)
        || (audio_format == 3 && bits != 32)
    {
        return Err("only PCM16/24/32 or float32 WAV is supported".into());
    }
    let bytes_per = (bits / 8) as usize;
    let frame_bytes = bytes_per * channels as usize;
    if frame_bytes == 0 {
        return Err("invalid WAV channels".into());
    }
    if (end - start) % frame_bytes != 0 {
        return Err("WAV data is not aligned to complete frames".into());
    }
    let mut result = Vec::with_capacity((end - start) / frame_bytes);
    for frame in (start..end).step_by(frame_bytes) {
        let mut sum = 0.0;
        for ch in 0..channels as usize {
            let p = frame + ch * bytes_per;
            let v = if bits == 16 {
                i16::from_le_bytes(read_array::<2>(&bytes, p, "PCM16 sample")?) as f32 / 32768.0
            } else if bits == 24 {
                let raw = (bytes[p] as i32)
                    | ((bytes[p + 1] as i32) << 8)
                    | ((bytes[p + 2] as i32) << 16);
                let signed = if raw & 0x0080_0000 != 0 {
                    raw | !0x00FF_FFFF
                } else {
                    raw
                };
                signed as f32 / 8_388_608.0
            } else if audio_format == 3 {
                f32::from_le_bytes(read_array::<4>(&bytes, p, "float32 sample")?)
            } else {
                i32::from_le_bytes(read_array::<4>(&bytes, p, "PCM32 sample")?) as f32
                    / 2147483648.0
            };
            if !v.is_finite() {
                return Err("WAV contains a non-finite sample".into());
            }
            sum += v.clamp(-1.0, 1.0);
        }
        result.push(sum / channels as f32);
    }
    if !sample_rate.is_finite() || !(1.0..=384_000.0).contains(&sample_rate) {
        return Err("invalid WAV sample rate".into());
    }
    Ok((result, sample_rate))
}

fn read_array<const N: usize>(bytes: &[u8], start: usize, what: &str) -> Result<[u8; N], String> {
    let end = start
        .checked_add(N)
        .ok_or_else(|| format!("{} offset overflow", what))?;
    let slice = bytes
        .get(start..end)
        .ok_or_else(|| format!("truncated {}", what))?;
    slice
        .try_into()
        .map_err(|_| format!("invalid {} length", what))
}

#[cfg(test)]
#[path = "preview_audio_runtime_tests.rs"]
mod preview_audio_runtime_tests;
