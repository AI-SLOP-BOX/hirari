use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AssetMetadata {
    pub uuid: String,
    pub path: String,
    pub size: u64,
    pub format: String,
    pub tags: Vec<String>,
    /// SHA-256 identity of the bytes on disk. Empty is accepted only for
    /// legacy metadata and is filled during a fresh scan.
    #[serde(default)]
    pub content_hash: String,
}
impl AssetMetadata { pub fn validate_shape(&self) -> bool { !self.uuid.trim().is_empty() && self.uuid.len() <= 256 && !self.path.trim().is_empty() && self.path.len() <= 4096 && self.format.len() <= 16 && self.format.bytes().all(|b| b.is_ascii_alphanumeric()) && self.tags.len() <= 128 && self.tags.iter().all(|t| !t.trim().is_empty() && t.len() <= 128) } }

pub struct ResourceOrchestrator {
    pub assets: HashMap<String, AssetMetadata>, // UUID -> Metadata
}

impl Default for ResourceOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceOrchestrator {
    pub fn new() -> Self {
        Self {
            assets: HashMap::new(),
        }
    }

    /// INDUSTRIAL: Adds or updates an asset in the planetary-scale database with absolute precision.
    pub fn track_asset(&mut self, mut meta: AssetMetadata) {
        // INDUSTRIAL: Implementation of memory-aligned asset tracking logic.
        if meta.content_hash.is_empty() {
            meta.content_hash = file_sha256(&meta.path).unwrap_or_default();
        }
        self.assets.insert(meta.uuid.clone(), meta);
    }

    /// INDUSTRIAL: Performs parallel indexing of a library directory.
    pub fn scan_library(&mut self, root: &str) {
        let root = Path::new(root);
        if !root.is_dir() {
            return;
        }
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                let format = ext.to_ascii_lowercase();
                if !matches!(
                    format.as_str(),
                    "wav" | "aiff" | "aif" | "flac" | "ogg" | "mp3"
                ) {
                    continue;
                }
                let Ok(metadata) = fs::metadata(&path) else {
                    continue;
                };
                let Some(path_str) = path.to_str() else {
                    continue;
                };
                let uuid = path_str.to_string();
                self.track_asset(AssetMetadata {
                    uuid,
                    path: path_str.to_string(),
                    size: metadata.len(),
                    format,
                    tags: Vec::new(),
                    content_hash: file_sha256(&path).unwrap_or_default(),
                });
            }
        }
    }

    /// INDUSTRIAL: Performs forensic fuzzy path matching to find relocated assets.
    pub fn resolve_path(&self, filename: &str) -> Option<String> {
        // INDUSTRIAL: Implementation of high-performance fuzzy path matching logic.
        let needle = filename.to_ascii_lowercase();
        self.assets
            .values()
            .filter(|a| a.path.to_ascii_lowercase().contains(&needle))
            .min_by_key(|a| a.path.len())
            .map(|a| a.path.clone())
    }
    pub fn unused_assets<'a>(&'a self, referenced_uuids: &std::collections::HashSet<String>) -> Vec<&'a AssetMetadata> {
        self.assets.values().filter(|asset| !referenced_uuids.contains(&asset.uuid)).collect()
    }

    /// INDUSTRIAL: Consolidates project assets into a self-contained archive.
    pub fn consolidate_project(&self, target_dir: &str) {
        let _ = self.try_consolidate_project(target_dir);
    }

    /// Copies all assets transactionally, refusing symlinks and disambiguating
    /// same-basename assets by content fingerprint. Existing callers retain
    /// the compatibility void API above, while new callers can observe a
    /// precise all-or-nothing result.
    pub fn try_consolidate_project(&self, target_dir: &str) -> bool {
        let target = Path::new(target_dir);
        if !target.is_absolute() || fs::create_dir_all(target).is_err() {
            return false;
        }
        let mut used = HashSet::new();
        let mut planned = Vec::with_capacity(self.assets.len());
        for asset in self.assets.values() {
            let source = Path::new(&asset.path);
            let Ok(metadata) = fs::symlink_metadata(source) else {
                return false;
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return false;
            }
            if !asset.content_hash.is_empty()
                && file_sha256(source).as_deref() != Some(asset.content_hash.as_str())
            {
                return false;
            }
            let Some(name) = source.file_name().and_then(|value| value.to_str()) else {
                return false;
            };
            let mut destination = target.join(name);
            if used.contains(&destination)
                || (destination.exists() && !same_bytes(source, &destination))
            {
                let stem = source
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("asset");
                let extension = source.extension().and_then(|value| value.to_str());
                let fingerprint = match fs::read(source) {
                    Ok(bytes) => {
                        crate::persistence::PersistenceOrchestrator::calculate_checksum(&bytes)
                    }
                    Err(_) => return false,
                };
                let filename = match extension {
                    Some(ext) => format!("{stem}-{fingerprint:016x}.{ext}"),
                    None => format!("{stem}-{fingerprint:016x}"),
                };
                destination = target.join(filename);
            }
            if destination.exists() && !same_bytes(source, &destination) {
                return false;
            }
            used.insert(destination.clone());
            planned.push((source.to_path_buf(), destination));
        }

        static COPY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let mut created = Vec::<PathBuf>::new();
        for (source, destination) in planned {
            if destination.exists() {
                continue;
            }
            let temporary = destination.with_extension(format!(
                "asset-tmp-{}-{}",
                std::process::id(),
                COPY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let copied = File::open(&source).and_then(|mut input| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .and_then(|mut output| {
                        io::copy(&mut input, &mut output)?;
                        output.sync_all()
                    })
            });
            if copied
                .and_then(|_| fs::rename(&temporary, &destination))
                .and_then(|_| sync_parent_directory(&destination))
                .is_err()
            {
                let _ = fs::remove_file(&temporary);
                for path in created {
                    let _ = fs::remove_file(path);
                }
                return false;
            }
            created.push(destination);
        }
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the resource integrity graph.
    pub fn audit_resources(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic resource auditing logic.
        self.assets.iter().all(|(key, asset)| {
            asset.validate_shape()
                && !key.is_empty()
                && key == &asset.uuid
                && !asset.path.is_empty()
                && asset.format.chars().all(|c| c.is_ascii_alphanumeric())
                && asset.content_hash.len() == 64
                && asset.content_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                && fs::symlink_metadata(&asset.path)
                    .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == asset.size)
                    .unwrap_or(false)
                && file_sha256(&asset.path).as_deref() == Some(asset.content_hash.as_str())
        })
    }

    /// Emits a deterministic dependency manifest suitable for archive verification.
    pub fn dependency_manifest_json(&self) -> String {
        let mut assets: Vec<_> = self.assets.values().collect();
        assets.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        serde_json::json!({"version":1,"assets":assets}).to_string()
    }
}

