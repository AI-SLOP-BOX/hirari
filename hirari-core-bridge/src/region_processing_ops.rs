//! Per-region processing history operations.
//!
//! Exposes append and clear operations directly on the Rust-owned `region_store`
//! without bouncing through the C++ `HirariUnifiedEngine` layer.

use std::ffi::c_void;
use crate::region_processing_history::HirariEventStep;
use crate::region_store::{
    hirari_region_store_append_processing, hirari_region_store_clear_processing,
};
use crate::track_session::TrackSession;

/// Appends a processing step to a region within a track.
pub fn append_region_processing(
    host: *mut c_void,
    tid: u32,
    rid: u32,
    step_id: u32,
    operation: &str,
    parameter: f32,
    enabled: bool,
) -> bool {
    let Some(session) = TrackSession::open(host, tid) else {
        return false;
    };
    let step = HirariEventStep {
        id: step_id,
        operation: operation.as_ptr(),
        operation_len: operation.len(),
        parameter,
        enabled,
    };
    unsafe { hirari_region_store_append_processing(session.track(), rid, step) }
}

/// Clears all processing steps from a region within a track.
pub fn clear_region_processing(host: *mut c_void, tid: u32, rid: u32) -> bool {
    let Some(session) = TrackSession::open(host, tid) else {
        return false;
    };
    unsafe { hirari_region_store_clear_processing(session.track(), rid) }
}
