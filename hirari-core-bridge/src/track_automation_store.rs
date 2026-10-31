//! Rust-owned per-track automation list storage.
//!
//! The track keeps only an opaque handle. Every published generation is kept
//! alive until the handle is destroyed, so an audio callback that already
//! loaded a list pointer can finish reading it, matching the retired-generation
//! contract the C++ storage had.

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::automation_capture_runtime::{
    AutomationRecorderState, TrackAutomationCapturePoint, TrackCaptureInner,
};

pub const KIND_VOLUME: u32 = 0;
pub const KIND_PAN: u32 = 1;
pub const KIND_TRACK_DELAY: u32 = 2;
const KIND_COUNT: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AutomationPoint {
    pub time: f64,
    pub value: f32,
    pub curve: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginLaneView {
    pub processor_index: u32,
    pub parameter_id: u32,
    pub points: *const AutomationPoint,
    pub point_count: usize,
}

#[derive(Clone)]
struct PluginLane {
    processor_index: u32,
    parameter_id: u32,
    points: Vec<AutomationPoint>,
}

struct LaneBundle {
    lanes: Vec<PluginLane>,
    views: Vec<PluginLaneView>,
}

impl LaneBundle {
    fn new(lanes: Vec<PluginLane>) -> Self {
        let views = lanes
            .iter()
            .map(|lane| PluginLaneView {
                processor_index: lane.processor_index,
                parameter_id: lane.parameter_id,
                points: lane.points.as_ptr(),
                point_count: lane.points.len(),
            })
            .collect();
        Self { lanes, views }
    }
}

struct Generation<T> {
    current: AtomicPtr<T>,
    history: Mutex<Vec<Box<T>>>,
}

impl<T> Generation<T> {
    fn new(value: T) -> Self {
        let mut initial = Box::new(value);
        let pointer = initial.as_mut() as *mut T;
        Self {
            current: AtomicPtr::new(pointer),
            history: Mutex::new(vec![initial]),
        }
    }

    fn publish(&self, value: T) {
        let mut next = Box::new(value);
        let pointer = next.as_mut() as *mut T;
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.push(next);
        self.current.store(pointer, Ordering::Release);
    }

    fn read(&self) -> *const T {
        self.current.load(Ordering::Acquire)
    }
}

fn sort_points(points: &mut [AutomationPoint]) {
    points.sort_by(|a, b| a.time.total_cmp(&b.time));
}

fn copy_points(points: *const AutomationPoint, count: usize) -> Option<Vec<AutomationPoint>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if points.is_null() {
        return None;
    }
    let mut copied = unsafe { std::slice::from_raw_parts(points, count) }.to_vec();
    sort_points(&mut copied);
    Some(copied)
}

pub struct TrackAutomationStore {
    capture: TrackCaptureInner,
    points: [Generation<Vec<AutomationPoint>>; KIND_COUNT],
    hints: [AtomicUsize; KIND_COUNT],
    lanes: Generation<LaneBundle>,
}

impl TrackAutomationStore {
    fn new() -> Self {
        Self {
            capture: TrackCaptureInner::new(),
            points: std::array::from_fn(|_| Generation::new(Vec::new())),
            hints: std::array::from_fn(|_| AtomicUsize::new(0)),
            lanes: Generation::new(LaneBundle::new(Vec::new())),
        }
    }

    fn points_snapshot(&self, kind: u32) -> Vec<AutomationPoint> {
        let slot = &self.points[kind as usize];
        let pointer = slot.read();
        if pointer.is_null() {
            Vec::new()
        } else {
            unsafe { &*pointer }.clone()
        }
    }

    fn publish_points(&self, kind: u32, points: Vec<AutomationPoint>) {
        self.points[kind as usize].publish(points);
    }

    fn lanes_snapshot(&self) -> Vec<PluginLane> {
        let pointer = self.lanes.read();
        if pointer.is_null() {
            Vec::new()
        } else {
            unsafe { &*pointer }.lanes.clone()
        }
    }
}

fn store_ref(store: *const c_void) -> Option<&'static TrackAutomationStore> {
    unsafe { store.cast::<TrackAutomationStore>().as_ref() }
}

