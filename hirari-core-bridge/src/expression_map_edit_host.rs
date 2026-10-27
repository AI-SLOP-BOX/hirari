//! MIDI expression-map edit policy.
//!
//! The articulation storage belongs to Rust and the audio callback reads it,
//! but the edit rules still lived in the engine: admit the packed map, skip a
//! no-op write, apply the rows, and record one coalescing history entry whose
//! name carries the track and articulation. This module owns that policy. The
//! engine supplies two callbacks: one resolves a track's articulation handle,
//! the other stores the map JSON and marks the project layout changed, so the
//! replay path re-resolves the track on every undo/redo as the native lambdas
//! did.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::track_midi_articulation::{
    hirari_track_midi_articulation_begin, hirari_track_midi_articulation_finish,
    hirari_track_midi_articulation_pack, hirari_track_midi_articulation_set_output,
    hirari_track_midi_expression_map_copy,
};

/// Resolves a track's articulation runtime handle, or null when the track is
/// not in the published audio state.
pub type TrackArticulationFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
/// Stores the map JSON (`kind` 0: articulation map, 1: pro map) on a track and
/// marks the project layout changed. Reports whether the track exists.
pub type SetMapJsonFn = unsafe extern "C" fn(*mut c_void, u32, u8, *const u8, usize) -> bool;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariExpressionMapEditHostOps {
    pub track_articulation: Option<TrackArticulationFn>,
    pub set_map_json: Option<SetMapJsonFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariExpressionMapEditHostOps {}
unsafe impl Sync for HirariExpressionMapEditHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariExpressionMapEditHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariExpressionMapEditHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_expression_map_edit_host_install(
    ops: *const HirariExpressionMapEditHostOps,
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

/// Count/copy read of the packed articulation map, retried when a concurrent
/// writer reports a size larger than the buffer.
fn read_packed(state: *mut c_void) -> Vec<u32> {
    let mut values = vec![
        0u32;
        unsafe { hirari_track_midi_articulation_pack(state, std::ptr::null_mut(), 0) }
    ];
    loop {
        let output = if values.is_empty() {
            std::ptr::null_mut()
        } else {
            values.as_mut_ptr()
        };
        let required = unsafe { hirari_track_midi_articulation_pack(state, output, values.len()) };
        if required <= values.len() {
            values.truncate(required);
            return values;
        }
        values.resize(required, 0);
    }
}

/// The map JSON (`kind` 0: articulation map, 1: pro map), read through the
/// same retrying copy the native `copyExpressionMapJson` helper used.
fn read_json(state: *mut c_void, kind: u8) -> Vec<u8> {
    let mut bytes = vec![
        0u8;
        unsafe { hirari_track_midi_expression_map_copy(state, kind, std::ptr::null_mut(), 0) }
    ];
    loop {
        let output = if bytes.is_empty() {
            std::ptr::null_mut()
        } else {
            bytes.as_mut_ptr()
        };
        let copied = unsafe {
            hirari_track_midi_expression_map_copy(state, kind, output, bytes.len())
        };
        if copied <= bytes.len() {
            bytes.truncate(copied);
            return bytes;
        }
        bytes.resize(copied, 0);
    }
}

/// Applies packed rows to the articulation runtime. Rows are grouped by
/// articulation and every id is rewritten, including the ones without rows,
/// exactly as the native visitor did; the byte narrowing matches its
/// `static_cast<uint8_t>` on each output field.
fn apply_packed(state: *mut c_void, values: &[u32]) -> bool {
    let mut outputs = vec![[[0u32; 5]; 16]; 256];
    let mut counts = [0u8; 256];
    for row in values.chunks_exact(6) {
        let id = row[0] as usize;
        let slot = counts[id] as usize;
        outputs[id][slot] = [row[2], row[1], row[3], row[4], row[5]];
        counts[id] += 1;
    }
    for raw_id in 1..256u32 {
        let id = raw_id as u8;
        if !unsafe { hirari_track_midi_articulation_begin(state, id) } {
            return false;
        }
        for (slot, output) in outputs[raw_id as usize][..counts[raw_id as usize] as usize]
            .iter()
            .enumerate()
        {
            if !unsafe {
                hirari_track_midi_articulation_set_output(
                    state,
                    id,
                    slot as u8,
                    output[0] as u8,
                    output[1] as u8,
                    output[2] as u8,
                    output[3] as u8,
                    output[4] as u8,
                )
            } {
                return false;
            }
        }
        if !unsafe { hirari_track_midi_articulation_finish(state, id, counts[raw_id as usize]) } {
            return false;
        }
    }
    true
}

/// Replays one snapshot through the host, re-resolving the track by id. The
/// native `applySnapshot` silently did nothing when the track was gone, and
/// stopped before storing the JSON when a row failed.
fn apply_snapshot(
    ops: HirariExpressionMapEditHostOps,
    host: *mut c_void,
    track_id: u32,
    kind: u8,
    packed: Option<&[u32]>,
    json: &[u8],
) -> bool {
    let (Some(track_articulation), Some(set_map_json)) = (ops.track_articulation, ops.set_map_json)
    else {
        return false;
    };
    let state = unsafe { track_articulation(host, track_id) };
    if state.is_null() {
        return false;
    }
    if let Some(values) = packed {
        if !apply_packed(state, values) {
            return false;
        }
    }
    unsafe { set_map_json(host, track_id, kind, json.as_ptr(), json.len()) }
}

/// Applies a packed articulation map plus its JSON serialization and records
/// one history entry. A no-op write returns success without touching history.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect, `packed`
/// must be readable for `packed_count` values and `json` for `json_length`
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_expression_map_edit_set(
    host: *mut c_void,
    track_id: u32,
    packed: *const u32,
    packed_count: usize,
    json: *const u8,
    json_length: usize,
    articulation_id: u32,
    undo: *mut c_void,
    log: *const c_void,
) -> bool {
    if (packed_count != 0 && packed.is_null()) || (json_length != 0 && json.is_null()) {
        return false;
    }
    let values = if packed_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(packed, packed_count) }
    };
    let json = if json_length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(json, json_length) }
    };
    if !crate::expression_map::admit_packed_map(values, json.len(), articulation_id) {
        return false;
    }
    let Some(ops) = host_ops() else { return false };
    let Some(track_articulation) = ops.track_articulation else {
        return false;
    };
    let state = unsafe { track_articulation(host, track_id) };
    if state.is_null() {
        return false;
    }
    let before_packed = read_packed(state);
    let before_json = read_json(state, 0);
    if before_packed == values && before_json == json {
        return true;
    }
    apply_snapshot(ops, host, track_id, 0, Some(values), json);

    let name = if articulation_id == 0 {
        format!("Edit MIDI Expression Map T{track_id}")
    } else {
        format!("Edit MIDI Expression Map T{track_id} A{articulation_id}")
    };
    let after_packed = values.to_vec();
    let after_json = json.to_vec();
    crate::record_external_action_coalesced(
        undo,
        log,
        &name,
        move || {
            apply_snapshot(ops, host, track_id, 0, Some(&before_packed), &before_json);
        },
        move || {
            apply_snapshot(ops, host, track_id, 0, Some(&after_packed), &after_json);
        },
    );
    true
}

