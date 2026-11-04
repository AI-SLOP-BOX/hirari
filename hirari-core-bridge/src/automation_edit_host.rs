//! Automation curve edits.
//!
//! Every curve a track carries lives in that track's own Rust automation store,
//! so reading and replacing one needs no host callback at all: this module opens
//! a track session, validates the packed curve, writes the store and records
//! exactly one undo entry. The engine contributes the tracks lock, the storage
//! handle behind it, and the undo history.
//!
//! Every entry point here runs on the control thread. Nothing in this module is
//! reachable from the audio callback.

use std::os::raw::c_void;
use std::sync::{Mutex, OnceLock};

use crate::plugin_edit_host::{register_replay, UndoStateFn};
use crate::track_automation_store::{
    hirari_track_automation_store_plugin_lane_views, hirari_track_automation_store_points,
    hirari_track_automation_store_replace_plugin_lane, hirari_track_automation_store_set_points,
    AutomationPoint,
};
use crate::track_session::TrackSession;

/// An automation point is the store's own type; the edit host has no separate
/// mirror of it any more.
pub type HirariAutomationPoint = AutomationPoint;

/// Target kind: the track's volume curve.
pub const HIRARI_AUTOMATION_VOLUME: i32 = 1;
/// Target kind: the track's pan curve.
pub const HIRARI_AUTOMATION_PAN: i32 = 2;
/// Target kind: the track's delay curve.
pub const HIRARI_AUTOMATION_TRACK_DELAY: i32 = 3;
/// Target kind: one lane of a plugin parameter curve.
pub const HIRARI_AUTOMATION_PLUGIN: i32 = 4;

const MAX_PLUGIN_POINTS: usize = 100_000;
const MAX_DELAY_POINTS: usize = 8192;
/// The largest sample index a plugin lane accepts; larger values lose integer
/// precision in the automation renderer.
const MAX_PLUGIN_TIME: f64 = 9_007_199_254_740_992.0;
/// The largest curve the editor will copy into one undo entry.
const MAX_READ_POINTS: usize = 1_000_000;

/// The one thing an automation edit cannot do alone: reach the engine's undo
/// history. Reading and writing the curves is Rust talking to the track's own
/// store.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariAutomationHostOps {
    pub undo_state: Option<UndoStateFn>,
}

// The table only holds function pointers supplied by the native engine; it
// carries no data the Rust side mutates.
unsafe impl Send for HirariAutomationHostOps {}
unsafe impl Sync for HirariAutomationHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariAutomationHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariAutomationHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
///
/// # Safety
/// `ops` must point to a valid table that stays readable for the duration of
/// the call.
#[no_mangle]
pub unsafe extern "C" fn hirari_automation_host_install(ops: *const HirariAutomationHostOps) {
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

/// Validates packed `(time, value, curve)` triples and converts them to points.
/// The accepted domain is the one the previous C++ façade + engine pair
/// enforced together.
fn packed_points(
    packed: *const f64,
    packed_len: usize,
    kind: i32,
    parameter_id: u32,
) -> Option<Vec<HirariAutomationPoint>> {
    if !packed_len.is_multiple_of(3) {
        return None;
    }
    let point_limit = match kind {
        HIRARI_AUTOMATION_VOLUME | HIRARI_AUTOMATION_PAN => {
            if parameter_id > 1 {
                return None;
            }
            MAX_READ_POINTS
        }
        HIRARI_AUTOMATION_TRACK_DELAY => MAX_DELAY_POINTS,
        HIRARI_AUTOMATION_PLUGIN => MAX_PLUGIN_POINTS,
        _ => return None,
    };
    let point_count = packed_len / 3;
    if point_count > point_limit {
        return None;
    }
    if point_count != 0 && packed.is_null() {
        return None;
    }
    let mut points = Vec::with_capacity(point_count);
    let mut previous_time = -1.0;
    for index in 0..point_count {
        // SAFETY: the caller guarantees `packed` holds `packed_len` values.
        let (time, value, curve) = unsafe {
            (
                *packed.add(index * 3),
                *packed.add(index * 3 + 1),
                *packed.add(index * 3 + 2),
            )
        };
        if !time.is_finite() || time < 0.0 || time.trunc() != time || time <= previous_time {
            return None;
        }
        if kind == HIRARI_AUTOMATION_PLUGIN && time > MAX_PLUGIN_TIME {
            return None;
        }
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return None;
        }
        if !curve.is_finite() || !(-1.0..=1.0).contains(&curve) {
            return None;
        }
        previous_time = time;
        points.push(HirariAutomationPoint {
            time,
            value: value as f32,
            curve: curve as f32,
        });
    }
    Some(points)
}