#[no_mangle]
pub extern "C" fn hirari_track_automation_store_create() -> *mut c_void {
    Box::into_raw(Box::new(TrackAutomationStore::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_destroy(store: *mut c_void) {
    if !store.is_null() {
        unsafe { drop(Box::from_raw(store.cast::<TrackAutomationStore>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_record(
    store: *mut c_void,
    recorder: *const c_void,
    track_id: u32,
    sample_position: u64,
    volume: f32,
    pan: f32,
) -> bool {
    let (Some(store), Some(recorder)) = (
        store_ref(store),
        unsafe { recorder.cast::<AutomationRecorderState>().as_ref() },
    ) else {
        return false;
    };
    store
        .capture
        .record(track_id, sample_position, volume, pan, recorder)
}

/// Drains the captured points and appends them to the published volume/pan
/// lists. Returns the number of captured points that were merged.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_flush_recorded(
    store: *mut c_void,
) -> u32 {
    let Some(store) = store_ref(store) else {
        return 0;
    };
    let pending = store.capture.pending();
    if pending == 0 {
        return 0;
    }
    let mut captured = vec![TrackAutomationCapturePoint::default(); pending];
    let Some(count) = store.capture.flush_into(&mut captured) else {
        return 0;
    };
    let mut volume = store.points_snapshot(KIND_VOLUME);
    let mut pan = store.points_snapshot(KIND_PAN);
    volume.reserve(count);
    pan.reserve(count);
    for point in &captured[..count] {
        volume.push(AutomationPoint {
            time: point.time,
            value: point.volume,
            curve: 0.0,
        });
        pan.push(AutomationPoint {
            time: point.time,
            value: point.pan,
            curve: 0.0,
        });
    }
    sort_points(&mut volume);
    sort_points(&mut pan);
    store.publish_points(KIND_VOLUME, volume);
    store.publish_points(KIND_PAN, pan);
    count.min(u32::MAX as usize) as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_set_points(
    store: *mut c_void,
    kind: u32,
    points: *const AutomationPoint,
    count: usize,
) {
    let Some(store) = store_ref(store) else {
        return;
    };
    if kind as usize >= KIND_COUNT {
        return;
    }
    let Some(copied) = copy_points(points, count) else {
        return;
    };
    store.publish_points(kind, copied);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_points(
    store: *const c_void,
    kind: u32,
    count: *mut usize,
) -> *const AutomationPoint {
    let Some(count) = (unsafe { count.as_mut() }) else {
        return std::ptr::null();
    };
    *count = 0;
    let Some(store) = store_ref(store) else {
        return std::ptr::null();
    };
    if kind as usize >= KIND_COUNT {
        return std::ptr::null();
    }
    let pointer = store.points[kind as usize].read();
    if pointer.is_null() {
        return std::ptr::null();
    }
    let points = unsafe { &*pointer };
    *count = points.len();
    points.as_ptr()
}

/// Address of the published hint index used by the realtime evaluator. The
/// audio thread is the only writer, exactly like the C++ `size_t` member.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_hint(
    store: *const c_void,
    kind: u32,
) -> *mut usize {
    let Some(store) = store_ref(store) else {
        return std::ptr::null_mut();
    };
    if kind as usize >= KIND_COUNT {
        return std::ptr::null_mut();
    }
    store.hints[kind as usize].as_ptr()
}

/// Offline renders restart every evaluation cursor so the first block of a
/// bounce searches from the beginning of each list.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_reset_hints(store: *const c_void) {
    let Some(store) = store_ref(store) else {
        return;
    };
    for hint in &store.hints {
        hint.store(0, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_set_plugin_lanes(
    store: *mut c_void,
    lanes: *const PluginLaneView,
    count: usize,
) {
    let Some(store) = store_ref(store) else {
        return;
    };
    if count != 0 && lanes.is_null() {
        return;
    }
    let views = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(lanes, count) }
    };
    let mut copied = Vec::with_capacity(count);
    for view in views {
        copied.push(PluginLane {
            processor_index: view.processor_index,
            parameter_id: view.parameter_id,
            points: copy_points(view.points, view.point_count).unwrap_or_default(),
        });
    }
    store.lanes.publish(LaneBundle::new(copied));
}

/// Replaces (or removes, for an empty list) the lane that targets one plugin
/// parameter. The merge policy lives here so callers never rebuild the list.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_replace_plugin_lane(
    store: *mut c_void,
    plugin_index: u32,
    parameter_id: u32,
    points: *const AutomationPoint,
    count: usize,
) {
    let Some(store) = store_ref(store) else {
        return;
    };
    let Some(copied) = copy_points(points, count) else {
        return;
    };
    let mut lanes = store.lanes_snapshot();
    lanes.retain(|lane| {
        lane.processor_index != plugin_index || lane.parameter_id != parameter_id
    });
    if !copied.is_empty() {
        lanes.push(PluginLane {
            processor_index: plugin_index,
            parameter_id,
            points: copied,
        });
    }
    store.lanes.publish(LaneBundle::new(lanes));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_plugin_lane_views(
    store: *const c_void,
    count: *mut usize,
) -> *const PluginLaneView {
    let Some(count) = (unsafe { count.as_mut() }) else {
        return std::ptr::null();
    };
    *count = 0;
    let Some(store) = store_ref(store) else {
        return std::ptr::null();
    };
    let pointer = store.lanes.read();
    if pointer.is_null() {
        return std::ptr::null();
    }
    let bundle = unsafe { &*pointer };
    *count = bundle.views.len();
    bundle.views.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_plugin_lane_count(
    store: *const c_void,
) -> usize {
    let Some(store) = store_ref(store) else {
        return 0;
    };
    let pointer = store.lanes.read();
    if pointer.is_null() {
        0
    } else {
        unsafe { &*pointer }.lanes.len()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_automation_store_plugin_lane(
    store: *const c_void,
    index: usize,
    plugin_index: *mut u32,
    parameter_id: *mut u32,
    count: *mut usize,
) -> *const AutomationPoint {
    let Some(count) = (unsafe { count.as_mut() }) else {
        return std::ptr::null();
    };
    *count = 0;
    let Some(store) = store_ref(store) else {
        return std::ptr::null();
    };
    let pointer = store.lanes.read();
    if pointer.is_null() {
        return std::ptr::null();
    }
    let bundle = unsafe { &*pointer };
    let Some(lane) = bundle.lanes.get(index) else {
        return std::ptr::null();
    };
    if let Some(plugin_index) = unsafe { plugin_index.as_mut() } {
        *plugin_index = lane.processor_index;
    }
    if let Some(parameter_id) = unsafe { parameter_id.as_mut() } {
        *parameter_id = lane.parameter_id;
    }
    *count = lane.points.len();
    lane.points.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_points_sorts_and_publishes_the_latest_generation() {
        let store = hirari_track_automation_store_create();
        let points = [
            AutomationPoint { time: 20.0, value: 0.2, curve: 0.0 },
            AutomationPoint { time: 10.0, value: 0.1, curve: 0.0 },
        ];
        unsafe {
            hirari_track_automation_store_set_points(
                store, KIND_VOLUME, points.as_ptr(), points.len());
        }
        let mut count = 0usize;
        let published =
            unsafe { hirari_track_automation_store_points(store, KIND_VOLUME, &mut count) };
        let slice = unsafe { std::slice::from_raw_parts(published, count) };
        assert_eq!(slice.len(), 2);
        assert_eq!(slice[0].time, 10.0);
        assert_eq!(slice[1].value, 0.2);
        unsafe { hirari_track_automation_store_destroy(store) };
    }

    #[test]
    fn replace_plugin_lane_merges_and_removes_by_key() {
        let store = hirari_track_automation_store_create();
        let points = [AutomationPoint { time: 1.0, value: 0.5, curve: 0.0 }];
        let other = [AutomationPoint { time: 2.0, value: 0.25, curve: 0.0 }];
        unsafe {
            hirari_track_automation_store_replace_plugin_lane(
                store, 1, 7, points.as_ptr(), points.len());
            hirari_track_automation_store_replace_plugin_lane(
                store, 2, 9, other.as_ptr(), other.len());
            hirari_track_automation_store_replace_plugin_lane(
                store, 1, 7, other.as_ptr(), other.len());
        }
        assert_eq!(
            unsafe { hirari_track_automation_store_plugin_lane_count(store) },
            2
        );
        // Replacing a lane appends it, exactly like the C++ erase-then-push
        // merge that lived in the engine before.
        let mut plugin_index = 0u32;
        let mut parameter_id = 0u32;
        let mut count = 0usize;
        let published = unsafe {
            hirari_track_automation_store_plugin_lane(
                store, 1, &mut plugin_index, &mut parameter_id, &mut count)
        };
        assert_eq!((plugin_index, parameter_id, count), (1, 7, 1));
        assert_eq!(unsafe { *published }.value, 0.25);

        unsafe {
            hirari_track_automation_store_replace_plugin_lane(
                store, 1, 7, std::ptr::null(), 0);
        }
        assert_eq!(
            unsafe { hirari_track_automation_store_plugin_lane_count(store) },
            1
        );
        unsafe { hirari_track_automation_store_destroy(store) };
    }

    #[test]
    fn flush_recorded_appends_captured_volume_and_pan_points() {
        let store = hirari_track_automation_store_create();
        let recorder = crate::automation_capture_runtime::hirari_automation_recorder_instance();
        unsafe {
            crate::automation_capture_runtime::hirari_automation_recorder_set_mode(recorder, 1);
            assert!(hirari_track_automation_store_record(
                store, recorder, 3, 100, 0.5, -0.25));
            assert_eq!(hirari_track_automation_store_flush_recorded(store), 1);
        }
        let mut volume_count = 0usize;
        let mut pan_count = 0usize;
        let volume = unsafe {
            hirari_track_automation_store_points(store, KIND_VOLUME, &mut volume_count)
        };
        let pan =
            unsafe { hirari_track_automation_store_points(store, KIND_PAN, &mut pan_count) };
        assert_eq!(volume_count, 1);
        assert_eq!(pan_count, 1);
        assert_eq!(unsafe { *volume }.value, 0.5);
        assert_eq!(unsafe { *pan }.value, -0.25);
        unsafe {
            crate::automation_capture_runtime::hirari_automation_recorder_set_mode(recorder, 0);
            hirari_track_automation_store_destroy(store);
        }
    }
}
