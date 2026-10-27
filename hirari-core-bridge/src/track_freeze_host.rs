//! Track freeze admission policy: validation, sample limits, project-end
//! sizing, path resolution and the order in which the engine is quiesced.
//!
//! The engine keeps the track vector and `TrackFreezeManager` keeps the render,
//! the published snapshot and the file artifact. This module decides whether a
//! request is admissible, which size it renders at, and which path is written.
//!
//! Every entry point runs on the control thread.

use std::ffi::{c_void, OsStr};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Mirrors `TrackFreezeManager::kMaxFreezeSamples` (two float channels).
pub const MAX_FREEZE_SAMPLES: u64 = 64 * 1024 * 1024;

/// Pins a track for the duration of one freeze operation. `wait` drains the
/// audio callback before the track is looked up.
pub type AcquireTrackFn = unsafe extern "C" fn(*mut c_void, u32, bool) -> *mut c_void;
/// Releases the pin returned by `acquire`.
pub type ReleaseTrackFn = unsafe extern "C" fn(*mut c_void);
/// Renders and publishes an in-memory freeze snapshot.
pub type FreezeFn = unsafe extern "C" fn(*mut c_void, u64, u32) -> bool;
/// Renders a disk-backed artifact at an absolute path, or restores one when
/// `persist` is false.
pub type RunFromFileFn = unsafe extern "C" fn(*mut c_void, *const u8, usize, u64, u32, bool) -> bool;
/// Sets the cache path recorded on a track.
pub type SetCachePathFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool;
/// Drops the frozen snapshot.
pub type UnfreezeFn = unsafe extern "C" fn(*mut c_void);
pub type IsFrozenFn = unsafe extern "C" fn(*mut c_void) -> bool;
/// Computes a pinned track's last region end; false when a region is malformed.
pub type ProjectEndFn = unsafe extern "C" fn(*mut c_void, *mut u64) -> bool;
/// Current engine block size, used as the minimum freeze length.
pub type BlockSizeFn = unsafe extern "C" fn(*mut c_void) -> u32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariTrackFreezeHostOps {
    pub acquire: Option<AcquireTrackFn>,
    pub release: Option<ReleaseTrackFn>,
    pub freeze: Option<FreezeFn>,
    pub run_from_file: Option<RunFromFileFn>,
    pub set_cache_path: Option<SetCachePathFn>,
    pub unfreeze: Option<UnfreezeFn>,
    pub is_frozen: Option<IsFrozenFn>,
    pub project_end: Option<ProjectEndFn>,
    pub block_size: Option<BlockSizeFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariTrackFreezeHostOps {}
unsafe impl Sync for HirariTrackFreezeHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariTrackFreezeHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariTrackFreezeHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_host_install(ops: *const HirariTrackFreezeHostOps) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { std::ptr::read(ops) })
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

/// Releases the pin when the freeze operation ends, even on an early return.
struct TrackPin {
    raw: *mut c_void,
    release: ReleaseTrackFn,
}

impl Drop for TrackPin {
    fn drop(&mut self) {
        unsafe { (self.release)(self.raw) };
    }
}

fn acquire(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    wait: bool,
) -> Option<TrackPin> {
    let acquire = ops.acquire?;
    let release = ops.release?;
    let raw = unsafe { acquire(host, track_id, wait) };
    if raw.is_null() {
        return None;
    }
    Some(TrackPin { raw, release })
}

fn freeze_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    if total_samples == 0 || sample_rate == 0 || total_samples > MAX_FREEZE_SAMPLES {
        return false;
    }
    let Some(freeze) = ops.freeze else {
        return false;
    };
    // A freeze render mutates processor state while producing its immutable
    // snapshot, so it must never overlap the live callback or another graph
    // hydration.
    let Some(pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    unsafe { freeze(pin.raw, total_samples, sample_rate) }
}

/// Resolves a caller-supplied path the way `std::filesystem::absolute` did:
/// relative paths get the current directory prepended, nothing is canonicalised.
fn absolute_path(path: &[u8]) -> Option<Vec<u8>> {
    let resolved = std::path::absolute(Path::new(OsStr::from_bytes(path))).ok()?;
    if resolved.as_os_str().is_empty() {
        return None;
    }
    Some(resolved.into_os_string().into_vec())
}

