//! ProjectArchive methods: package, restore, verify, inspect and the
//! card-threshold reporting, split out of project_archive.rs.
use super::*;

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
        // Parse straight off the bounded reader instead of materializing the
        // bytes first: the Value is needed below anyway, so the raw buffer was
        // pure extra peak memory on top of it.
        let mut bounded = project_member.take(MAX_PROJECT_JSON_BYTES + 1);
        let mut project_bytes = Vec::new();
        bounded.read_to_end(&mut project_bytes)?;
        let project: serde_json::Value = serde_json::from_slice(&project_bytes)
            .context("archive project.json is invalid")?;
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
