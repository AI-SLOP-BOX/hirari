//! Stable content fingerprints for plug-in files and bundles.
//!
//! This is a cache identity, not a security signature. Admission policy stays
//! with the plug-in host; this routine rejects symlink entries and hashes
//! canonical relative paths, entry kinds, file lengths, and file bytes.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const FNV_OFFSET: u64 = 1_469_598_103_934_665_603;
const FNV_PRIME: u64 = 1_099_511_628_211;

pub fn fingerprint_path(path: &Path) -> Option<u64> {
    let root = fs::canonicalize(path).ok()?;
    let root_metadata = fs::symlink_metadata(&root).ok()?;
    if root_metadata.file_type().is_symlink() {
        return None;
    }

    let mut hash = FNV_OFFSET;
    mix_entry(&mut hash, &root, &root_metadata)?;
    if root_metadata.is_file() {
        return Some(hash);
    }
    if !root_metadata.is_dir() {
        return None;
    }

    let mut entries = Vec::new();
    collect_entries(&root, &mut entries)?;
    entries.sort_by(|left, right| path_key(left).cmp(&path_key(right)));
    for entry in entries {
        let metadata = fs::symlink_metadata(&entry).ok()?;
        mix_entry(&mut hash, &entry, &metadata)?;
    }
    Some(hash)
}

fn collect_entries(directory: &Path, entries: &mut Vec<PathBuf>) -> Option<()> {
    let children = fs::read_dir(directory).ok()?;
    for child in children {
        let child = match child {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => continue,
            Err(_) => return None,
        };
        let path = child.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => continue,
            Err(_) => return None,
        };
        if metadata.file_type().is_symlink() {
            entries.push(path);
            continue;
        }
        let is_directory = metadata.is_dir();
        entries.push(path.clone());
        if is_directory {
            collect_entries(&path, entries)?;
        }
    }
    Some(())
}

fn mix_entry(hash: &mut u64, path: &Path, metadata: &fs::Metadata) -> Option<()> {
    if metadata.file_type().is_symlink() {
        return None;
    }
    mix_string(hash, &path_key(path));
    if metadata.is_file() {
        // Match std::filesystem::file_type::regular and preserve the existing
        // native cache fingerprint layout on supported desktop platforms.
        mix_u64(hash, 1);
        mix_u64(hash, metadata.len());
        let mut file = File::open(path).ok()?;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut chunk).ok()?;
            if count == 0 {
                break;
            }
            for byte in &chunk[..count] {
                mix_byte(hash, *byte);
            }
        }
    } else if metadata.is_dir() {
        // std::filesystem::file_type::directory on supported desktop STL.
        mix_u64(hash, 2);
        mix_u64(hash, 0xd1);
    } else {
        return None;
    }
    Some(())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn mix_string(hash: &mut u64, value: &str) {
    for byte in value.as_bytes() {
        mix_byte(hash, *byte);
    }
    mix_u64(hash, 0xff);
}

fn mix_u64(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        mix_byte(hash, byte);
    }
}

fn mix_byte(hash: &mut u64, byte: u8) {
    *hash ^= u64::from(byte);
    *hash = hash.wrapping_mul(FNV_PRIME);
}

/// C ABI for native plug-in admission and cache callers. Returns success
/// separately so every `u64` fingerprint value remains representable.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_fingerprint(
    path: *const std::ffi::c_char,
    path_size: usize,
    output: *mut u64,
) -> u8 {
    if path.is_null() || output.is_null() || path_size == 0 {
        return 0;
    }
    let bytes = std::slice::from_raw_parts(path.cast::<u8>(), path_size);
    let Some(path) = path_from_bytes(bytes) else {
        return 0;
    };
    let Some(fingerprint) = fingerprint_path(&path) else {
        return 0;
    };
    *output = fingerprint;
    1
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    Some(PathBuf::from(std::str::from_utf8(bytes).ok()?))
}
