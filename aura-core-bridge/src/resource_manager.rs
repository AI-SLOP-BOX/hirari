use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub struct AssetMetadataRust {
    pub uuid: String,
    pub path: String,
    pub size: u64,
    pub format: String,
    pub tags: Vec<String>,
    pub is_missing: bool,
}

pub struct ResourceOrchestrator {
    pub assets: Vec<AssetMetadataRust>,
}

impl Default for ResourceOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceOrchestrator {
    pub fn new() -> Self {
        Self { assets: Vec::new() }
    }

    /// Indexes regular files below an absolute directory without reading file contents.
    pub fn scan_library(&mut self, root: String) {
        let root = PathBuf::from(root);
        if !root.is_absolute() || !root.is_dir() {
            return;
        }
        let root = match fs::canonicalize(root) {
            Ok(path) => path,
            Err(_) => return,
        };

        let mut found = Vec::new();
        Self::walk(&root, &mut found);
        found.sort();
        found.dedup();

        self.assets = found
            .into_iter()
            .filter_map(|path| {
                let metadata = fs::metadata(&path).ok()?;
                let format = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let path = path.to_string_lossy().into_owned();
                Some(AssetMetadataRust {
                    uuid: path.clone(),
                    path,
                    size: metadata.len(),
                    format,
                    tags: Vec::new(),
                    is_missing: false,
                })
            })
            .collect();
    }

