//! Import-time cache policy for decoded media.
//!
//! Path validation, file identity (size plus modification time), the hit/miss
//! decision and eviction are owned here, and so is the import itself: the
//! decoder dispatch, the planar build and the weak entry that follows it. The
//! decoded buffers stay native only in name - `AudioBuffer` is a view over the
//! Rust storage, so the host contributes handle lifetimes and nothing else.
//! Every entry point runs on the control thread. Nothing in this module is
//! reachable from the audio callback.

use crate::audio_buffer_ops::{
    hirari_audio_buffer_deinterleave_interleaved, hirari_audio_buffer_storage_array_write,
    hirari_audio_buffer_storage_channels, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_samples,
};
use crate::preview_audio_runtime::{
    hirari_free_decoded_audio, hirari_import_audio_file, NativeDecodedAudio,
};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::num::NonZeroUsize;
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
    unsafe { crate::storage_handle::hirari_audio_import_weak_lock(handle) }
}

fn native_alive(handle: *mut c_void) -> bool {
    unsafe { crate::storage_handle::hirari_audio_import_weak_alive(handle) }
}

fn native_release(handle: *mut c_void) {
    unsafe { crate::storage_handle::hirari_audio_import_weak_release(handle) }
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
    let release = || unsafe { crate::storage_handle::hirari_audio_import_weak_release(weak_audio) };
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

/// Native handle helpers for one decoded import. The buffer is only a view
/// over the Rust audio storage, so the host contributes the handle lifetimes
/// and nothing else: sizing, channel planes and the de-interleave live here.
type AcquireFn = fn(*const u8, usize, *mut f64, *mut *mut c_void) -> i32;
type StoreFn = fn(*const u8, usize, f64, *mut c_void) -> bool;
type CreateFn = fn(*mut *mut c_void) -> *mut c_void;
type StorageFn = fn(*mut c_void) -> *mut c_void;
type ChannelsFn = fn(*mut c_void) -> u32;
type FramesFn = fn(*mut c_void) -> u64;
type WeakFn = fn(*mut c_void) -> *mut c_void;
type BufferReleaseFn = fn(*mut c_void);
type DecodeFn = fn(*const u8, usize) -> NativeDecodedAudio;

#[derive(Clone, Copy)]
pub struct ImportOps {
    pub acquire: AcquireFn,
    pub store: StoreFn,
    pub create: CreateFn,
    pub storage: StorageFn,
    pub channels: ChannelsFn,
    pub frames: FramesFn,
    pub weak: WeakFn,
    pub release: BufferReleaseFn,
    pub decode: DecodeFn,
}

fn native_create(out_storage: *mut *mut c_void) -> *mut c_void {
    unsafe { crate::storage_handle::hirari_audio_import_buffer_create(out_storage) }
}

fn native_storage(audio: *mut c_void) -> *mut c_void {
    unsafe { crate::storage_handle::hirari_audio_import_buffer_storage(audio) }
}

fn native_channels(storage: *mut c_void) -> u32 {
    unsafe { hirari_audio_buffer_storage_channels(storage) }
}

fn native_frames(storage: *mut c_void) -> u64 {
    u64::from(unsafe { hirari_audio_buffer_storage_samples(storage) })
}

fn native_weak(audio: *mut c_void) -> *mut c_void {
    unsafe { crate::storage_handle::hirari_audio_import_buffer_weak(audio) }
}

fn native_buffer_release(audio: *mut c_void) {
    unsafe { crate::storage_handle::hirari_audio_import_buffer_release(audio) }
}

fn native_decode(path: *const u8, path_size: usize) -> NativeDecodedAudio {
    unsafe { hirari_import_audio_file(path, path_size) }
}

fn native_acquire(path: *const u8, path_size: usize, rate: *mut f64, handle: *mut *mut c_void) -> i32 {
    unsafe { hirari_audio_import_cache_acquire(path, path_size, rate, handle) }
}

fn native_store(path: *const u8, path_size: usize, rate: f64, weak: *mut c_void) -> bool {
    unsafe { hirari_audio_import_cache_store(path, path_size, rate, weak) }
}

const NATIVE_IMPORT_OPS: ImportOps = ImportOps {
    acquire: native_acquire,
    store: native_store,
    create: native_create,
    storage: native_storage,
    channels: native_channels,
    frames: native_frames,
    weak: native_weak,
    release: native_buffer_release,
    decode: native_decode,
};

/// Releases the interleaved block a decode returned, on every path.
struct DecodedGuard {
    samples: *mut f32,
    sample_count: usize,
}

impl Drop for DecodedGuard {
    fn drop(&mut self) {
        unsafe { hirari_free_decoded_audio(self.samples, self.sample_count) };
    }
}

fn usable_rate(rate: f64) -> f64 {
    if rate.is_finite() && rate > 0.0 {
        rate
    } else {
        0.0
    }
}

/// Decodes `path`, or reuses the cached buffer, and builds the planar audio
/// the region installs. Returns the owned native handle with its usable
/// sample rate, channel count and frame count; `None` means the media is
/// unusable and the region stays unloaded for Relink.
pub fn import_audio_file(path: &[u8]) -> Option<(*mut c_void, f64, u32, u64)> {
    import_with(&IMPORT_OPS.with(|ops| ops.get()).unwrap_or(NATIVE_IMPORT_OPS), path)
}

/// Test seam: replaces the native decode and buffer-handle callbacks for the
/// calling thread. Clearing the override restores the native callbacks, and
/// other threads keep using them.
pub fn set_import_ops(ops: Option<ImportOps>) {
    IMPORT_OPS.with(|slot| slot.set(ops));
}

thread_local! {
    static IMPORT_OPS: std::cell::Cell<Option<ImportOps>> = const { std::cell::Cell::new(None) };
}

fn import_with(ops: &ImportOps, path: &[u8]) -> Option<(*mut c_void, f64, u32, u64)> {
    let (path_ptr, path_len) = (path.as_ptr(), path.len());
    let mut rate = 0.0;
    let mut handle = std::ptr::null_mut();
    let outcome = (ops.acquire)(path_ptr, path_len, &mut rate, &mut handle);
    if outcome == HIRARI_IMPORT_CACHE_HIT {
        // A hit hands over an owned strong handle. A buffer without samples or
        // channels is not a usable import, so it is released and refused.
        if handle.is_null() {
            return None;
        }
        let storage = (ops.storage)(handle);
        if storage.is_null() {
            (ops.release)(handle);
            return None;
        }
        let (channels, samples) = ((ops.channels)(storage), (ops.frames)(storage));
        if channels == 0 || samples == 0 {
            (ops.release)(handle);
            return None;
        }
        return Some((handle, usable_rate(rate), channels, samples as u64));
    }
    // The cache only reports "needs decode" for a readable file; an empty or
    // missing path is a terminal rejection that must not reach the decoder.
    if outcome != HIRARI_IMPORT_NEEDS_DECODE {
        return None;
    }
    let decoded = (ops.decode)(path_ptr, path_len);
    let _guard = DecodedGuard {
        samples: decoded.samples,
        sample_count: decoded.sample_count,
    };
    if decoded.status != 0 || decoded.samples.is_null() || decoded.channels == 0 {
        return None;
    }
    let Some(frames) = NonZeroUsize::new(decoded.sample_count / decoded.channels as usize) else {
        return None;
    };
    let frames = frames.get() as u32;
    let mut storage = std::ptr::null_mut();
    let handle = (ops.create)(&mut storage);
    if handle.is_null() {
        return None;
    }
    let built = !storage.is_null()
        && unsafe {
            hirari_audio_buffer_storage_reserve_buffer(storage, decoded.channels, frames) == 0
                && hirari_audio_buffer_storage_resize(storage, decoded.channels, frames)
                && hirari_audio_buffer_deinterleave_interleaved(
                    decoded.samples,
                    decoded.sample_count,
                    hirari_audio_buffer_storage_array_write(storage),
                    decoded.channels,
                    frames,
                )
        };
    if !built {
        (ops.release)(handle);
        return None;
    }
    let rate = usable_rate(decoded.sample_rate);
    // The cache holds only a weak reference; the region owns the buffer. A
    // refused entry has already released the handle it was given.
    let weak = (ops.weak)(handle);
    if !weak.is_null() {
        (ops.store)(path_ptr, path_len, rate, weak);
    }
    Some((handle, rate, decoded.channels, frames as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

    const SEQ: Ordering = Ordering::SeqCst;

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

    // --- import orchestration -------------------------------------------------
    //
    // The storages are real Rust audio storage, so the fixtures only replace the
    // host handle helpers: acquire verdict, decoder result and cache offer.

    use crate::audio_buffer_ops::{
        hirari_audio_buffer_storage_create, hirari_audio_buffer_storage_destroy,
        hirari_audio_buffer_storage_read_pointer, hirari_audio_buffer_storage_resize,
    };

    static IMPORT_LOCK: Mutex<()> = Mutex::new(());
    static ACQUIRE_MODE: AtomicUsize = AtomicUsize::new(0);
    static HIT_CHANNELS: AtomicU32 = AtomicU32::new(2);
    static HIT_FRAMES: AtomicUsize = AtomicUsize::new(8);
    static HIT_RATE_BITS: AtomicU64 = AtomicU64::new(48_000f64.to_bits());
    static DECODE_STATUS: AtomicU32 = AtomicU32::new(0);
    static DECODE_CHANNELS: AtomicU32 = AtomicU32::new(2);
    static DECODE_FRAMES: AtomicUsize = AtomicUsize::new(4);
    static DECODE_RATE_BITS: AtomicU64 = AtomicU64::new(44_100f64.to_bits());
    static DECODES: AtomicUsize = AtomicUsize::new(0);
    static RELEASES: AtomicUsize = AtomicUsize::new(0);
    static STORED: AtomicUsize = AtomicUsize::new(0);

    fn reset_import(decode_status: u32, frames: usize) {
        ACQUIRE_MODE.store(0, SEQ);
        HIT_CHANNELS.store(2, SEQ);
        HIT_FRAMES.store(frames, SEQ);
        HIT_RATE_BITS.store(48_000f64.to_bits(), SEQ);
        DECODE_STATUS.store(decode_status, SEQ);
        DECODE_CHANNELS.store(2, SEQ);
        DECODE_FRAMES.store(frames, SEQ);
        DECODE_RATE_BITS.store(44_100f64.to_bits(), SEQ);
        DECODES.store(0, SEQ);
        RELEASES.store(0, SEQ);
        STORED.store(0, SEQ);
    }

    fn test_acquire(_path: *const u8, _length: usize, rate: *mut f64, out: *mut *mut c_void) -> i32 {
        match ACQUIRE_MODE.load(SEQ) {
            0 => {
                let storage = unsafe { hirari_audio_buffer_storage_create() };
                let frames = HIT_FRAMES.load(SEQ) as u32;
                unsafe {
                    hirari_audio_buffer_storage_reserve_buffer(storage, HIT_CHANNELS.load(SEQ), frames);
                    hirari_audio_buffer_storage_resize(storage, HIT_CHANNELS.load(SEQ), frames)
                };
                unsafe {
                    *rate = f64::from_bits(HIT_RATE_BITS.load(SEQ));
                    *out = storage;
                }
                HIRARI_IMPORT_CACHE_HIT
            }
            1 => HIRARI_IMPORT_NEEDS_DECODE,
            2 => HIRARI_IMPORT_FILE_NOT_FOUND,
            _ => HIRARI_IMPORT_EMPTY_PATH,
        }
    }

    /// An interleaved block of `(frame * 10 + channel)` samples, boxed the same
    /// way the decoder hands one over so the guard can release it.
    fn test_decode(_path: *const u8, _length: usize) -> NativeDecodedAudio {
        DECODES.fetch_add(1, SEQ);
        let channels = DECODE_CHANNELS.load(SEQ) as usize;
        let frames = DECODE_FRAMES.load(SEQ);
        let mut block = Vec::with_capacity(channels * frames);
        for frame in 0..frames {
            for channel in 0..channels {
                block.push((frame * 10 + channel) as f32);
            }
        }
        let mut block = block.into_boxed_slice();
        let samples = block.as_mut_ptr();
        let sample_count = block.len();
        std::mem::forget(block);
        NativeDecodedAudio {
            samples,
            sample_count,
            channels: channels as u32,
            sample_rate: f64::from_bits(DECODE_RATE_BITS.load(SEQ)),
            bits_per_sample: 24,
            status: DECODE_STATUS.load(SEQ),
        }
    }

    /// Mirrors the native helper: the handle it returns publishes the storage
    /// the import writes into.
    fn test_create(out_storage: *mut *mut c_void) -> *mut c_void {
        let storage = unsafe { hirari_audio_buffer_storage_create() };
        if !out_storage.is_null() {
            unsafe { *out_storage = storage };
        }
        storage
    }

    fn test_import_ops() -> ImportOps {
        ImportOps {
            acquire: test_acquire,
            store: |_, _, _, weak| {
                STORED.store(weak as usize, SEQ);
                true
            },
            create: test_create,
            storage: |handle| handle,
            channels: |storage| unsafe { hirari_audio_buffer_storage_channels(storage) },
            frames: |storage| u64::from(unsafe { hirari_audio_buffer_storage_samples(storage) }),
            weak: |handle| handle,
            release: |handle| {
                RELEASES.fetch_add(1, SEQ);
                unsafe { hirari_audio_buffer_storage_destroy(handle) };
            },
            decode: test_decode,
        }
    }

    #[test]
    fn import_hands_over_a_hit_and_refuses_an_unusable_one() {
        let _guard = IMPORT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let ops = test_import_ops();

        reset_import(0, 8);
        let (handle, rate, channels, frames) =
            import_with(&ops, b"cached.wav").expect("a hit is an import");
        assert_eq!(rate, 48_000.0);
        assert_eq!((channels, frames), (2, 8));
        assert_eq!(RELEASES.load(SEQ), 0);
        assert_eq!(DECODES.load(SEQ), 0);
        unsafe { hirari_audio_buffer_storage_destroy(handle) };

        // A cached entry with no samples is released back, not imported.
        reset_import(0, 0);
        assert!(import_with(&ops, b"cached.wav").is_none());
        assert_eq!(RELEASES.load(SEQ), 1);

        // An empty rate is not usable, but the buffer still is.
        reset_import(0, 4);
        HIT_RATE_BITS.store(f64::NAN.to_bits(), SEQ);
        let (handle, rate, _, _) = import_with(&ops, b"cached.wav").unwrap();
        assert_eq!(rate, 0.0);
        unsafe { hirari_audio_buffer_storage_destroy(handle) };
    }

    #[test]
    fn import_only_decodes_when_the_cache_asks_for_it() {
        let _guard = IMPORT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let ops = test_import_ops();

        // Mode 2 is a missing file and mode 3 an unrepresentable path; neither
        // is a decode request.
        for mode in [2usize, 3] {
            reset_import(0, 4);
            ACQUIRE_MODE.store(mode, SEQ);
            assert!(import_with(&ops, b"gone.wav").is_none());
            assert_eq!(DECODES.load(SEQ), 0);
            assert_eq!(RELEASES.load(SEQ), 0);
        }
    }

    #[test]
    fn import_rejects_a_failed_or_empty_decode() {
        let _guard = IMPORT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let ops = test_import_ops();

        reset_import(3, 4);
        ACQUIRE_MODE.store(1, SEQ);
        assert!(import_with(&ops, b"broken.wav").is_none());
        assert_eq!(DECODES.load(SEQ), 1);
        assert_eq!(RELEASES.load(SEQ), 0);

        // A zero-length decode never reaches the buffer at all.
        reset_import(0, 0);
        ACQUIRE_MODE.store(1, SEQ);
        assert!(import_with(&ops, b"empty.wav").is_none());
        assert_eq!(RELEASES.load(SEQ), 0);
    }

    #[test]
    fn import_builds_planar_channels_and_offers_the_buffer_to_the_cache() {
        let _guard = IMPORT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let ops = test_import_ops();
        reset_import(0, 4);
        ACQUIRE_MODE.store(1, SEQ);

        let (handle, rate, channels, frames) =
            import_with(&ops, b"fresh.wav").expect("a decode is an import");
        assert_eq!(rate, 44_100.0);
        assert_eq!((channels, frames), (2, 4));
        let read = |channel: u32, offset: u32| unsafe {
            *hirari_audio_buffer_storage_read_pointer(handle, channel, offset)
        };
        assert_eq!([read(0, 0), read(0, 1), read(0, 3)], [0.0, 10.0, 30.0]);
        assert_eq!([read(1, 0), read(1, 2)], [1.0, 21.0]);
        // The cache is offered the same buffer the region now owns.
        assert_eq!(STORED.load(SEQ), handle as usize);
        assert_eq!(RELEASES.load(SEQ), 0);
        unsafe { hirari_audio_buffer_storage_destroy(handle) };
    }
}
