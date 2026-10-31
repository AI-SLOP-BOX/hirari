//! Import-time cache policy for decoded media.
//!
//! Path validation, file identity (size plus modification time), the hit/miss
//! decision and eviction are owned here. The decoded buffers themselves stay
//! native: the cache only holds an opaque weak handle to each `AudioBuffer`
//! and asks the host to lock it, so a hit still returns the very same buffer
//! the region already holds.
//!
//! Every entry point runs on the control thread. Nothing in this module is
//! reachable from the audio callback.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

/// The path was empty, embedded a NUL byte, or was not representable.
pub const HIRARI_IMPORT_EMPTY_PATH: i32 = 1;
/// The path did not resolve to a readable file.
pub const HIRARI_IMPORT_FILE_NOT_FOUND: i32 = 2;
/// A live buffer was found; the audio handle and sample rate were returned.
pub const HIRARI_IMPORT_CACHE_HIT: i32 = 0;
/// No usable cache entry exists; the caller decodes the file.
pub const HIRARI_IMPORT_NEEDS_DECODE: i32 = 3;

const MAX_CACHED_IMPORTS: usize = 1024;
const MAX_PATH_BYTES: usize = 1024 * 1024;

type LockFn = fn(*mut c_void) -> *mut c_void;
type AliveFn = fn(*mut c_void) -> bool;
type ReleaseFn = fn(*mut c_void);

#[derive(Clone, Copy)]
struct WeakOps {
    lock: LockFn,
    alive: AliveFn,
    release: ReleaseFn,
}

const NATIVE_OPS: WeakOps = WeakOps {
    lock: native_lock,
    alive: native_alive,
    release: native_release,
};

fn native_lock(handle: *mut c_void) -> *mut c_void {
    unsafe { hirari_audio_import_weak_lock(handle) }
}

fn native_alive(handle: *mut c_void) -> bool {
    unsafe { hirari_audio_import_weak_alive(handle) }
}

fn native_release(handle: *mut c_void) {
    unsafe { hirari_audio_import_weak_release(handle) }
}

extern "C" {
    fn hirari_audio_import_weak_lock(handle: *mut c_void) -> *mut c_void;
    fn hirari_audio_import_weak_alive(handle: *mut c_void) -> bool;
    fn hirari_audio_import_weak_release(handle: *mut c_void);
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    size: u64,
    modified: SystemTime,
}

struct CachedImport {
    identity: FileIdentity,
    sample_rate: f64,
    weak_audio: usize,
}

#[derive(Clone, Copy)]
struct FileStat {
    size: u64,
    modified: Option<SystemTime>,
}

impl FileStat {
    fn identity(self) -> Option<FileIdentity> {
        self.modified.map(|modified| FileIdentity {
            size: self.size,
            modified,
        })
    }
}

static CACHE: OnceLock<Mutex<BTreeMap<String, CachedImport>>> = OnceLock::new();