fn freeze_to_project_end_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    sample_rate: u32,
) -> bool {
    if sample_rate == 0 {
        return false;
    }
    let (Some(project_end), Some(block_size)) = (ops.project_end, ops.block_size) else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id, false) else {
        return false;
    };
    let mut project_end_samples = 0_u64;
    let readable = unsafe { project_end(pin.raw, &mut project_end_samples) };
    drop(pin);
    if !readable {
        return false;
    }
    // The end of the last region wins, but a track with no regions still needs
    // a full block round trip so the frozen snapshot is not empty.
    let fallback = u64::from(unsafe { block_size(host) }.max(1)) * 2;
    let total_samples = project_end_samples.max(fallback);
    if total_samples > MAX_FREEZE_SAMPLES {
        return false;
    }
    // Re-enter the regular freeze path so validation and the callback drain
    // happen exactly once, right before the render.
    freeze_impl(ops, host, track_id, total_samples, sample_rate)
}

fn freeze_to_file_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    if path.is_empty()
        || total_samples == 0
        || sample_rate == 0
        || total_samples > MAX_FREEZE_SAMPLES
    {
        return false;
    }
    let Some(run) = ops.run_from_file else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    let Some(absolute) = absolute_path(path) else {
        return false;
    };
    unsafe { run(pin.raw, absolute.as_ptr(), absolute.len(), total_samples, sample_rate, true) }
}

fn restore_from_file_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    if path.is_empty() || total_samples == 0 || sample_rate == 0 {
        return false;
    }
    let Some(run) = ops.run_from_file else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    unsafe { run(pin.raw, path.as_ptr(), path.len(), total_samples, sample_rate, false) }
}

fn set_cache_path_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
) -> bool {
    if path.is_empty() {
        return false;
    }
    let Some(set) = ops.set_cache_path else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id, false) else {
        return false;
    };
    unsafe { set(pin.raw, path.as_ptr(), path.len()) }
}

fn unfreeze_impl(ops: &HirariTrackFreezeHostOps, host: *mut c_void, track_id: u32) -> bool {
    let Some(unfreeze) = ops.unfreeze else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    unsafe { unfreeze(pin.raw) };
    true
}

fn is_frozen_impl(ops: &HirariTrackFreezeHostOps, host: *mut c_void, track_id: u32) -> bool {
    let Some(is_frozen) = ops.is_frozen else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id, false) else {
        return false;
    };
    unsafe { is_frozen(pin.raw) }
}

