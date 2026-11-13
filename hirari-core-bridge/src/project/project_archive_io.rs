//! Archive internals: packaged-project path rewriting, path verification and
//! atomic writes, split out of project_archive.rs.
use super::*;

pub(crate) fn rewrite_packaged_project_paths(
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

pub(crate) fn verify_packaged_project_paths(
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

pub(crate) fn verify_packaged_project_member_paths(
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

pub(crate) fn package_temp_path(destination: &Path) -> PathBuf {
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

pub(crate) fn resolve_path(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(crate) fn unique_package_path(base: &str, hash: &str, used: &mut BTreeMap<String, String>) -> String {
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

pub(crate) fn safe_package_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path.starts_with("Media")
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
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

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    let mut file = File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(crate) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))
}

pub(crate) fn sync_directory_tree(root: &Path) -> Result<()> {
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