fn cache() -> &'static Mutex<BTreeMap<String, CachedImport>> {
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn path_text(path: *const u8, path_size: usize) -> Option<String> {
    if path.is_null() || path_size == 0 || path_size > MAX_PATH_BYTES {
        return None;
    }
    // SAFETY: the caller guarantees `path` holds `path_size` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

fn file_stat(path: &str) -> Option<FileStat> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileStat {
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// Returns the locked audio handle and cached sample rate for a matching,
/// still-live entry. A dead weak handle is dropped so a crash-free reimport
/// simply decodes again.
fn lookup(
    entries: &mut BTreeMap<String, CachedImport>,
    path: &str,
    identity: FileIdentity,
    ops: WeakOps,
) -> Option<(f64, usize)> {
    let (stored_identity, sample_rate, weak_audio) = {
        let entry = entries.get(path)?;
        (entry.identity, entry.sample_rate, entry.weak_audio)
    };
    if stored_identity != identity {
        return None;
    }
    let strong = (ops.lock)(weak_audio as *mut c_void);
    if strong.is_null() {
        if let Some(stale) = entries.remove(path) {
            (ops.release)(stale.weak_audio as *mut c_void);
        }
        return None;
    }
    Some((sample_rate, strong as usize))
}

/// Stores one entry, replacing any previous record for the same path, then
/// releases entries whose buffer died and enforces the size cap in key order.
fn insert(
    entries: &mut BTreeMap<String, CachedImport>,
    path: String,
    identity: FileIdentity,
    sample_rate: f64,
    weak_audio: usize,
    ops: WeakOps,
) {
    let replaced = entries.insert(
        path,
        CachedImport {
            identity,
            sample_rate,
            weak_audio,
        },
    );
    if let Some(previous) = replaced {
        (ops.release)(previous.weak_audio as *mut c_void);
    }
    let dead: Vec<String> = entries
        .iter()
        .filter(|(_, entry)| !(ops.alive)(entry.weak_audio as *mut c_void))
        .map(|(key, _)| key.clone())
        .collect();
    for key in dead {
        if let Some(entry) = entries.remove(&key) {
            (ops.release)(entry.weak_audio as *mut c_void);
        }
    }
    while entries.len() > MAX_CACHED_IMPORTS {
        let Some(key) = entries.keys().next().cloned() else {
            break;
        };
        if let Some(entry) = entries.remove(&key) {
            (ops.release)(entry.weak_audio as *mut c_void);
        }
    }
}

/// Probes the import cache for `path`.
///
/// Returns a status code: [HIRARI_IMPORT_CACHE_HIT] fills `audio_handle` with
/// an owned strong handle the caller must delete, and `sample_rate` when
/// non-null; [HIRARI_IMPORT_NEEDS_DECODE] means the caller should decode and
/// then call [hirari_audio_import_cache_store]; the remaining codes are
/// terminal rejections that need no decode.
///
/// # Safety
/// `path` must hold `path_size` bytes when non-null, and the out pointers must
/// be valid or null.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_import_cache_acquire(
    path: *const u8,
    path_size: usize,
    sample_rate: *mut f64,
    audio_handle: *mut *mut c_void,
) -> i32 {
    let Some(text) = path_text(path, path_size) else {
        return HIRARI_IMPORT_EMPTY_PATH;
    };
    let Some(stat) = file_stat(&text) else {
        return HIRARI_IMPORT_FILE_NOT_FOUND;
    };
    if audio_handle.is_null() {
        return HIRARI_IMPORT_NEEDS_DECODE;
    }
    let Some(identity) = stat.identity() else {
        // Without a modification time the entry could never be validated, so
        // the file is decoded but not cached (matching the previous policy).
        return HIRARI_IMPORT_NEEDS_DECODE;
    };
    let mut entries = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((rate, strong)) = lookup(&mut entries, &text, identity, NATIVE_OPS) {
        unsafe {
            *audio_handle = strong as *mut c_void;
            if !sample_rate.is_null() {
                *sample_rate = rate;
            }
        }
        return HIRARI_IMPORT_CACHE_HIT;
    }
    HIRARI_IMPORT_NEEDS_DECODE
}

/// Records a freshly decoded buffer for `path`, taking ownership of
/// `weak_audio` on every path. Returns false when the entry was refused; the
/// handle is already released in that case.
///
/// # Safety
/// `path` must hold `path_size` bytes when non-null and `weak_audio` must be a
/// host-created weak handle not owned by anyone else.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_import_cache_store(
    path: *const u8,
    path_size: usize,
    sample_rate: f64,
    weak_audio: *mut c_void,
) -> bool {
    if weak_audio.is_null() {
        return false;
    }
    let release = || unsafe { hirari_audio_import_weak_release(weak_audio) };
    let Some(text) = path_text(path, path_size) else {
        release();
        return false;
    };
    let Some(identity) = file_stat(&text).and_then(FileStat::identity) else {
        release();
        return false;
    };
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        release();
        return false;
    }
    let mut entries = cache().lock().unwrap_or_else(PoisonError::into_inner);
    insert(
        &mut entries,
        text,
        identity,
        sample_rate,
        weak_audio as usize,
        NATIVE_OPS,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_lock(handle: *mut c_void) -> *mut c_void {
        if test_alive(handle) {
            handle
        } else {
            std::ptr::null_mut()
        }
    }

    fn test_alive(handle: *mut c_void) -> bool {
        handle as usize % 2 == 0
    }

    fn test_release(_handle: *mut c_void) {}

    const TEST_OPS: WeakOps = WeakOps {
        lock: test_lock,
        alive: test_alive,
        release: test_release,
    };

    fn identity(size: u64, seconds: u64) -> FileIdentity {
        FileIdentity {
            size,
            modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
        }
    }

    #[test]
    fn path_validation_rejects_empty_oversized_and_nul_paths() {
        assert_eq!(path_text(std::ptr::null(), 0), None);
        assert_eq!(path_text(b"a\0b".as_ptr(), 3), None);
        assert_eq!(path_text(std::ptr::null(), 10), None);
        assert_eq!(path_text(b"audio.wav".as_ptr(), 9).as_deref(), Some("audio.wav"));
    }

    #[test]
    fn lookup_requires_a_matching_identity() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "a.wav".to_string(),
            CachedImport {
                identity: identity(10, 1),
                sample_rate: 48_000.0,
                weak_audio: 2,
            },
        );
        assert!(lookup(&mut entries, "a.wav", identity(10, 2), TEST_OPS).is_none());
        assert_eq!(
            lookup(&mut entries, "a.wav", identity(10, 1), TEST_OPS),
            Some((48_000.0, 2))
        );
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn lookup_drops_a_dead_buffer() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "a.wav".to_string(),
            CachedImport {
                identity: identity(10, 1),
                sample_rate: 44_100.0,
                weak_audio: 3,
            },
        );
        assert!(lookup(&mut entries, "a.wav", identity(10, 1), TEST_OPS).is_none());
        assert!(entries.is_empty());
    }

    #[test]
    fn insert_purges_dead_entries_and_caps_the_table() {
        let mut entries = BTreeMap::new();
        insert(
            &mut entries,
            "dead.wav".to_string(),
            identity(1, 1),
            44_100.0,
            3,
            TEST_OPS,
        );
        assert!(entries.is_empty());

        for index in 0..=MAX_CACHED_IMPORTS {
            insert(
                &mut entries,
                format!("{index:05}.wav"),
                identity(1, 1),
                44_100.0,
                2 * (index + 1),
                TEST_OPS,
            );
        }
        assert_eq!(entries.len(), MAX_CACHED_IMPORTS);
        assert!(!entries.contains_key("00000.wav"));
        assert!(entries.contains_key("01024.wav"));
    }
}