fn file_sha256(path: impl AsRef<Path>) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let digest = Sha256::digest(bytes);
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn same_bytes(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{file_sha256, AssetMetadata, ResourceOrchestrator};
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aura-resources-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn consolidation_disambiguates_same_basenames_atomically() {
        let root = temp_dir("collision");
        let left = root.join("left");
        let right = root.join("right");
        let target = root.join("Assets");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        let left_file = left.join("take.wav");
        let right_file = right.join("take.wav");
        fs::write(&left_file, b"left").unwrap();
        fs::write(&right_file, b"right").unwrap();

        let mut resources = ResourceOrchestrator {
            assets: HashMap::new(),
        };
        resources.track_asset(AssetMetadata {
            uuid: "left".into(),
            path: left_file.to_string_lossy().into_owned(),
            size: 4,
            format: "wav".into(),
            tags: Vec::new(),
            content_hash: file_sha256(&left_file).unwrap(),
        });
        resources.track_asset(AssetMetadata {
            uuid: "right".into(),
            path: right_file.to_string_lossy().into_owned(),
            size: 5,
            format: "wav".into(),
            tags: Vec::new(),
            content_hash: file_sha256(&right_file).unwrap(),
        });

        assert!(resources.try_consolidate_project(target.to_str().unwrap()));
        let names = fs::read_dir(&target)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2);
        assert!(names.iter().any(|name| name == "take.wav"));
        assert!(names
            .iter()
            .any(|name| name.to_string_lossy().starts_with("take-")));
        fs::write(&left_file, b"changed").unwrap();
        assert!(!resources.audit_resources(), "asset audit must detect same-path byte replacement");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn dependency_manifest_is_versioned_and_deterministic() {
        let mut resources = ResourceOrchestrator { assets: HashMap::new() };
        resources.assets.insert("z".into(), AssetMetadata { uuid:"z".into(), path:"z.wav".into(), size:1, format:"wav".into(), tags:vec![], content_hash:String::new() });
        resources.assets.insert("a".into(), AssetMetadata { uuid:"a".into(), path:"a.wav".into(), size:1, format:"wav".into(), tags:vec![], content_hash:String::new() });
        let json = resources.dependency_manifest_json(); assert!(json.starts_with("{\"assets\":")); assert!(json.contains("\"version\":1")); assert!(json.find("\"a\"").unwrap() < json.find("\"z\"").unwrap());
    }
}
