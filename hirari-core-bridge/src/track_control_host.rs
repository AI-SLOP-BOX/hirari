//! Track control-plane policy: spatial-position validation and HRTF kernel
//! admission (non-empty, matching stereo taps, bounded length).
//!
//! The engine keeps the track vector and the panner. This module decides
//! which spatial requests are valid and pins the track for the write.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

/// Mirrors `HolographicPanner::kMaxHrtfTaps`.
pub const MAX_HRTF_TAPS: usize = 128;
/// Pins a live track for one request; null when the id no longer resolves.
pub type AcquireTrackFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
pub type ReleaseTrackFn = unsafe extern "C" fn(*mut c_void);
pub type SetSpatialFn = unsafe extern "C" fn(*mut c_void, f32, f32, f32);
pub type SetHrtfFn = unsafe extern "C" fn(*mut c_void, *const f32, *const f32, u32) -> bool;
pub type ClearHrtfFn = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariTrackControlHostOps {
    pub acquire: Option<AcquireTrackFn>,
    pub release: Option<ReleaseTrackFn>,
    pub set_spatial: Option<SetSpatialFn>,
    pub set_hrtf: Option<SetHrtfFn>,
    pub clear_hrtf: Option<ClearHrtfFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariTrackControlHostOps {}
unsafe impl Sync for HirariTrackControlHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariTrackControlHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariTrackControlHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_control_host_install(
    ops: *const HirariTrackControlHostOps,
) {
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

/// Releases the pinned track when the request ends, even on an early return.
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
    ops: &HirariTrackControlHostOps,
    host: *mut c_void,
    track_id: u32,
) -> Option<TrackPin> {
    let acquire = ops.acquire?;
    let release = ops.release?;
    let raw = unsafe { acquire(host, track_id) };
    if raw.is_null() {
        return None;
    }
    Some(TrackPin { raw, release })
}

fn set_spatial_impl(
    ops: &HirariTrackControlHostOps,
    host: *mut c_void,
    track_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> bool {
    // A non-finite coordinate would poison the panner's history. Reject it
    // before the track is even looked up.
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return false;
    }
    let Some(set_spatial) = ops.set_spatial else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id) else {
        return false;
    };
    unsafe { set_spatial(pin.raw, x, y, z) };
    true
}

fn set_hrtf_impl(
    ops: &HirariTrackControlHostOps,
    host: *mut c_void,
    track_id: u32,
    left: *const f32,
    left_len: usize,
    right: *const f32,
    right_len: usize,
) -> bool {
    if left_len == 0
        || left_len != right_len
        || left_len > MAX_HRTF_TAPS
        || left.is_null()
        || right.is_null()
    {
        return false;
    }
    let Some(set_hrtf) = ops.set_hrtf else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id) else {
        return false;
    };
    let taps = left_len as u32;
    unsafe { set_hrtf(pin.raw, left, right, taps) }
}

