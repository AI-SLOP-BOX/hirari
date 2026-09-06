//! Project package/archive support.
//!
//! A package is a directory containing the project JSON, a deterministic
//! dependency manifest, and copied media/plugin state.  Keeping the format
//! directory based makes it usable without an external archive SDK while
//! still providing the same "collect and copy" semantics as a DAW archive
//! command.  The manifest is content addressed so an archive can be checked
//! before it is opened or transferred to another machine.

use crate::project::ProjectDocument;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const ARCHIVE_SCHEMA_VERSION: u32 = 1;
const MAX_DEPENDENCIES: usize = 65_536;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub source: String,
    pub package_path: String,
    pub bytes: u64,
    pub sha256: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveManifest {
    pub schema_version: u32,
    pub project_file: String,
    pub entries: Vec<ArchiveEntry>,
    pub missing: Vec<String>,
    pub total_bytes: u64,
}

impl ArchiveManifest {
    pub fn validate(&self) -> bool {
        if self.schema_version != ARCHIVE_SCHEMA_VERSION
            || self.project_file.trim().is_empty()
            || self.entries.len() > MAX_DEPENDENCIES
            || self.missing.len() > MAX_DEPENDENCIES
        {
            return false;
        }
        let mut package_paths = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut total = 0u64;
        for entry in &self.entries {
            if entry.source.trim().is_empty()
                || entry.package_path.trim().is_empty()
                || !safe_package_path(Path::new(&entry.package_path))
                || !sources.insert(entry.source.clone())
                || !package_paths.insert(entry.package_path.clone())
                || (entry.exists && (entry.sha256.len() != 64 || entry.bytes > MAX_FILE_BYTES))
                || (!entry.exists && (entry.bytes != 0 || !entry.sha256.is_empty()))
            {
                return false;
            }
            total = match total.checked_add(entry.bytes) {
                Some(value) => value,
                None => return false,
            };
        }
        let mut missing = BTreeSet::new();
        total == self.total_bytes
            && self.missing.iter().all(|path| {
                !path.trim().is_empty() && missing.insert(path) && !sources.contains(path)
            })
    }
}

#[derive(Debug, Clone)]
pub struct ProjectArchive;

impl ProjectArchive {
    /// Build a manifest without modifying the filesystem.
    pub fn plan(
        project_file: impl AsRef<Path>,
        project: &ProjectDocument,
    ) -> Result<ArchiveManifest> {
        let project_file = project_file.as_ref();
        if project_file.as_os_str().is_empty() {
            bail!("project file path is empty");
        }
        let project_root = project_file.parent().unwrap_or_else(|| Path::new("."));
        let mut paths = BTreeSet::new();
        for region in &project.regions {
            if !region.path.trim().is_empty() {
                paths.insert(resolve_path(project_root, &region.path));
            }
        }
        for track in &project.tracks {
            for path in &track.sandbox_plugin_paths {
                if !path.trim().is_empty() {
                    paths.insert(resolve_path(project_root, path));
                }
            }
        }
        if paths.len() > MAX_DEPENDENCIES {
            bail!("project has too many dependencies");
        }
        let mut used = BTreeMap::<String, String>::new();
        let mut entries = Vec::new();
        let mut missing = Vec::new();
        let mut total_bytes = 0u64;
        for source in paths {
            let source_text = source.to_string_lossy().into_owned();
            let metadata = match fs::metadata(&source) {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => {
                    missing.push(source_text);
                    continue;
                }
            };
            if metadata.len() > MAX_FILE_BYTES {
                bail!("dependency is too large: {}", source.display());
            }
            let hash = sha256_file(&source)?;
            let base = source
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("asset");
            let package_path = unique_package_path(base, &hash, &mut used);
            total_bytes = total_bytes
                .checked_add(metadata.len())
                .context("archive size overflow")?;
            entries.push(ArchiveEntry {
                source: source_text,
                package_path,
                bytes: metadata.len(),
                sha256: hash,
                exists: true,
            });
        }
        let manifest = ArchiveManifest {
            schema_version: ARCHIVE_SCHEMA_VERSION,
            project_file: project_file.to_string_lossy().into_owned(),
            entries,
            missing,
            total_bytes,
        };
        if !manifest.validate() {
            bail!("generated archive manifest is invalid");
        }
        Ok(manifest)
    }