fn path_slice<'a>(path: *const u8, length: usize) -> &'a [u8] {
    if path.is_null() || length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(path, length) }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze(
    host: *mut c_void,
    track_id: u32,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => freeze_impl(&ops, host, track_id, total_samples, sample_rate),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_to_project_end(
    host: *mut c_void,
    track_id: u32,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => freeze_to_project_end_impl(&ops, host, track_id, sample_rate),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_to_file(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => freeze_to_file_impl(
            &ops,
            host,
            track_id,
            path_slice(path, path_length),
            total_samples,
            sample_rate,
        ),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_restore_freeze(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => restore_from_file_impl(
            &ops,
            host,
            track_id,
            path_slice(path, path_length),
            total_samples,
            sample_rate,
        ),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_freeze_cache_path(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    match host_ops() {
        Some(ops) => {
            set_cache_path_impl(&ops, host, track_id, path_slice(path, path_length))
        }
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_unfreeze(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => unfreeze_impl(&ops, host, track_id),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_is_frozen(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => is_frozen_impl(&ops, host, track_id),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn log(entry: impl Into<String>) {
        LOG.with(|log| log.borrow_mut().push(entry.into()));
    }

    fn take_log() -> Vec<String> {
        LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
    }

    unsafe extern "C" fn fake_acquire(host: *mut c_void, track_id: u32, wait: bool) -> *mut c_void {
        log(format!("acquire {} wait={wait}", host as usize));
        if track_id == 99 {
            return std::ptr::null_mut();
        }
        track_id as usize as *mut c_void
    }

    unsafe extern "C" fn fake_release(handle: *mut c_void) {
        log(format!("release {}", handle as usize));
    }

    unsafe extern "C" fn fake_freeze(handle: *mut c_void, total: u64, rate: u32) -> bool {
        log(format!("freeze {} {total} {rate}", handle as usize));
        true
    }

    unsafe extern "C" fn fake_run_from_file(
        handle: *mut c_void,
        path: *const u8,
        length: usize,
        total: u64,
        rate: u32,
        persist: bool,
    ) -> bool {
        let path = std::str::from_utf8(std::slice::from_raw_parts(path, length)).unwrap();
        let action = if persist { "file" } else { "restore" };
        log(format!("{action} {} {path} {total} {rate}", handle as usize));
        true
    }

    unsafe extern "C" fn empty_project_end(_handle: *mut c_void, out: *mut u64) -> bool {
        unsafe { *out = 0 };
        true
    }

    unsafe extern "C" fn huge_project_end(_handle: *mut c_void, out: *mut u64) -> bool {
        unsafe { *out = MAX_FREEZE_SAMPLES + 1 };
        true
    }

    unsafe extern "C" fn fake_set_cache(handle: *mut c_void, path: *const u8, length: usize) -> bool {
        let path = std::str::from_utf8(std::slice::from_raw_parts(path, length)).unwrap();
        log(format!("cache {} {path}", handle as usize));
        path != "reject"
    }

    unsafe extern "C" fn fake_unfreeze(handle: *mut c_void) {
        log(format!("unfreeze {}", handle as usize));
    }

    unsafe extern "C" fn fake_is_frozen(handle: *mut c_void) -> bool {
        log(format!("is_frozen {}", handle as usize));
        handle as usize == 5
    }

    unsafe extern "C" fn fake_project_end(handle: *mut c_void, out: *mut u64) -> bool {
        log(format!("project_end {}", handle as usize));
        unsafe { *out = 1_000 };
        true
    }

    unsafe extern "C" fn unreadable_project_end(handle: *mut c_void, _out: *mut u64) -> bool {
        log(format!("project_end {}", handle as usize));
        false
    }

    unsafe extern "C" fn fake_block_size(_host: *mut c_void) -> u32 {
        256
    }

    fn fake_ops() -> HirariTrackFreezeHostOps {
        HirariTrackFreezeHostOps {
            acquire: Some(fake_acquire),
            release: Some(fake_release),
            freeze: Some(fake_freeze),
            run_from_file: Some(fake_run_from_file),
            set_cache_path: Some(fake_set_cache),
            unfreeze: Some(fake_unfreeze),
            is_frozen: Some(fake_is_frozen),
            project_end: Some(fake_project_end),
            block_size: Some(fake_block_size),
        }
    }

    #[test]
    fn freeze_rejects_empty_and_oversized_requests_before_touching_the_engine() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!freeze_impl(&ops, host, 1, 0, 48_000));
        assert!(!freeze_impl(&ops, host, 1, 16, 0));
        assert!(!freeze_impl(&ops, host, 1, MAX_FREEZE_SAMPLES + 1, 48_000));
        assert!(take_log().is_empty());
    }

    #[test]
    fn freeze_drains_callbacks_renders_and_releases_the_pin() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(freeze_impl(&ops, host, 5, 480, 44_100));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=true".to_string(),
                "freeze 5 480 44100".to_string(),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn freeze_reports_a_missing_track() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!freeze_impl(&ops, host, 99, 480, 44_100));
        assert_eq!(take_log(), vec!["acquire 8 wait=true".to_string()]);
    }

    #[test]
    fn project_end_uses_the_larger_of_last_region_and_two_blocks() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(freeze_to_project_end_impl(&ops, host, 5, 48_000));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=false".to_string(),
                "project_end 5".to_string(),
                "release 5".to_string(),
                "acquire 8 wait=true".to_string(),
                "freeze 5 1000 48000".to_string(),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn project_end_falls_back_to_two_blocks_for_an_empty_track() {
        let mut ops = fake_ops();
        ops.project_end = Some(empty_project_end);
        let host = 8_usize as *mut c_void;
        assert!(freeze_to_project_end_impl(&ops, host, 5, 48_000));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=false".to_string(),
                "release 5".to_string(),
                "acquire 8 wait=true".to_string(),
                "freeze 5 512 48000".to_string(),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn project_end_rejects_missing_track_and_zero_rate() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!freeze_to_project_end_impl(&ops, host, 5, 0));
        assert!(take_log().is_empty());
        assert!(!freeze_to_project_end_impl(&ops, host, 99, 48_000));
        assert_eq!(take_log(), vec!["acquire 8 wait=false".to_string()]);
    }

    #[test]
    fn project_end_rejects_an_unreadable_region_table() {
        let mut ops = fake_ops();
        ops.project_end = Some(unreadable_project_end);
        assert!(!freeze_to_project_end_impl(&ops, 8_usize as *mut c_void, 5, 48_000));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=false".to_string(),
                "project_end 5".to_string(),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn project_end_rejects_oversized_total_before_the_render() {
        let mut ops = fake_ops();
        ops.project_end = Some(huge_project_end);
        assert!(!freeze_to_project_end_impl(&ops, 8_usize as *mut c_void, 5, 48_000));
        assert_eq!(
            take_log(),
            vec!["acquire 8 wait=false".to_string(), "release 5".to_string()]
        );
    }

    #[test]
    fn to_file_resolves_an_absolute_path_after_quiescing() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(freeze_to_file_impl(&ops, host, 5, b"cache/take.wav", 480, 48_000));
        let expected = std::path::absolute("cache/take.wav").unwrap();
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=true".to_string(),
                format!("file 5 {} 480 48000", expected.display()),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn to_file_rejects_empty_path_and_oversized_total() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!freeze_to_file_impl(&ops, host, 5, b"", 480, 48_000));
        assert!(!freeze_to_file_impl(
            &ops,
            host,
            5,
            b"take.wav",
            MAX_FREEZE_SAMPLES + 1,
            48_000
        ));
        assert!(take_log().is_empty());
    }

    #[test]
    fn restore_uses_the_given_path_and_drains_first() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(restore_from_file_impl(&ops, host, 5, b"cache/take.wav", 480, 48_000));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=true".to_string(),
                "restore 5 cache/take.wav 480 48000".to_string(),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn restore_rejects_incomplete_requests_without_quiescing() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!restore_from_file_impl(&ops, host, 5, b"", 480, 48_000));
        assert!(!restore_from_file_impl(&ops, host, 5, b"take.wav", 0, 48_000));
        assert!(!restore_from_file_impl(&ops, host, 5, b"take.wav", 480, 0));
        assert!(take_log().is_empty());
    }

    #[test]
    fn cache_path_does_not_drain_callbacks() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(set_cache_path_impl(&ops, host, 5, b"cache/take.wav"));
        assert!(!set_cache_path_impl(&ops, host, 5, b""));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=false".to_string(),
                "cache 5 cache/take.wav".to_string(),
                "release 5".to_string(),
            ]
        );
    }

    #[test]
    fn cache_path_reports_a_missing_track() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!set_cache_path_impl(&ops, host, 99, b"cache/take.wav"));
        assert_eq!(take_log(), vec!["acquire 8 wait=false".to_string()]);
    }

    #[test]
    fn unfreeze_drains_and_reports_missing_tracks() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(unfreeze_impl(&ops, host, 5));
        assert!(!unfreeze_impl(&ops, host, 99));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=true".to_string(),
                "unfreeze 5".to_string(),
                "release 5".to_string(),
                "acquire 8 wait=true".to_string(),
            ]
        );
    }

    #[test]
    fn is_frozen_reads_without_draining() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(is_frozen_impl(&ops, host, 5));
        assert!(!is_frozen_impl(&ops, host, 6));
        assert!(!is_frozen_impl(&ops, host, 99));
        assert_eq!(
            take_log(),
            vec![
                "acquire 8 wait=false".to_string(),
                "is_frozen 5".to_string(),
                "release 5".to_string(),
                "acquire 8 wait=false".to_string(),
                "is_frozen 6".to_string(),
                "release 6".to_string(),
                "acquire 8 wait=false".to_string(),
            ]
        );
    }

    #[test]
    fn missing_ops_fail_closed() {
        let ops = HirariTrackFreezeHostOps::default();
        let host = 8_usize as *mut c_void;
        assert!(!freeze_impl(&ops, host, 5, 480, 48_000));
        assert!(!freeze_to_project_end_impl(&ops, host, 5, 48_000));
        assert!(!freeze_to_file_impl(&ops, host, 5, b"take.wav", 480, 48_000));
        assert!(!restore_from_file_impl(&ops, host, 5, b"take.wav", 480, 48_000));
        assert!(!set_cache_path_impl(&ops, host, 5, b"take.wav"));
        assert!(!unfreeze_impl(&ops, host, 5));
        assert!(!is_frozen_impl(&ops, host, 5));
    }
}