fn clear_hrtf_impl(
    ops: &HirariTrackControlHostOps,
    host: *mut c_void,
    track_id: u32,
) -> bool {
    let Some(clear_hrtf) = ops.clear_hrtf else {
        return false;
    };
    let Some(pin) = acquire(ops, host, track_id) else {
        return false;
    };
    unsafe { clear_hrtf(pin.raw) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_spatial_position(
    host: *mut c_void,
    track_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> bool {
    match host_ops() {
        Some(ops) => set_spatial_impl(&ops, host, track_id, x, y, z),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_hrtf_kernel(
    host: *mut c_void,
    track_id: u32,
    left: *const f32,
    left_len: usize,
    right: *const f32,
    right_len: usize,
) -> bool {
    match host_ops() {
        Some(ops) => set_hrtf_impl(&ops, host, track_id, left, left_len, right, right_len),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_clear_hrtf_kernel(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => clear_hrtf_impl(&ops, host, track_id),
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

    unsafe extern "C" fn fake_acquire(_host: *mut c_void, track_id: u32) -> *mut c_void {
        log(format!("acquire {track_id}"));
        if track_id == 99 {
            return std::ptr::null_mut();
        }
        track_id as usize as *mut c_void
    }

    unsafe extern "C" fn fake_release(handle: *mut c_void) {
        log(format!("release {}", handle as usize));
    }

    unsafe extern "C" fn fake_set_spatial(handle: *mut c_void, x: f32, y: f32, z: f32) {
        log(format!("spatial {} {x} {y} {z}", handle as usize));
    }

    unsafe extern "C" fn fake_set_hrtf(
        handle: *mut c_void,
        left: *const f32,
        right: *const f32,
        taps: u32,
    ) -> bool {
        let left = unsafe { std::slice::from_raw_parts(left, taps as usize) };
        let right = unsafe { std::slice::from_raw_parts(right, taps as usize) };
        log(format!("hrtf {} {taps} {left:?} {right:?}", handle as usize));
        true
    }

    unsafe extern "C" fn fake_clear_hrtf(handle: *mut c_void) {
        log(format!("clear {}", handle as usize));
    }

    fn fake_ops() -> HirariTrackControlHostOps {
        HirariTrackControlHostOps {
            acquire: Some(fake_acquire),
            release: Some(fake_release),
            set_spatial: Some(fake_set_spatial),
            set_hrtf: Some(fake_set_hrtf),
            clear_hrtf: Some(fake_clear_hrtf),
        }
    }

    #[test]
    fn spatial_position_rejects_non_finite_coordinates_locally() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        assert!(!set_spatial_impl(&ops, host, 5, f32::NAN, 0.0, 0.0));
        assert!(!set_spatial_impl(&ops, host, 5, 0.0, f32::INFINITY, 0.0));
        assert!(!set_spatial_impl(&ops, host, 5, 0.0, 0.0, f32::NEG_INFINITY));
        assert!(take_log().is_empty());
    }

    #[test]
    fn spatial_position_pins_forwards_and_releases() {
        let ops = fake_ops();
        assert!(set_spatial_impl(&ops, 8_usize as *mut c_void, 5, 1.0, 2.0, 3.0));
        assert_eq!(
            take_log(),
            vec!["acquire 5", "spatial 5 1 2 3", "release 5"]
        );
    }

    #[test]
    fn spatial_position_reports_a_missing_track() {
        let ops = fake_ops();
        assert!(!set_spatial_impl(&ops, 8_usize as *mut c_void, 99, 1.0, 2.0, 3.0));
        assert_eq!(take_log(), vec!["acquire 99"]);
    }

    #[test]
    fn hrtf_rejects_incomplete_or_oversized_kernels() {
        let ops = fake_ops();
        let host = 8_usize as *mut c_void;
        let taps = vec![0.5f32; MAX_HRTF_TAPS + 1];
        assert!(!set_hrtf_impl(&ops, host, 5, std::ptr::null(), 0, std::ptr::null(), 0));
        assert!(!set_hrtf_impl(&ops, host, 5, taps.as_ptr(), 2, taps.as_ptr(), 3));
        assert!(!set_hrtf_impl(
            &ops,
            host,
            5,
            taps.as_ptr(),
            MAX_HRTF_TAPS + 1,
            taps.as_ptr(),
            MAX_HRTF_TAPS + 1
        ));
        assert!(take_log().is_empty());
    }

    #[test]
    fn hrtf_pins_forwards_a_matching_kernel_and_releases() {
        let ops = fake_ops();
        let left = [0.5f32, 0.25];
        let right = [0.75f32, 0.125];
        assert!(set_hrtf_impl(
            &ops,
            8_usize as *mut c_void,
            5,
            left.as_ptr(),
            left.len(),
            right.as_ptr(),
            right.len()
        ));
        assert_eq!(
            take_log(),
            vec!["acquire 5", "hrtf 5 2 [0.5, 0.25] [0.75, 0.125]", "release 5"]
        );
    }

    #[test]
    fn clear_hrtf_pins_forwards_and_releases() {
        let ops = fake_ops();
        assert!(clear_hrtf_impl(&ops, 8_usize as *mut c_void, 5));
        assert_eq!(take_log(), vec!["acquire 5", "clear 5", "release 5"]);
    }

    #[test]
    fn missing_ops_fail_closed() {
        let ops = HirariTrackControlHostOps::default();
        let host = 8_usize as *mut c_void;
        assert!(!set_spatial_impl(&ops, host, 5, 0.0, 0.0, 0.0));
        assert!(!set_hrtf_impl(&ops, host, 5, [0.5f32].as_ptr(), 1, [0.5f32].as_ptr(), 1));
        assert!(!clear_hrtf_impl(&ops, host, 5));
        assert!(take_log().is_empty());
    }
}