fn action_name(kind: i32, track_id: u32) -> Vec<u8> {
    let mut name = match kind {
        HIRARI_AUTOMATION_VOLUME => format!("Volume Automation Track {track_id}"),
        HIRARI_AUTOMATION_PAN => format!("Pan Automation Track {track_id}"),
        HIRARI_AUTOMATION_TRACK_DELAY => format!("Track Delay Automation {track_id}"),
        _ => format!("Plugin Parameter Automation Track {track_id}"),
    };
    name.push('\0');
    name.into_bytes()
}

/// The store kind a track-level curve lives under. A kind that is neither pan
/// nor delay reads and writes the volume curve, which is how the engine's host
/// callback treated its own default branch.
fn track_store_kind(kind: i32) -> u32 {
    match kind {
        HIRARI_AUTOMATION_PAN => 1,
        HIRARI_AUTOMATION_TRACK_DELAY => 2,
        _ => 0,
    }
}

/// The lane addressed to `(plugin_index, parameter_id)`. No matching lane is the
/// empty curve, which is what clearing one looks like in the store.
fn lane_points(
    store: *mut c_void,
    plugin_index: u32,
    parameter_id: u32,
) -> Vec<HirariAutomationPoint> {
    let mut count = 0usize;
    let views = unsafe { hirari_track_automation_store_plugin_lane_views(store, &mut count) };
    if views.is_null() {
        return Vec::new();
    }
    let lanes = unsafe { std::slice::from_raw_parts(views, count) };
    lanes
        .iter()
        .find(|lane| lane.processor_index == plugin_index && lane.parameter_id == parameter_id)
        .map_or(Vec::new(), |lane| {
            if lane.points.is_null() || lane.point_count == 0 {
                return Vec::new();
            }
            unsafe { std::slice::from_raw_parts(lane.points, lane.point_count) }.to_vec()
        })
}

/// Reads the target's current curve out of the track's own store while the
/// engine lock is held for it. A missing track, or a plugin slot the chain does
/// not have, is the rejection the host callback used to report as
/// `targetFound == false`.
fn read_points(
    host: *mut c_void,
    track_id: u32,
    kind: i32,
    plugin_index: u32,
    parameter_id: u32,
) -> Option<Vec<HirariAutomationPoint>> {
    let session = TrackSession::open(host, track_id)?;
    let store = session.automation();
    let points = if kind == HIRARI_AUTOMATION_PLUGIN {
        let slots = unsafe {
            crate::effect_chain_host::hirari_effect_chain_count(session.chain() as *const c_void)
        };
        if plugin_index >= slots {
            return None;
        }
        lane_points(store, plugin_index, parameter_id)
    } else {
        let mut count = 0usize;
        let raw =
            unsafe { hirari_track_automation_store_points(store, track_store_kind(kind), &mut count) };
        if raw.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(raw, count) }.to_vec()
        }
    };
    (points.len() <= MAX_READ_POINTS).then_some(points)
}

/// Replaces the target's curve in the track's store, then runs the track's own
/// drop-the-frozen-render and bump-the-layout protocol, which is what makes an
/// automation edit audible and visible to the layout snapshot.
fn apply(
    host: *mut c_void,
    track_id: u32,
    kind: i32,
    plugin_index: u32,
    parameter_id: u32,
    points: &[HirariAutomationPoint],
) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let store = session.automation();
    match kind {
        HIRARI_AUTOMATION_VOLUME | HIRARI_AUTOMATION_PAN | HIRARI_AUTOMATION_TRACK_DELAY => unsafe {
            hirari_track_automation_store_set_points(
                store,
                track_store_kind(kind),
                points.as_ptr(),
                points.len(),
            );
        },
        HIRARI_AUTOMATION_PLUGIN => unsafe {
            hirari_track_automation_store_replace_plugin_lane(
                store,
                plugin_index,
                parameter_id,
                points.as_ptr(),
                points.len(),
            );
        },
        _ => return false,
    }
    session.commit_edit(true)
}

