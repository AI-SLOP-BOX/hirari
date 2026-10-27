//! Host-driven automation edits.
//!
//! Automation storage stays in the C++ engine because it owns the track list
//! and the immutable curve snapshots the audio callback reads. This module owns
//! the edit policy: it validates the packed curve, reads the previous curve
//! through a host callback, asks the host to apply, and records exactly one undo
//! entry. The C++ side keeps no per-kind edit code.
//!
//! Every callback here runs on the control thread. Nothing in this module is
//! reachable from the audio callback.

use std::ffi::c_char;
use std::os::raw::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};

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
/// Upper bound for a host-reported curve so a broken host cannot make the
/// control thread allocate without limit.
const MAX_READ_POINTS: usize = 1_000_000;

/// One automation point. Mirrors `ProjectAutomationPoint` byte for byte.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HirariAutomationPoint {
    pub time: f64,
    pub value: f32,
    pub curve: f32,
}

/// One reversible edit. The host copies `points` before returning from the
/// apply or undo capture call.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HirariAutomationEditPayload {
    pub track_id: u32,
    pub kind: i32,
    pub plugin_index: u32,
    pub parameter_id: u32,
    pub points: *const HirariAutomationPoint,
    pub point_count: usize,
}

// The payload only borrows a point buffer for the duration of one host call;
// Rust never dereferences it.
unsafe impl Send for HirariAutomationEditPayload {}
unsafe impl Sync for HirariAutomationEditPayload {}

/// Copies a target's current curve into `out` and reports whether the target
/// exists (the track, and for plugin lanes the plugin slot too). Returns the
/// total point count so callers can size `out` with a second call.
///
/// # Safety
/// `host` must be the opaque handle the native host expects; `out` must hold
/// `capacity` points when non-null, and `targetFound` must be valid or null.
pub type HirariAutomationReadFn = unsafe extern "C" fn(
    *mut c_void,
    u32,
    i32,
    u32,
    u32,
    *mut HirariAutomationPoint,
    usize,
    *mut bool,
) -> usize;
pub type HirariAutomationApplyFn =
    unsafe extern "C" fn(*mut c_void, *const HirariAutomationEditPayload) -> bool;
