//! Project package/archive support.
//!
//! A portable project package is one ZIP file containing project JSON, a
//! deterministic dependency manifest, and copied project media. Legacy
//! directory APIs remain available for old packages; UI archive/restore uses
//! the single-file APIs. The manifest is content addressed so packages can
//! be checked before they are opened or transferred.

use crate::project::ProjectDocument;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use zip::read::ZipArchive as ReadZipArchive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const ARCHIVE_SCHEMA_VERSION: u32 = 1;
const MAX_DEPENDENCIES: usize = 65_536;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROJECT_JSON_BYTES: u64 = 1024 * 1024 * 1024;

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
        for artifact in &project.freeze_artifacts {
            if !artifact.path.trim().is_empty() {
                paths.insert(resolve_path(project_root, &artifact.path));
            }
        }
        for vocal in &project.openutau_vocals {
            for path in [&vocal.source_path, &vocal.rendered_audio_path] {
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
        let project_file = project_file.as_ref();
        let destination = destination.as_ref();
        if destination.exists() {
            bail!(
                "archive destination already exists: {}",
                destination.display()
            );
        }
        let manifest = Self::plan(project_file, project)?;
        if !manifest.missing.is_empty() {
            bail!(
                "cannot package project with {} missing media dependency/dependencies",
                manifest.missing.len()
            );
        }
        let temp = destination.with_extension("hirari-package.tmp");
        if temp.exists() {
            bail!("temporary archive destination already exists");
        }
        fs::create_dir_all(temp.join("Media"))?;
        let result = (|| -> Result<()> {
            // A package must remain usable after its original source tree is
            // moved or removed. Store references to the copied package paths
            // in its project document instead of preserving source-machine
            // absolute paths.
            let mut packaged_project: serde_json::Value =
                serde_json::from_slice(project_json).context("invalid project JSON")?;
            let project_root = project_file.parent().unwrap_or_else(|| Path::new("."));
            rewrite_packaged_project_paths(&mut packaged_project, project_root, &manifest);
            atomic_write(
                &temp.join("project.json"),
                &serde_json::to_vec_pretty(&packaged_project)?,
            )?;
            for entry in &manifest.entries {
                let target = temp.join(&entry.package_path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&entry.source, &target)
                    .with_context(|| format!("copying {}", entry.source))?;
                File::open(&target)?.sync_all()?;
                let copied = fs::metadata(&target)?;
                if copied.len() != entry.bytes || sha256_file(&target)? != entry.sha256 {
                    bail!("dependency changed while archiving: {}", entry.source);
                }
            }
            atomic_write(
                &temp.join("manifest.json"),
                &serde_json::to_vec_pretty(&manifest)?,
            )?;
            let verification = Self::verify(&temp)?;
            if !verification.is_empty() {
                bail!("package verification failed: {}", verification.join("; "));
            }
            sync_directory_tree(&temp)?;
            fs::rename(&temp, destination)?;
            sync_parent_directory(destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temp);
        }
        result.map(|_| manifest)
    }

    /// Create a portable, single-file ZIP package containing the project and
    /// every referenced media dependency. The live `.hirari` document stays
    /// lightweight; this is the explicit collect-and-transfer operation.
    pub fn create_single_file(
        project_file: impl AsRef<Path>,
        project_json: &[u8],
        project: &ProjectDocument,
        destination: impl AsRef<Path>,
    ) -> Result<ArchiveManifest> {
        let project_file = project_file.as_ref();
        let destination = destination.as_ref();
        if destination.exists() {
            bail!(
                "archive destination already exists: {}",
                destination.display()
            );
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let temp = package_temp_path(destination);
        let manifest = Self::plan(project_file, project)?;
        if !manifest.missing.is_empty() {
            bail!(
                "cannot package project with {} missing media dependency/dependencies",
                manifest.missing.len()
            );
        }
        let mut packaged_project: serde_json::Value =
            serde_json::from_slice(project_json).context("invalid project JSON")?;
        let project_root = project_file.parent().unwrap_or_else(|| Path::new("."));
        rewrite_packaged_project_paths(&mut packaged_project, project_root, &manifest);
        let packaged_project = serde_json::to_vec_pretty(&packaged_project)?;

        // Source paths are only needed while rewriting the project. Do not
        // leak the source machine's absolute paths into a package manifest.
        let mut portable_manifest = manifest.clone();
        portable_manifest.project_file = "project.json".into();
        for entry in &mut portable_manifest.entries {
            entry.source = entry.package_path.clone();
        }
        let manifest_bytes = serde_json::to_vec_pretty(&portable_manifest)?;

        let result = (|| -> Result<()> {
            let output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .with_context(|| format!("could not create archive {}", temp.display()))?;
            let mut archive = ZipWriter::new(output);
            let json_options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            archive.start_file("project.json", json_options)?;
            archive.write_all(&packaged_project)?;
            archive.start_file("manifest.json", json_options)?;
            archive.write_all(&manifest_bytes)?;

            // Audio is commonly already compressed. Store it directly to
            // avoid spending minutes recompressing large recordings.
            let media_options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let mut buffer = vec![0u8; 1024 * 1024];
            for entry in &manifest.entries {
                archive.start_file(&entry.package_path, media_options)?;
                let mut source = File::open(&entry.source)
                    .with_context(|| format!("could not read media {}", entry.source))?;
                loop {
                    let count = source.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    archive.write_all(&buffer[..count])?;
                }
                if sha256_file(Path::new(&entry.source))? != entry.sha256 {
                    bail!("dependency changed while archiving: {}", entry.source);
                }
            }
            let mut output = archive
                .finish()
                .context("could not finish project package")?;
            output.flush()?;
            output.sync_all()?;
            drop(output);
            let errors = Self::verify_single_file(&temp)?;
            if !errors.is_empty() {
                bail!("package verification failed: {}", errors.join("; "));
            }
            fs::rename(&temp, destination)
                .with_context(|| format!("could not publish {}", destination.display()))?;
            sync_parent_directory(destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result.map(|()| manifest)
    }

    /// Read the bounded manifest from a single-file project package.
    pub fn read_single_file_manifest(package: impl AsRef<Path>) -> Result<ArchiveManifest> {
        let file = File::open(package)?;
        let mut archive = ReadZipArchive::new(file).context("project package is not a ZIP file")?;
        let member = archive
            .by_name("manifest.json")
            .context("project package manifest is missing")?;
        if member.size() > MAX_MANIFEST_BYTES {
            bail!("archive manifest exceeds {} byte limit", MAX_MANIFEST_BYTES);
        }
        let mut bytes = Vec::with_capacity(member.size() as usize);
        member
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
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

    /// Verify all ZIP members, project references, sizes, and SHA-256 hashes.
    pub fn verify_single_file(package: impl AsRef<Path>) -> Result<Vec<String>> {
        let package = package.as_ref();
        let manifest = Self::read_single_file_manifest(package)?;
        let expected_count = manifest.entries.len().saturating_add(2);
        let mut archive = ReadZipArchive::new(File::open(package)?)
            .context("project package is not a ZIP file")?;
        let mut names = BTreeSet::new();
        for index in 0..archive.len() {
            let member = archive.by_index(index)?;
            if !names.insert(member.name().to_owned()) {
                bail!("project package contains a duplicate ZIP member");
            }
        }
        if names.len() != expected_count
            || !names.contains("project.json")
            || !names.contains("manifest.json")
        {
            bail!("project package member list does not match its manifest");
        }

        let mut errors = manifest.missing.clone();
        for entry in &manifest.entries {
            let mut member = archive.by_name(&entry.package_path)?;
            if member.size() != entry.bytes {
                errors.push(format!("size mismatch: {}", entry.package_path));
                continue;
            }
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = member.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            if format!("{:x}", digest.finalize()) != entry.sha256 {
                errors.push(format!("checksum mismatch: {}", entry.package_path));
            }
        }

        let project_member = archive.by_name("project.json")?;
        if project_member.size() > MAX_PROJECT_JSON_BYTES {
            bail!("archive project.json exceeds supported size");
        }
        let mut project_bytes = Vec::with_capacity(project_member.size() as usize);
        project_member
            .take(MAX_PROJECT_JSON_BYTES + 1)
            .read_to_end(&mut project_bytes)?;
        let project: serde_json::Value =
            serde_json::from_slice(&project_bytes).context("archive project.json is invalid")?;
        verify_packaged_project_member_paths(&project, &manifest, &mut errors);
        errors.sort();
        errors.dedup();
        Ok(errors)
    }

    /// Restore a verified single-file package into a new project directory.
    /// Extraction is restricted to manifest paths and published by one final
    /// directory rename after a second on-disk verification.
    pub fn restore_single_file(
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
        let errors = Self::verify_single_file(package)?;
        if !errors.is_empty() {
            bail!("archive verification failed: {}", errors.join("; "));
        }
        let manifest = Self::read_single_file_manifest(package)?;
        let temp = destination.with_extension("hirari-restore.tmp");
        if temp.exists() {
            bail!("temporary restore destination already exists");
        }
        fs::create_dir_all(&temp)?;
        let result = (|| -> Result<Vec<PathBuf>> {
            let mut archive = ReadZipArchive::new(File::open(package)?)?;
            for name in ["project.json", "manifest.json"] {
                let mut member = archive.by_name(name)?;
                let limit = if name == "manifest.json" {
                    MAX_MANIFEST_BYTES
                } else {
                    MAX_PROJECT_JSON_BYTES
                };
                if member.size() > limit {
                    bail!("archive member exceeds supported size: {name}");
                }
                let mut target = File::create(temp.join(name))?;
                std::io::copy(&mut member, &mut target)?;
                target.sync_all()?;
            }
            let mut restored = Vec::with_capacity(manifest.entries.len() + 1);
            restored.push(destination.join("project.json"));
            for entry in &manifest.entries {
                if !safe_package_path(Path::new(&entry.package_path)) {
                    bail!("unsafe path in package manifest");
                }
                let target = temp.join(&entry.package_path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut member = archive.by_name(&entry.package_path)?;
                let mut output = File::create(&target)?;
                std::io::copy(&mut member, &mut output)?;
                output.sync_all()?;
                if fs::metadata(&target)?.len() != entry.bytes
                    || sha256_file(&target)? != entry.sha256
                {
                    bail!("restored media failed verification: {}", entry.package_path);
                }
                restored.push(destination.join(&entry.package_path));
            }
            let errors = Self::verify(&temp)?;
            if !errors.is_empty() {
                bail!(
                    "restored package verification failed: {}",
                    errors.join("; ")
                );
            }
            fs::remove_file(temp.join("manifest.json"))?;
            sync_directory_tree(&temp)?;
            fs::rename(&temp, destination)?;
            sync_parent_directory(destination)?;
            Ok(restored)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temp);
        }
        result
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
        let project_path = package.join("project.json");
        let project_bytes = fs::read(&project_path).context("archive project.json is missing")?;
        let project: serde_json::Value =
            serde_json::from_slice(&project_bytes).context("archive project.json is invalid")?;
        verify_packaged_project_paths(&project, package, &mut errors);
        errors.sort();
        errors.dedup();
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
        let temp = destination.with_extension("hirari-restore.tmp");
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
            fs::copy(package.join("manifest.json"), temp.join("manifest.json"))?;
            let mut restored = Vec::with_capacity(manifest.entries.len() + 1);
            restored.push(destination.join("project.json"));
            for entry in &manifest.entries {
                let source = package.join(&entry.package_path);
                let target = temp.join(&entry.package_path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source, &target)?;
                File::open(&target)?.sync_all()?;
                restored.push(destination.join(&entry.package_path));
            }
            let errors = Self::verify(&temp)?;
            if !errors.is_empty() {
                bail!(
                    "restored package verification failed: {}",
                    errors.join("; ")
                );
            }
            fs::remove_file(temp.join("manifest.json"))?;
            sync_directory_tree(&temp)?;
            fs::rename(&temp, destination)?;
            sync_parent_directory(destination)?;
            Ok(restored)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temp);
        }
        result
    }
}

fn rewrite_packaged_project_paths(
    project: &mut serde_json::Value,
    project_root: &Path,
    manifest: &ArchiveManifest,
) {
    let replacements: BTreeMap<String, &str> = manifest
        .entries
        .iter()
        .map(|entry| (entry.source.clone(), entry.package_path.as_str()))
        .collect();
    let replace = |path: &mut serde_json::Value| {
        let Some(raw) = path.as_str() else {
            return;
        };
        let source = resolve_path(project_root, raw)
            .to_string_lossy()
            .into_owned();
        if let Some(package_path) = replacements.get(&source) {
            *path = serde_json::Value::String((*package_path).to_owned());
        }
    };
    if let Some(regions) = project
        .get_mut("regions")
        .and_then(serde_json::Value::as_array_mut)
    {
        for region in regions {
            if let Some(path) = region.get_mut("path") {
                replace(path);
            }
        }
    }
    for collection in ["freeze_artifacts", "openutau_vocals"] {
        if let Some(entries) = project
            .get_mut(collection)
            .and_then(serde_json::Value::as_array_mut)
        {
            for entry in entries {
                let path_keys: &[&str] = if collection == "freeze_artifacts" {
                    &["path"]
                } else {
                    &["source_path", "rendered_audio_path"]
                };
                for key in path_keys {
                    if let Some(path) = entry.get_mut(*key) {
                        replace(path);
                    }
                }
            }
        }
    }
}

fn verify_packaged_project_paths(
    project: &serde_json::Value,
    package: &Path,
    errors: &mut Vec<String>,
) {
    let mut check = |raw: &str| {
        if raw.trim().is_empty() {
            return;
        }
        let relative = Path::new(raw);
        if !safe_package_path(relative) {
            errors.push(format!("project references an external dependency: {raw}"));
        } else if !package.join(relative).is_file() {
            errors.push(format!("missing project dependency: {raw}"));
        }
    };
    if let Some(regions) = project.get("regions").and_then(serde_json::Value::as_array) {
        for region in regions {
            if let Some(path) = region.get("path").and_then(serde_json::Value::as_str) {
                check(path);
            }
        }
    }
    for collection in ["freeze_artifacts", "openutau_vocals"] {
        if let Some(entries) = project
            .get(collection)
            .and_then(serde_json::Value::as_array)
        {
            for entry in entries {
                let path_keys: &[&str] = if collection == "freeze_artifacts" {
                    &["path"]
                } else {
                    &["source_path", "rendered_audio_path"]
                };
                for key in path_keys {
                    if let Some(path) = entry.get(*key).and_then(serde_json::Value::as_str) {
                        check(path);
                    }
                }
            }
        }
    }
}

fn verify_packaged_project_member_paths(
    project: &serde_json::Value,
    manifest: &ArchiveManifest,
    errors: &mut Vec<String>,
) {
    let available = manifest
        .entries
        .iter()
        .map(|entry| entry.package_path.as_str())
        .collect::<BTreeSet<_>>();
    let mut check = |raw: &str| {
        if raw.trim().is_empty() {
            return;
        }
        if !safe_package_path(Path::new(raw)) || !available.contains(raw) {
            errors.push(format!("project references an external dependency: {raw}"));
        }
    };
    if let Some(regions) = project.get("regions").and_then(serde_json::Value::as_array) {
        for region in regions {
            if let Some(path) = region.get("path").and_then(serde_json::Value::as_str) {
                check(path);
            }
        }
    }
    for collection in ["freeze_artifacts", "openutau_vocals"] {
        if let Some(entries) = project
            .get(collection)
            .and_then(serde_json::Value::as_array)
        {
            let path_keys: &[&str] = if collection == "freeze_artifacts" {
                &["path"]
            } else {
                &["source_path", "rendered_audio_path"]
            };
            for entry in entries {
                for key in path_keys {
                    if let Some(path) = entry.get(*key).and_then(serde_json::Value::as_str) {
                        check(path);
                    }
                }
            }
        }
    }
}

fn package_temp_path(destination: &Path) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Hirari Project.hirari-package");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    parent.join(format!(".{name}.tmp-{}-{nonce}", std::process::id()))
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

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))
}

fn sync_directory_tree(root: &Path) -> Result<()> {
    fn collect(directory: &Path, directories: &mut Vec<PathBuf>) -> Result<()> {
        directories.push(directory.to_path_buf());
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                collect(&entry.path(), directories)?;
            }
        }
        Ok(())
    }

    let mut directories = Vec::new();
    collect(root, &mut directories)?;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(&directory)?;
    }
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
                source_length: 1,
                source_sample_rate: 48_000,
                source_offset: 0,
                base_source_offset: 0,
                base_length: 1,
                muted: false,
                clip_gain: 1.0,
                fade_in_samples: 0,
                fade_out_samples: 0,
                warp_ratio: 1.0,
                pitch_preserve_warp: false,
                pitch_semitones: 0.0,
                reverse: false,
                loop_count: 1,
                sync_group: 0,
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
            audio_input_assignments: vec![],
            step_sequencer_patterns: vec![],
            openutau_vocals: vec![],
            comp_takes: vec![],
            comp_segments: vec![],
            track_stacks: vec![],
            markers: vec![],
            vca_groups: vec![],
            hardware_inserts: vec![],
            control_room: None,
        }
    }

    #[test]
    fn plans_missing_and_existing_dependencies() {
        let root = std::env::temp_dir().join(format!("hirari-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let manifest =
            ProjectArchive::plan(root.join("song.hirari"), &project("take.wav")).unwrap();
        assert!(manifest.validate());
        assert_eq!(manifest.entries.len(), 1);
        assert!(manifest.missing.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn creates_and_verifies_package() {
        let root = std::env::temp_dir().join(format!("hirari-package-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.hirari-package");
        let manifest = ProjectArchive::create(
            root.join("song.hirari"),
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
            std::env::temp_dir().join(format!("hirari-archive-collision-{}", std::process::id()));
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

        let manifest = ProjectArchive::plan(root.join("song.hirari"), &document).unwrap();

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
            project_file: "song.hirari".into(),
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
        let root =
            std::env::temp_dir().join(format!("hirari-package-tamper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.hirari-package");
        let manifest = ProjectArchive::create(
            root.join("song.hirari"),
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
            std::env::temp_dir().join(format!("hirari-oversized-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let package = root.join("song.hirari-package");
        fs::create_dir_all(&package).unwrap();
        let file = File::create(package.join("manifest.json")).unwrap();
        file.set_len(MAX_MANIFEST_BYTES + 1).unwrap();

        let result = ProjectArchive::read_manifest(&package);

        assert!(result.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restores_verified_package_atomically() {
        let root = std::env::temp_dir().join(format!("hirari-restore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.hirari-package");
        ProjectArchive::create(
            root.join("song.hirari"),
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