    /// Copy a project and all available dependencies into a new package.
    /// Existing destinations are rejected to avoid silently mixing versions.
    pub fn create(
        project_file: impl AsRef<Path>,
        project_json: &[u8],
        project: &ProjectDocument,
        destination: impl AsRef<Path>,
    ) -> Result<ArchiveManifest> {
        let destination = destination.as_ref();
        if destination.exists() {
            bail!(
                "archive destination already exists: {}",
                destination.display()
            );
        }
        let manifest = Self::plan(&project_file, project)?;
        let temp = destination.with_extension("aura-package.tmp");
        if temp.exists() {
            bail!("temporary archive destination already exists");
        }
        fs::create_dir_all(temp.join("Media"))?;
        let result = (|| -> Result<()> {
            atomic_write(&temp.join("project.json"), project_json)?;
            for entry in &manifest.entries {
                let target = temp.join(&entry.package_path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&entry.source, &target)
                    .with_context(|| format!("copying {}", entry.source))?;
                let copied = fs::metadata(&target)?;
                if copied.len() != entry.bytes || sha256_file(&target)? != entry.sha256 {
                    bail!("dependency changed while archiving: {}", entry.source);
                }
            }
            atomic_write(
                &temp.join("manifest.json"),
                &serde_json::to_vec_pretty(&manifest)?,
            )?;
            fs::rename(&temp, destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temp);
        }
        result.map(|_| manifest)
    }

    pub fn read_manifest(package: impl AsRef<Path>) -> Result<ArchiveManifest> {
        let manifest_path = package.as_ref().join("manifest.json");
        let file = File::open(&manifest_path)?;
        let size = file.metadata()?.len();
        if size > MAX_MANIFEST_BYTES {
            bail!("archive manifest exceeds {} byte limit", MAX_MANIFEST_BYTES);
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            bail!("archive manifest exceeds {} byte limit", MAX_MANIFEST_BYTES);
        }
        let manifest: ArchiveManifest =
            serde_json::from_slice(&bytes).context("invalid archive manifest")?;
        if !manifest.validate() {
            bail!("archive manifest failed validation");
        }
        Ok(manifest)
    }

    /// Verify package files against the recorded content hashes.
    pub fn verify(package: impl AsRef<Path>) -> Result<Vec<String>> {
        let package = package.as_ref();
        let manifest = Self::read_manifest(package)?;
        let mut errors = manifest.missing.clone();
        for entry in &manifest.entries {
            let path = package.join(&entry.package_path);
            if !path.is_file() {
                errors.push(format!("missing packaged file: {}", entry.package_path));
                continue;
            }
            let metadata = fs::metadata(&path)?;
            if metadata.len() != entry.bytes {
                errors.push(format!("size mismatch: {}", entry.package_path));
                continue;
            }
            if sha256_file(&path)? != entry.sha256 {
                errors.push(format!("checksum mismatch: {}", entry.package_path));
            }
        }
        Ok(errors)
    }

    /// Restores a verified package into a new project directory. The
    /// destination must not exist; restoration is staged in a sibling
    /// temporary directory and published only after every dependency copy
    /// succeeds, preventing half-restored projects after interruption.
    pub fn restore(
        package: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<Vec<PathBuf>> {
        let package = package.as_ref();
        let destination = destination.as_ref();
        if destination.exists() {
            bail!(
                "restore destination already exists: {}",
                destination.display()
            );
        }
        if !Self::verify(package)?.is_empty() {
            bail!("archive verification failed");
        }
        let manifest = Self::read_manifest(package)?;
        let temp = destination.with_extension("aura-restore.tmp");
        if temp.exists() {
            bail!("temporary restore destination already exists");
        }
        fs::create_dir_all(&temp)?;
        let result = (|| -> Result<Vec<PathBuf>> {
            let project_source = package.join("project.json");
            if !project_source.is_file() {
                bail!("archive project.json is missing");
            }
            fs::copy(project_source, temp.join("project.json"))?;
            let mut restored = Vec::with_capacity(manifest.entries.len() + 1);
            restored.push(destination.join("project.json"));
            for entry in &manifest.entries {
                let source = package.join(&entry.package_path);
                let target = temp.join(&entry.package_path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source, &target)?;
                restored.push(destination.join(&entry.package_path));
            }
            fs::rename(&temp, destination)?;
            Ok(restored)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temp);
        }
        result
    }
}

fn resolve_path(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn unique_package_path(base: &str, hash: &str, used: &mut BTreeMap<String, String>) -> String {
    let stem = Path::new(base)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("asset");
    let ext = Path::new(base)
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| format!(".{v}"))
        .unwrap_or_default();
    let first = format!("Media/{base}");
    if !used.contains_key(&first) {
        used.insert(first.clone(), hash.to_owned());
        return first;
    }
    for suffix in 0u32.. {
        let discriminator = if suffix == 0 {
            hash[..8].to_owned()
        } else {
            format!("{}_{}", &hash[..8], suffix + 1)
        };
        let candidate = format!("Media/{stem}_{discriminator}{ext}");
        if !used.contains_key(&candidate) {
            used.insert(candidate.clone(), hash.to_owned());
            return candidate;
        }
    }
    unreachable!("u32 package suffix space exhausted")
}

fn safe_package_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path.starts_with("Media")
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    let mut file = File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::ProjectMetadata;
    use crate::project::{ProjectDocument, ProjectRegion};

    fn project(path: &str) -> ProjectDocument {
        ProjectDocument {
            schema_version: 1,
            contract_version: 1,
            project_id: "p".into(),
            metadata: ProjectMetadata::default(),
            sample_rate: 48_000.0,
            master_gain: 1.0,
            cycle_start_sample: 0,
            cycle_end_sample: 0,
            cycle_enabled: false,
            metronome_enabled: false,
            tracks: vec![],
            aux_track_ids: vec![],
            regions: vec![ProjectRegion {
                id: 1,
                track_id: 1,
                name: "a".into(),
                path: path.into(),
                start: 0,
                length: 1,
                source_offset: 0,
                base_source_offset: 0,
                base_length: 1,
                muted: false,
                clip_gain: 1.0,
                fade_in_samples: 0,
                fade_out_samples: 0,
                warp_ratio: 1.0,
                pitch_semitones: 0.0,
                reverse: false,
                loop_count: 1,
            }],
            plugin_instances: vec![],
            midi_learn_mappings: vec![],
            midi_notes: vec![],
            chord_track: vec![],
            midi_events: vec![],
            tempo_events: vec![],
            time_signature_events: vec![],
            macro_mappings: vec![],
            warp_markers: vec![],
            render_targets: vec![],
            freeze_artifacts: vec![],
            sidechain_routes: vec![],
            feedback_routes: vec![],
            audio_routes: vec![],
            openutau_vocals: vec![],
            comp_takes: vec![],
            comp_segments: vec![],
            track_stacks: vec![],
            markers: vec![],
            vca_groups: vec![],
            hardware_inserts: vec![],
        }
    }

    #[test]
    fn plans_missing_and_existing_dependencies() {
        let root = std::env::temp_dir().join(format!("aura-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let manifest = ProjectArchive::plan(root.join("song.aura"), &project("take.wav")).unwrap();
        assert!(manifest.validate());
        assert_eq!(manifest.entries.len(), 1);
        assert!(manifest.missing.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn creates_and_verifies_package() {
        let root = std::env::temp_dir().join(format!("aura-package-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.aura-package");
        let manifest = ProjectArchive::create(
            root.join("song.aura"),
            b"{}",
            &project("take.wav"),
            &package,
        )
        .unwrap();
        assert_eq!(
            ProjectArchive::verify(&package).unwrap(),
            Vec::<String>::new()
        );
        assert!(manifest.validate());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gives_same_named_dependencies_unique_package_paths() {
        let root =
            std::env::temp_dir().join(format!("aura-archive-collision-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::write(root.join("one/take.wav"), b"same audio").unwrap();
        fs::write(root.join("two/take.wav"), b"same audio").unwrap();
        let mut document = project("one/take.wav");
        document.regions.push(ProjectRegion {
            id: 2,
            path: "two/take.wav".into(),
            ..document.regions[0].clone()
        });

        let manifest = ProjectArchive::plan(root.join("song.aura"), &document).unwrap();

        assert!(manifest.validate());
        assert_eq!(manifest.entries.len(), 2);
        assert_ne!(
            manifest.entries[0].package_path,
            manifest.entries[1].package_path
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_rejects_traversal_and_duplicate_missing_paths() {
        let mut manifest = ArchiveManifest {
            schema_version: ARCHIVE_SCHEMA_VERSION,
            project_file: "song.aura".into(),
            entries: vec![ArchiveEntry {
                source: "take.wav".into(),
                package_path: "Media/../take.wav".into(),
                bytes: 1,
                sha256: "0".repeat(64),
                exists: true,
            }],
            missing: vec![],
            total_bytes: 1,
        };
        assert!(!manifest.validate());
        manifest.entries.clear();
        manifest.total_bytes = 0;
        manifest.missing = vec!["lost.wav".into(), "lost.wav".into()];
        assert!(!manifest.validate());
    }

    #[test]
    fn verify_reports_packaged_file_size_changes() {
        let root = std::env::temp_dir().join(format!("aura-package-tamper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.aura-package");
        let manifest = ProjectArchive::create(
            root.join("song.aura"),
            b"{}",
            &project("take.wav"),
            &package,
        )
        .unwrap();
        fs::write(
            package.join(&manifest.entries[0].package_path),
            b"longer audio",
        )
        .unwrap();

        let errors = ProjectArchive::verify(&package).unwrap();

        assert_eq!(
            errors,
            vec![format!(
                "size mismatch: {}",
                manifest.entries[0].package_path
            )]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_manifest_is_rejected_before_allocation() {
        let root =
            std::env::temp_dir().join(format!("aura-oversized-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let package = root.join("song.aura-package");
        fs::create_dir_all(&package).unwrap();
        let file = File::create(package.join("manifest.json")).unwrap();
        file.set_len(MAX_MANIFEST_BYTES + 1).unwrap();

        let result = ProjectArchive::read_manifest(&package);

        assert!(result.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restores_verified_package_atomically() {
        let root = std::env::temp_dir().join(format!("aura-restore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.aura-package");
        ProjectArchive::create(
            root.join("song.aura"),
            b"{}",
            &project("take.wav"),
            &package,
        )
        .unwrap();
        let destination = root.join("restored");
        let restored = ProjectArchive::restore(&package, &destination).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(
            fs::read(destination.join("Media/take.wav")).unwrap(),
            b"audio"
        );
        let _ = fs::remove_dir_all(root);
    }
}