    fn walk(directory: &Path, files: &mut Vec<PathBuf>) {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Discovery must not silently escape the selected library root
            // through a symlink. Explicitly supplied external assets remain
            // supported by consolidation, but scans stay tree-local.
            let link_metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if link_metadata.file_type().is_symlink() {
                continue;
            }
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                Self::walk(&path, files);
            } else if metadata.is_file() {
                if let Ok(path) = fs::canonicalize(path) {
                    files.push(path);
                }
            }
        }
    }

    /// Revalidates assets and recovers a missing path from an unambiguous filename match.
    pub fn resolve_missing_assets(&mut self) {
        let candidates: Vec<(std::ffi::OsString, String)> = self
            .assets
            .iter()
            .filter(|asset| Path::new(&asset.path).is_file())
            .filter_map(|asset| {
                Some((
                    Path::new(&asset.path).file_name()?.to_owned(),
                    asset.path.clone(),
                ))
            })
            .collect();
        for asset in &mut self.assets {
            if Path::new(&asset.path).is_file() {
                asset.is_missing = false;
                continue;
            }
            let name = match Path::new(&asset.path).file_name() {
                Some(name) => name,
                None => {
                    asset.is_missing = true;
                    continue;
                }
            };
            let matches: Vec<&String> = candidates
                .iter()
                .filter(|(candidate, _)| candidate == name)
                .map(|(_, path)| path)
                .collect();
            if matches.len() == 1 {
                asset.path = matches[0].clone();
                asset.is_missing = false;
            } else {
                asset.is_missing = true;
            }
        }
    }

    /// Compatibility wrapper for older callers. New code must use the checked
    /// API so a partial/failed consolidation cannot be reported as success.
    pub fn consolidate_project(&self, target_dir: String) {
        let _ = self.try_consolidate_project(target_dir);
    }

    /// Copies all assets transactionally into an absolute target directory.
    /// Missing files, symlinks, collisions, and publish failures are errors;
    /// files created by this invocation are removed on failure.
    pub fn try_consolidate_project(&self, target_dir: String) -> Result<(), String> {
        let target = PathBuf::from(target_dir);
        if !target.is_absolute()
            || (target.exists() && !target.is_dir())
            || fs::create_dir_all(&target).is_err()
        {
            return Err("invalid asset target directory".into());
        }
        // Complete the validation pass before publishing anything. This
        // keeps a missing/invalid later asset from leaving earlier assets
        // behind after an otherwise avoidable failure.
        for asset in &self.assets {
            let metadata = fs::symlink_metadata(&asset.path)
                .map_err(|error| format!("asset metadata failed: {error}"))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(format!("refusing invalid asset: {}", asset.path));
            }
            let source = fs::canonicalize(&asset.path)
                .map_err(|error| format!("asset canonicalization failed: {error}"))?;
            if !source.is_file() || source.file_name().and_then(|name| name.to_str()).is_none() {
                return Err(format!("asset is not a regular file: {}", asset.path));
            }
        }
        let mut copied = HashSet::new();
        let mut created = Vec::new();
        for asset in &self.assets {
            // Consolidation is an ownership boundary: never turn an
            // external symlink into a project-owned copy silently. Callers
            // must resolve and explicitly approve the target first.
            let link_metadata = match fs::symlink_metadata(&asset.path) {
                Ok(metadata) => metadata,
                Err(error) => return Err(format!("asset metadata failed: {error}")),
            };
            if link_metadata.file_type().is_symlink() {
                return Err(format!("refusing symlink asset: {}", asset.path));
            }
            let source = match fs::canonicalize(&asset.path) {
                Ok(path) if path.is_file() => path,
                _ => return Err(format!("asset is missing or not a file: {}", asset.path)),
            };
            if !copied.insert(source.clone()) {
                continue;
            }
            if source.file_name().and_then(|name| name.to_str()).is_none() {
                return Err(format!("asset has no valid filename: {}", source.display()));
            }
            let suffix = match Self::content_suffix(&source) {
                Ok(value) => value,
                Err(error) => return Err(format!("asset read failed: {error}")),
            };
            let stem = source
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("asset");
            let extension = source.extension().and_then(|v| v.to_str()).unwrap_or("");
            let filename = if extension.is_empty() {
                format!("{stem}-{suffix}")
            } else {
                format!("{stem}-{suffix}.{extension}")
            };
            let destination = target.join(filename);
            if destination.exists() {
                if !destination.is_file() {
                    return Err(format!(
                        "asset destination is not a file: {}",
                        destination.display()
                    ));
                }
                let existing_suffix = Self::content_suffix(&destination)
                    .map_err(|error| format!("asset destination read failed: {error}"))?;
                if existing_suffix != suffix {
                    return Err(format!(
                        "asset destination content mismatch: {}",
                        destination.display()
                    ));
                }
                continue;
            }
            let temporary = target.join(format!(
                ".{}.tmp-{}-{}",
                suffix,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or_default()
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
            if let Err(error) = copied {
                let _ = fs::remove_file(&temporary);
                for path in created {
                    let _ = fs::remove_file(path);
                }
                return Err(format!("asset copy failed: {error}"));
            }
            let published = File::open(&temporary)
                .and_then(|file| file.sync_all())
                .and_then(|_| fs::rename(&temporary, &destination))
                .and_then(|_| Self::sync_parent_directory(&destination))
                .is_ok();
            if !published {
                let _ = fs::remove_file(&temporary);
                for path in created {
                    let _ = fs::remove_file(path);
                }
                return Err(format!("asset publish failed: {}", destination.display()));
            }
            created.push(destination);
        }
        Ok(())
    }

    fn content_suffix(path: &Path) -> io::Result<String> {
        // Asset names are part of the project-owned namespace. Use the same
        // cryptographic identity family as project history rather than a
        // 32-bit non-cryptographic suffix that can collide easily.
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 1024 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let digest = hasher.finalize();
        Ok(digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::File::open(parent)?.sync_all()
    }

    pub fn audit_resource_manager(&self) -> bool {
        self.assets.iter().all(|asset| {
            !asset.path.is_empty() && !asset.is_missing && Path::new(&asset.path).is_file()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceOrchestrator;

    #[test]
    fn consolidation_uses_content_hash_and_atomic_publish() {
        let root = std::env::temp_dir().join(format!(
            "aura-assets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first_dir = root.join("one");
        let second_dir = root.join("two");
        let target = root.join("Assets");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();
        let first = first_dir.join("same.wav");
        let second = second_dir.join("same.wav");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let mut resources = ResourceOrchestrator::new();
        resources.assets.push(super::AssetMetadataRust {
            uuid: "one".into(),
            path: first.to_string_lossy().into_owned(),
            size: 5,
            format: "wav".into(),
            tags: Vec::new(),
            is_missing: false,
        });
        resources.assets.push(super::AssetMetadataRust {
            uuid: "two".into(),
            path: second.to_string_lossy().into_owned(),
            size: 6,
            format: "wav".into(),
            tags: Vec::new(),
            is_missing: false,
        });
        resources.consolidate_project(target.to_string_lossy().into_owned());
        let files = std::fs::read_dir(&target).unwrap().count();
        assert_eq!(files, 2);
        assert_eq!(
            std::fs::read_dir(&target)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
                .count(),
            0
        );
        let existing = std::fs::read_dir(&target)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&existing, b"tampered").unwrap();
        assert!(resources
            .try_consolidate_project(target.to_string_lossy().into_owned())
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn consolidation_rolls_back_when_a_later_asset_is_missing() {
        let root = std::env::temp_dir().join(format!(
            "aura-assets-rollback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source.wav");
        let target = root.join("Assets");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"valid").unwrap();

        let mut resources = ResourceOrchestrator::new();
        resources.assets.push(super::AssetMetadataRust {
            uuid: "valid".into(),
            path: source.to_string_lossy().into_owned(),
            size: 5,
            format: "wav".into(),
            tags: Vec::new(),
            is_missing: false,
        });
        resources.assets.push(super::AssetMetadataRust {
            uuid: "missing".into(),
            path: root.join("missing.wav").to_string_lossy().into_owned(),
            size: 0,
            format: "wav".into(),
            tags: Vec::new(),
            is_missing: true,
        });

        assert!(resources
            .try_consolidate_project(target.to_string_lossy().into_owned())
            .is_err());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_asset_relinks_only_on_unique_filename_match() {
        let root = std::env::temp_dir().join(format!("aura-relink-{}", std::process::id()));
        let available = root.join("available");
        std::fs::create_dir_all(&available).unwrap();
        let replacement = available.join("take.wav");
        std::fs::write(&replacement, b"wav").unwrap();
        let mut resources = ResourceOrchestrator::new();
        resources.assets.push(super::AssetMetadataRust {
            uuid: "missing".into(),
            path: root
                .join("old")
                .join("take.wav")
                .to_string_lossy()
                .into_owned(),
            size: 0,
            format: "wav".into(),
            tags: Vec::new(),
            is_missing: true,
        });
        resources.assets.push(super::AssetMetadataRust {
            uuid: "candidate".into(),
            path: replacement.to_string_lossy().into_owned(),
            size: 3,
            format: "wav".into(),
            tags: Vec::new(),
            is_missing: false,
        });
        resources.resolve_missing_assets();
        assert_eq!(resources.assets[0].path, replacement.to_string_lossy());
        assert!(!resources.assets[0].is_missing);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn library_scan_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "aura-library-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.join("outside.wav");
        let library = root.join("Library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, library.join("linked.wav")).unwrap();

        let mut resources = ResourceOrchestrator::new();
        resources.scan_library(library.to_string_lossy().into_owned());
        assert!(resources.assets.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
