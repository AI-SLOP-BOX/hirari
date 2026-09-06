use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
}
pub fn collect_release_assets(root: &str) -> Option<Vec<ReleaseAsset>> {
    let r = Path::new(root);
    let metadata = fs::symlink_metadata(r).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let mut out = Vec::new();
    collect(r, r, &mut out).ok()?;
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Some(out)
}
fn collect(root: &Path, current: &Path, out: &mut Vec<ReleaseAsset>) -> std::io::Result<()> {
    for entry in fs::read_dir(current)? {
        let p = entry?.path();
        if p.file_name().and_then(|name| name.to_str()) == Some("AURA-RELEASE-MANIFEST.txt") {
            continue;
        }
        let metadata = fs::symlink_metadata(&p)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect(root, &p, out)?;
        } else if metadata.is_file() {
            let mut file = fs::File::open(&p)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 1024 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            let hash = hasher.finalize();
            out.push(ReleaseAsset {
                relative_path: p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/"),
                size: metadata.len(),
                sha256: format!("{hash:x}"),
            });
        }
    }
    Ok(())
}
pub fn release_manifest(root: &str) -> Option<String> {
    let assets = collect_release_assets(root)?;
    let mut out = String::from("AURA-RELEASE-MANIFEST-V1\n");
    for a in assets {
        out.push_str(&format!("{}\t{}\t{}\n", a.sha256, a.size, a.relative_path));
    }
    Some(out)
}
pub fn verify_release_manifest(root: &str, manifest: &str) -> bool {
    let Some(expected) = manifest.strip_prefix("AURA-RELEASE-MANIFEST-V1\n") else {
        return false;
    };
    let Some(actual) = release_manifest(root) else {
        return false;
    };
    actual.strip_prefix("AURA-RELEASE-MANIFEST-V1\n") == Some(expected)
}

/// Validates a manifest without touching the filesystem, rejecting malformed
/// records before a delivery tool consumes them.
pub fn validate_release_manifest(manifest: &str) -> bool {
    let Some(body) = manifest.strip_prefix("AURA-RELEASE-MANIFEST-V1\n") else {
        return false;
    };
    if body.trim().is_empty() {
        return false;
    }
    let mut paths = std::collections::HashSet::new();
    body.lines().all(|line| {
        let mut fields = line.split('\t');
        let (Some(hash), Some(size), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        fields.next().is_none()
            && hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
            && size.parse::<u64>().is_ok()
            && !path.is_empty()
            && !path.contains('\0')
            && !path.starts_with('/')
            && !path.split('/').any(|part| part == "..")
            && paths.insert(path)
    })
}
/// Persist a deterministic release manifest next to a project, refusing to
/// overwrite an existing manifest so a verified delivery cannot be silently
/// replaced.
pub fn write_release_manifest(root: impl AsRef<Path>) -> Option<PathBuf> {
    let root = root.as_ref();
    let root_str = root.to_str()?;
    let manifest = release_manifest(root_str)?;
    let path = root.join("AURA-RELEASE-MANIFEST.txt");
    if path.exists() {
        return None;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;
    if file.write_all(manifest.as_bytes()).is_err() || file.sync_all().is_err() {
        let _ = fs::remove_file(&path);
        return None;
    }
    Some(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn manifest_is_deterministic() {
        let d = std::env::temp_dir().join(format!("aura-release-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        fs::write(d.join("mix.wav"), b"audio").unwrap();
        let m = release_manifest(d.to_str().unwrap()).unwrap();
        assert!(m.contains("mix.wav"));
        assert!(verify_release_manifest(d.to_str().unwrap(), &m));
        let _ = fs::remove_dir_all(d);
    }
}
