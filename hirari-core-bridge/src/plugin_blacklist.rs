//! Rust-owned, fingerprint-aware blacklist for plug-in scan failures.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy)]
struct Entry {
    reason: u32,
    fingerprint: Option<u64>,
}

struct Blacklist {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl Blacklist {
    fn open(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter_map(parse_record)
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    fn persist(&self, entries: &BTreeMap<String, Entry>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let temporary = self.path.with_extension(format!(
            "blacklist-{}-{nonce}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut bytes = Vec::new();
        for (path, entry) in entries {
            bytes.push(b'"');
            for byte in path.bytes() {
                if byte == b'"' || byte == b'\\' {
                    bytes.push(b'\\');
                }
                bytes.push(byte);
            }
            bytes.extend_from_slice(format!("\" {}", entry.reason).as_bytes());
            if let Some(fingerprint) = entry.fingerprint {
                bytes.extend_from_slice(format!(" {fingerprint}").as_bytes());
            }
            bytes.push(b'\n');
        }
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            #[cfg(unix)]
            if let Some(parent) = self.path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
            Ok::<(), std::io::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

fn parse_record(line: &str) -> Option<(String, Entry)> {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    let mut path = Vec::new();
    let mut closed = false;
    while let Some(&byte) = bytes.get(cursor) {
        cursor += 1;
        match byte {
            b'"' => {
                closed = true;
                break;
            }
            b'\\' => path.push(*bytes.get(cursor)?),
            other => path.push(other),
        }
        if byte == b'\\' {
            cursor += 1;
        }
    }
    if !closed {
        return None;
    }
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let reason_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let reason = std::str::from_utf8(bytes.get(reason_start..cursor)?)
        .ok()?
        .parse::<u32>()
        .ok()?;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let fingerprint_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let fingerprint = (cursor > fingerprint_start)
        .then(|| {
            std::str::from_utf8(&bytes[fingerprint_start..cursor])
                .ok()?
                .parse()
                .ok()
        })
        .flatten();
    let path = String::from_utf8(path).ok()?;
    if path.len() > 4096 || path.contains('\0') {
        return None;
    }
    Some((
        cache_key(&path),
        Entry {
            reason,
            fingerprint,
        },
    ))
}

fn cache_key(value: &str) -> String {
    let path = PathBuf::from(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_or(path.clone(), |cwd| cwd.join(path))
    };
    let mut existing = absolute.clone();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        suffix.push(name.to_os_string());
        if !existing.pop() {
            break;
        }
    }
    let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

// ---- process-wide scan cache -------------------------------------------------
//
// The plug-in host used to spell "key + fingerprint + blacklist record" out in
// C++ for every admission decision. The whole rule lives here now: callers hand
// over the on-disk path and this facade resolves the canonical key and content
// fingerprint before touching the shared record.

static SCAN_CACHE: std::sync::OnceLock<Option<Blacklist>> = std::sync::OnceLock::new();

fn scan_cache() -> Option<&'static Blacklist> {
    SCAN_CACHE
        .get_or_init(|| {
            let mut path = crate::plugin_cache_path::cache_file_path()?
                .clone()
                .into_os_string();
            path.push(".blacklist");
            Some(Blacklist::open(PathBuf::from(path)))
        })
        .as_ref()
}

fn valid_path(path: *const u8, path_size: usize) -> Option<String> {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 {
        return None;
    }
    let text = std::str::from_utf8(unsafe { std::slice::from_raw_parts(path, path_size) }).ok()?;
    (!text.contains('\0')).then(|| text.to_owned())
}

fn key_fingerprint(path: &str) -> (String, Option<u64>) {
    let key = cache_key(path);
    let fingerprint = crate::plugin_fingerprint::fingerprint_path(std::path::Path::new(&key));
    (key, fingerprint)
}

fn blocked(state: &Blacklist, key: &str, fingerprint: Option<u64>) -> bool {
    let Ok(entries) = state.entries.lock() else {
        return false;
    };
    let Some(entry) = entries.get(key) else {
        return false;
    };
    match entry.fingerprint {
        None => true,
        Some(saved) => fingerprint == Some(saved),
    }
}

/// Admission gate: an empty path is never scanned, and a path whose current
/// content fingerprint still matches a recorded failure stays blocked. A
/// replaced plug-in binary therefore becomes scannable again.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_scan_cache_should_scan(
    path: *const u8,
    path_size: usize,
) -> bool {
    let Some(path) = (unsafe { valid_path(path, path_size) }) else {
        return false;
    };
    let Some(state) = scan_cache() else {
        return true;
    };
    let (key, fingerprint) = key_fingerprint(&path);
    !key.is_empty() && !blocked(state, &key, fingerprint)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_scan_cache_record_failure(
    path: *const u8,
    path_size: usize,
    reason: u32,
) -> bool {
    let Some(path) = (unsafe { valid_path(path, path_size) }) else {
        return false;
    };
    let Some(state) = scan_cache() else {
        return false;
    };
    let (key, _) = key_fingerprint(&path);
    if key.is_empty() {
        return false;
    }
    // The record stores the fingerprint of the path the host actually probed,
    // matching the native scan, which fingerprints the file it just loaded.
    let fingerprint = crate::plugin_fingerprint::fingerprint_path(std::path::Path::new(&path));
    let Ok(mut entries) = state.entries.lock() else {
        return false;
    };
    entries.insert(
        key,
        Entry {
            reason,
            fingerprint,
        },
    );
    state.persist(&entries).is_ok()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_scan_cache_record_success(
    path: *const u8,
    path_size: usize,
) -> bool {
    let Some(path) = (unsafe { valid_path(path, path_size) }) else {
        return false;
    };
    let Some(state) = scan_cache() else {
        return false;
    };
    let key = cache_key(&path);
    let Ok(mut entries) = state.entries.lock() else {
        return false;
    };
    entries.remove(&key);
    state.persist(&entries).is_ok()
}

#[cfg(test)]
mod scan_cache_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hirari-scan-cache-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn recorded_failure_blocks_until_the_plugin_changes() {
        let dir = scratch("failure");
        let plugin = dir.join("plugin.clap");
        std::fs::write(&plugin, b"plugin-v1").expect("write plugin");
        let text = plugin.to_string_lossy().to_string();

        let (key, fingerprint) = key_fingerprint(&text);
        let fingerprint = fingerprint.expect("fingerprint");
        let store = Blacklist::open(dir.join("blacklist"));
        assert!(!blocked(&store, &key, Some(fingerprint)));

        entries_with(&store, &key, fingerprint, 17);
        assert!(blocked(&store, &key, Some(fingerprint)));
        // The record survives a reopen, which is the persisted cache contract.
        let reopened = Blacklist::open(dir.join("blacklist"));
        assert!(blocked(&reopened, &key, Some(fingerprint)));

        // A "/./" alias canonicalises to the same key.
        let alias = dir.join(".").join("plugin.clap");
        assert_eq!(cache_key(&alias.to_string_lossy()), key);

        // Replacing the binary invalidates the recorded fingerprint.
        std::fs::write(&plugin, b"plugin-v2-with-new-content").expect("replace plugin");
        let (_, replaced) = key_fingerprint(&text);
        assert_ne!(replaced, Some(fingerprint));
        assert!(!blocked(&store, &key, replaced));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_path_is_never_admitted() {
        assert!(unsafe { hirari_plugin_scan_cache_should_scan(std::ptr::null(), 0) } == false);
        let text = b"";
        assert!(!unsafe { hirari_plugin_scan_cache_should_scan(text.as_ptr(), text.len()) });
    }

    fn entries_with(store: &Blacklist, key: &str, fingerprint: u64, reason: u32) {
        let mut entries = store.entries.lock().expect("entries");
        entries.insert(
            key.to_owned(),
            Entry {
                reason,
                fingerprint: Some(fingerprint),
            },
        );
        store.persist(&entries).expect("persist");
    }
}