/// Records one applied edit. The host copies both point buffers before
/// returning.
pub type HirariAutomationRecordUndoFn = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *const HirariAutomationEditPayload,
    *const HirariAutomationEditPayload,
);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariAutomationHostOps {
    pub read_points: Option<HirariAutomationReadFn>,
    pub apply: Option<HirariAutomationApplyFn>,
    pub record_undo: Option<HirariAutomationRecordUndoFn>,
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
        Some(unsafe { ptr::read(ops) })
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

fn read_points(
    ops: &HirariAutomationHostOps,
    host: *mut c_void,
    track_id: u32,
    kind: i32,
    plugin_index: u32,
    parameter_id: u32,
) -> Option<Vec<HirariAutomationPoint>> {
    let read = ops.read_points?;
    let mut found = false;
    let total = unsafe {
        read(
            host,
            track_id,
            kind,
            plugin_index,
            parameter_id,
            ptr::null_mut(),
            0,
            &mut found,
        )
    };
    if !found || total > MAX_READ_POINTS {
        return None;
    }
    if total == 0 {
        return Some(Vec::new());
    }
    let mut points = vec![HirariAutomationPoint::default(); total];
    let copied = unsafe {
        read(
            host,
            track_id,
            kind,
            plugin_index,
            parameter_id,
            points.as_mut_ptr(),
            points.len(),
            &mut found,
        )
    };
    if !found || copied > points.len() {
        return None;
    }
    points.truncate(copied);
    Some(points)
}

fn record(
    ops: &HirariAutomationHostOps,
    host: *mut c_void,
    name: &[u8],
    undo: &HirariAutomationEditPayload,
    redo: &HirariAutomationEditPayload,
) {
    if let Some(record_undo) = ops.record_undo {
        unsafe {
            record_undo(host, name.as_ptr().cast(), undo, redo);
        }
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
    let Some(before) = read_points(ops, host, track_id, kind, plugin_index, parameter_id) else {
        return false;
    };
    let Some(apply) = ops.apply else {
        return false;
    };
    let redo = HirariAutomationEditPayload {
        track_id,
        kind,
        plugin_index,
        parameter_id,
        points: points.as_ptr(),
        point_count: points.len(),
    };
    if !unsafe { apply(host, &redo) } {
        return false;
    }
    let undo = HirariAutomationEditPayload {
        track_id,
        kind,
        plugin_index,
        parameter_id,
        points: before.as_ptr(),
        point_count: before.len(),
    };
    // The host already applied the edit, so only the undo entry is recorded.
    record(
        ops,
        host,
        &action_name(kind, track_id),
        &undo,
        &redo,
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[derive(Default)]
    struct FakeTrack {
        id: u32,
        plugin_slots: u32,
        volume: Vec<HirariAutomationPoint>,
        pan: Vec<HirariAutomationPoint>,
        delay: Vec<HirariAutomationPoint>,
        lanes: Vec<(u32, u32, Vec<HirariAutomationPoint>)>,
    }

    #[derive(Default)]
    struct FakeHost {
        tracks: Vec<FakeTrack>,
        undo: Option<(String, Vec<HirariAutomationPoint>, Vec<HirariAutomationPoint>)>,
    }

    fn point(time: f64, value: f32) -> HirariAutomationPoint {
        HirariAutomationPoint {
            time,
            value,
            curve: 0.0,
        }
    }

    fn packed(points: &[(f64, f64, f64)]) -> Vec<f64> {
        points.iter().flat_map(|&(t, v, c)| [t, v, c]).collect()
    }

    unsafe extern "C" fn fake_read(
        host: *mut c_void,
        track_id: u32,
        kind: i32,
        plugin_index: u32,
        parameter_id: u32,
        out: *mut HirariAutomationPoint,
        capacity: usize,
        found: *mut bool,
    ) -> usize {
        // SAFETY: tests pass a live FakeHost pointer.
        let fake = unsafe { &mut *(host as *mut FakeHost) };
        if !found.is_null() {
            unsafe { *found = false };
        }
        let Some(track) = fake.tracks.iter().find(|track| track.id == track_id) else {
            return 0;
        };
        let none: &[HirariAutomationPoint] = &[];
        let points: &[HirariAutomationPoint] = match kind {
            HIRARI_AUTOMATION_VOLUME => &track.volume,
            HIRARI_AUTOMATION_PAN => &track.pan,
            HIRARI_AUTOMATION_TRACK_DELAY => &track.delay,
            HIRARI_AUTOMATION_PLUGIN => {
                if plugin_index >= track.plugin_slots {
                    return 0;
                }
                track
                    .lanes
                    .iter()
                    .find(|(plugin, parameter, _)| {
                        *plugin == plugin_index && *parameter == parameter_id
                    })
                    .map_or(none, |(_, _, points)| points)
            }
            _ => return 0,
        };
        if !found.is_null() {
            unsafe { *found = true };
        }
        if !out.is_null() && capacity >= points.len() {
            unsafe { ptr::copy_nonoverlapping(points.as_ptr(), out, points.len()) };
        }
        points.len()
    }

    unsafe extern "C" fn fake_apply(
        host: *mut c_void,
        payload: *const HirariAutomationEditPayload,
    ) -> bool {
        let payload = unsafe { &*payload };
        // SAFETY: tests pass a live FakeHost pointer.
        let fake = unsafe { &mut *(host as *mut FakeHost) };
        let points = if payload.point_count == 0 {
            Vec::new()
        } else if payload.points.is_null() {
            return false;
        } else {
            unsafe { std::slice::from_raw_parts(payload.points, payload.point_count) }.to_vec()
        };
        let Some(track) = fake
            .tracks
            .iter_mut()
            .find(|track| track.id == payload.track_id)
        else {
            return false;
        };
        match payload.kind {
            HIRARI_AUTOMATION_VOLUME => track.volume = points,
            HIRARI_AUTOMATION_PAN => track.pan = points,
            HIRARI_AUTOMATION_TRACK_DELAY => track.delay = points,
            HIRARI_AUTOMATION_PLUGIN => {
                if payload.plugin_index >= track.plugin_slots {
                    return false;
                }
                track.lanes.retain(|(plugin, parameter, _)| {
                    !(*plugin == payload.plugin_index && *parameter == payload.parameter_id)
                });
                if !points.is_empty() {
                    track.lanes
                        .push((payload.plugin_index, payload.parameter_id, points));
                }
            }
            _ => return false,
        }
        true
    }

    unsafe extern "C" fn fake_record_undo(
        host: *mut c_void,
        name: *const c_char,
        undo: *const HirariAutomationEditPayload,
        redo: *const HirariAutomationEditPayload,
    ) {
        let label = unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        let copy = |payload: *const HirariAutomationEditPayload| {
            if payload.is_null() {
                return Vec::new();
            }
            let payload = unsafe { &*payload };
            if payload.point_count == 0 {
                return Vec::new();
            }
            unsafe { std::slice::from_raw_parts(payload.points, payload.point_count) }.to_vec()
        };
        // SAFETY: tests pass a live FakeHost pointer.
        let fake = unsafe { &mut *(host as *mut FakeHost) };
        fake.undo = Some((label, copy(undo), copy(redo)));
    }

    fn fake_ops() -> HirariAutomationHostOps {
        HirariAutomationHostOps {
            read_points: Some(fake_read),
            apply: Some(fake_apply),
            record_undo: Some(fake_record_undo),
        }
    }

    fn host() -> FakeHost {
        FakeHost {
            tracks: vec![FakeTrack {
                id: 7,
                plugin_slots: 2,
                ..FakeTrack::default()
            }],
            ..FakeHost::default()
        }
    }

    #[test]
    fn volume_edit_applies_the_curve_and_records_one_undo_entry() {
        let mut fake = host();
        fake.tracks[0].volume = vec![point(10.0, 0.4)];
        let input = packed(&[(20.0, 0.75, 0.5), (30.0, 1.0, -0.25)]);
        let host_ptr = &mut fake as *mut FakeHost as *mut c_void;
        assert!(unsafe {
            edit_with(
                &fake_ops(),
                host_ptr,
                7,
                HIRARI_AUTOMATION_VOLUME,
                0,
                0,
                input.as_ptr(),
                input.len(),
            )
        });
        assert_eq!(
            fake.tracks[0].volume,
            vec![
                HirariAutomationPoint {
                    time: 20.0,
                    value: 0.75,
                    curve: 0.5
                },
                HirariAutomationPoint {
                    time: 30.0,
                    value: 1.0,
                    curve: -0.25
                },
            ]
        );
        let (label, undo, redo) = fake.undo.expect("undo recorded");
        assert_eq!(label, "Volume Automation Track 7");
        assert_eq!(undo, vec![point(10.0, 0.4)]);
        assert_eq!(redo, fake.tracks[0].volume);
    }

    #[test]
    fn point_layout_matches_the_native_project_type() {
        assert_eq!(std::mem::size_of::<HirariAutomationPoint>(), 16);
        assert_eq!(std::mem::offset_of!(HirariAutomationPoint, value), 8);
        assert_eq!(std::mem::offset_of!(HirariAutomationPoint, curve), 12);
    }

    #[test]
    fn delay_and_pan_edits_use_their_own_labels() {
        for (kind, label) in [
            (HIRARI_AUTOMATION_TRACK_DELAY, "Track Delay Automation 7"),
            (HIRARI_AUTOMATION_PAN, "Pan Automation Track 7"),
        ] {
            let mut fake = host();
            let input = packed(&[(0.0, 0.5, 0.0)]);
            let host_ptr = &mut fake as *mut FakeHost as *mut c_void;
            assert!(unsafe {
                edit_with(
                    &fake_ops(),
                    host_ptr,
                    7,
                    kind,
                    0,
                    0,
                    input.as_ptr(),
                    input.len(),
                )
            });
            assert_eq!(fake.undo.expect("undo recorded").0, label);
        }
    }

    #[test]
    fn plugin_edit_replaces_only_the_matching_lane() {
        let mut fake = host();
        fake.tracks[0].lanes = vec![
            (0, 1, vec![point(5.0, 0.2)]),
            (1, 2, vec![point(6.0, 0.3)]),
        ];
        let input = packed(&[(7.0, 0.9, 0.0)]);
        let host_ptr = &mut fake as *mut FakeHost as *mut c_void;
        assert!(unsafe {
            edit_with(
                &fake_ops(),
                host_ptr,
                7,
                HIRARI_AUTOMATION_PLUGIN,
                0,
                1,
                input.as_ptr(),
                input.len(),
            )
        });
        assert_eq!(
            fake.tracks[0].lanes,
            vec![
                (1, 2, vec![point(6.0, 0.3)]),
                (
                    0,
                    1,
                    vec![HirariAutomationPoint {
                        time: 7.0,
                        value: 0.9,
                        curve: 0.0
                    }]
                ),
            ]
        );
        let (label, undo, redo) = fake.undo.expect("undo recorded");
        assert_eq!(label, "Plugin Parameter Automation Track 7");
        assert_eq!(undo, vec![point(5.0, 0.2)]);
        assert_eq!(redo.len(), 1);
    }

    #[test]
    fn plugin_clear_removes_the_lane() {
        let mut fake = host();
        fake.tracks[0].lanes = vec![(0, 1, vec![point(5.0, 0.2)])];
        let host_ptr = &mut fake as *mut FakeHost as *mut c_void;
        assert!(unsafe {
            edit_with(
                &fake_ops(),
                host_ptr,
                7,
                HIRARI_AUTOMATION_PLUGIN,
                0,
                1,
                ptr::null(),
                0,
            )
        });
        assert!(fake.tracks[0].lanes.is_empty());
    }

    #[test]
    fn unknown_track_or_plugin_slot_is_rejected() {
        let mut fake = host();
        let input = packed(&[(0.0, 0.5, 0.0)]);
        let host_ptr = &mut fake as *mut FakeHost as *mut c_void;
        assert!(!unsafe {
            edit_with(
                &fake_ops(),
                host_ptr,
                99,
                HIRARI_AUTOMATION_VOLUME,
                0,
                0,
                input.as_ptr(),
                input.len(),
            )
        });
        assert!(!unsafe {
            edit_with(
                &fake_ops(),
                host_ptr,
                7,
                HIRARI_AUTOMATION_PLUGIN,
                5,
                0,
                input.as_ptr(),
                input.len(),
            )
        });
        assert!(fake.undo.is_none());
    }

    #[test]
    fn malformed_points_are_rejected() {
        let cases: &[&[(f64, f64, f64)]] = &[
            &[(0.0, f64::NAN, 0.0)],
            &[(0.0, 0.5, 0.0), (0.0, 0.5, 0.0)],
            &[(-1.0, 0.5, 0.0)],
            &[(1.5, 0.5, 0.0)],
            &[(0.0, 1.5, 0.0)],
            &[(0.0, -0.1, 0.0)],
            &[(0.0, 0.5, 1.5)],
        ];
        for case in cases {
            let packed = packed(case);
            assert!(packed_points(packed.as_ptr(), packed.len(), HIRARI_AUTOMATION_VOLUME, 0)
                .is_none());
        }
        let odd = [0.0, 0.5];
        assert!(packed_points(odd.as_ptr(), odd.len(), HIRARI_AUTOMATION_VOLUME, 0).is_none());
        let ok = packed(&[(0.0, 0.5, 0.0)]);
        assert!(packed_points(ok.as_ptr(), ok.len(), HIRARI_AUTOMATION_VOLUME, 2).is_none());
        assert!(packed_points(ok.as_ptr(), ok.len(), 9, 0).is_none());
    }

    #[test]
    fn point_limits_match_the_previous_engine() {
        assert!(packed_points(ptr::null(), MAX_DELAY_POINTS * 3 + 3, 3, 0).is_none());
        assert!(packed_points(ptr::null(), MAX_PLUGIN_POINTS * 3 + 3, 4, 0).is_none());
        // Empty curves are valid for every kind.
        assert!(packed_points(ptr::null(), 0, 1, 0).is_some());
        assert!(packed_points(ptr::null(), 0, 4, 0).is_some());
        let too_far = packed(&[(MAX_PLUGIN_TIME * 2.0, 0.5, 0.0)]);
        assert!(packed_points(too_far.as_ptr(), too_far.len(), 4, 0).is_none());
        assert!(packed_points(too_far.as_ptr(), too_far.len(), 1, 0).is_some());
    }

    #[test]
    fn edits_require_a_read_callback() {
        let mut fake = host();
        let host_ptr = &mut fake as *mut FakeHost as *mut c_void;
        let input = packed(&[(0.0, 0.5, 0.0)]);
        let ops = HirariAutomationHostOps {
            read_points: None,
            apply: Some(fake_apply),
            record_undo: Some(fake_record_undo),
        };
        assert!(!unsafe {
            edit_with(
                &ops,
                host_ptr,
                7,
                HIRARI_AUTOMATION_VOLUME,
                0,
                0,
                input.as_ptr(),
                input.len(),
            )
        });
        assert!(fake.undo.is_none());
    }
}