/// Applies a pro-map JSON snapshot and records one coalescing history entry.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect and `json`
/// must be readable for `json_length` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_expression_map_edit_set_pro(
    host: *mut c_void,
    track_id: u32,
    json: *const u8,
    json_length: usize,
    undo: *mut c_void,
    log: *const c_void,
) -> bool {
    if track_id == 0 || json_length > 1024 * 1024 || (json_length != 0 && json.is_null()) {
        return false;
    }
    let json = if json_length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(json, json_length) }
    };
    let Some(ops) = host_ops() else { return false };
    let (Some(track_articulation), Some(set_map_json)) = (ops.track_articulation, ops.set_map_json)
    else {
        return false;
    };
    let state = unsafe { track_articulation(host, track_id) };
    if state.is_null() {
        return false;
    }
    let before = read_json(state, 1);
    if before == json {
        return true;
    }
    unsafe { set_map_json(host, track_id, 1, json.as_ptr(), json.len()) };
    let after = json.to_vec();
    crate::record_external_action_coalesced(
        undo,
        log,
        &format!("Edit MIDI Expression Map Pro T{track_id}"),
        move || {
            apply_snapshot(ops, host, track_id, 1, None, &before);
        },
        move || {
            apply_snapshot(ops, host, track_id, 1, None, &after);
        },
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track_midi_articulation::{
        hirari_track_midi_articulation_create, hirari_track_midi_articulation_destroy,
    };

    #[test]
    fn packed_rows_round_trip_through_the_apply_path() {
        let state = hirari_track_midi_articulation_create();
        let rows = [
            1u32, 0, 1, 60, 100, 0, // articulation 1, key switch A0
            1u32, 0, 3, 64, 0, 0,   // articulation 1, control change
            2u32, 2, 16, 1, 0, 0,   // articulation 2, group select
        ];
        assert!(apply_packed(state, &rows));
        let mut packed = vec![0u32; unsafe {
            hirari_track_midi_articulation_pack(state, std::ptr::null_mut(), 0)
        }];
        let count = unsafe {
            hirari_track_midi_articulation_pack(state, packed.as_mut_ptr(), packed.len())
        };
        assert_eq!(count, rows.len());
        packed.truncate(count);
        assert_eq!(packed, rows);
        unsafe { hirari_track_midi_articulation_destroy(state) };
    }

    #[test]
    fn applying_an_empty_map_clears_every_articulation() {
        let state = hirari_track_midi_articulation_create();
        assert!(apply_packed(state, &[1, 0, 1, 60, 100, 0]));
        assert!(apply_packed(state, &[]));
        let required = unsafe { hirari_track_midi_articulation_pack(state, std::ptr::null_mut(), 0) };
        assert_eq!(required, 0);
        unsafe { hirari_track_midi_articulation_destroy(state) };
    }
}