/// One edit kept by the undo history. The point buffers are owned so a replay
/// can run long after the caller's arrays are gone.
struct OwnedAutomationEdit {
    host: *mut c_void,
    track_id: u32,
    kind: i32,
    plugin_index: u32,
    parameter_id: u32,
    points: Vec<HirariAutomationPoint>,
}

impl OwnedAutomationEdit {
    fn new(
        host: *mut c_void,
        track_id: u32,
        kind: i32,
        plugin_index: u32,
        parameter_id: u32,
        points: &[HirariAutomationPoint],
    ) -> Self {
        Self {
            host,
            track_id,
            kind,
            plugin_index,
            parameter_id,
            points: points.to_vec(),
        }
    }

    fn replay(&self) {
        apply(
            self.host,
            self.track_id,
            self.kind,
            self.plugin_index,
            self.parameter_id,
            &self.points,
        );
    }
}

// `host` is the engine the edit was made against, which outlives the undo
// entries recorded in its history.
unsafe impl Send for OwnedAutomationEdit {}

fn record(
    ops: &HirariAutomationHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: OwnedAutomationEdit,
    redo: OwnedAutomationEdit,
) {
    let Some(read_state) = ops.undo_state else {
        return;
    };
    let state = unsafe { read_state(host) };
    if state.is_null() {
        return;
    }
    let undo_id = register_replay(state, Box::new(move || undo.replay()));
    if undo_id == 0 {
        return;
    }
    let redo_id = register_replay(state, Box::new(move || redo.replay()));
    if redo_id == 0 {
        unsafe {
            crate::undo_transaction::hirari_undo_manager_discard_native_callback(state, undo_id)
        };
        return;
    }
    let name = name.strip_suffix(&[0]).unwrap_or(name);
    // The edit was already applied, so the entry is recorded without a replay.
    unsafe {
        crate::undo_transaction::hirari_undo_manager_commit(
            state,
            name.as_ptr(),
            name.len(),
            undo_id,
            redo_id,
            false,
            false,
        );
    }
}

fn edit_with(
    ops: &HirariAutomationHostOps,
    host: *mut c_void,
    track_id: u32,
    kind: i32,
    plugin_index: u32,
    parameter_id: u32,
    packed: *const f64,
    packed_len: usize,
) -> bool {
    let Some(points) = packed_points(packed, packed_len, kind, parameter_id) else {
        return false;
    };
    let Some(before) = read_points(host, track_id, kind, plugin_index, parameter_id) else {
        return false;
    };
    if !apply(host, track_id, kind, plugin_index, parameter_id, &points) {
        return false;
    }
    let undo = OwnedAutomationEdit::new(host, track_id, kind, plugin_index, parameter_id, &before);
    let redo = OwnedAutomationEdit::new(host, track_id, kind, plugin_index, parameter_id, &points);
    // The edit is already applied, so only the undo entry is recorded.
    record(ops, host, &action_name(kind, track_id), undo, redo);
    true
}

/// Replaces one of a track's volume, pan or delay curves.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, `packed`
/// must point to `packed_len` doubles, and the installed table must outlive the
/// call.
#[no_mangle]
pub unsafe extern "C" fn hirari_automation_edit_track(
    host: *mut c_void,
    track_id: u32,
    kind: i32,
    parameter_id: u32,
    packed: *const f64,
    packed_len: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_with(
        &ops,
        host,
        track_id,
        kind,
        0,
        parameter_id,
        packed,
        packed_len,
    )
}

/// Replaces one plugin parameter lane, removing the lane when the curve is
/// empty.
///
/// # Safety
/// `host` must be the opaque handle the installed host ops expect, `packed`
/// must point to `packed_len` doubles, and the installed table must outlive the
/// call.
#[no_mangle]
pub unsafe extern "C" fn hirari_automation_edit_plugin(
    host: *mut c_void,
    track_id: u32,
    plugin_index: u32,
    parameter_id: u32,
    packed: *const f64,
    packed_len: usize,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    edit_with(
        &ops,
        host,
        track_id,
        HIRARI_AUTOMATION_PLUGIN,
        plugin_index,
        parameter_id,
        packed,
        packed_len,
    )
}
