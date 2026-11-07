//! Rust-owned Control Room model and realtime publication state.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

const MAX_CUES: usize = 32;

#[derive(Clone)]
struct SpeakerSet {
    name: Vec<u8>,
    gain: f32,
    enabled: bool,
}

#[derive(Clone, Copy)]
struct CueMix {
    id: u32,
    gain: f32,
    enabled: bool,
    bus_track_id: u32,
    output_channel: u32,
    click_enabled: bool,
}

struct Model {
    speakers: Vec<SpeakerSet>,
    cues: Vec<CueMix>,
    active_speaker: usize,
    dim_db: f32,
    dim_gain: f32,
    talkback_gain: f32,
    dim: bool,
    talkback: bool,
}

impl Model {
    fn new() -> Self {
        Self {
            speakers: vec![SpeakerSet {
                name: b"Main".to_vec(),
                gain: 1.0,
                enabled: true,
            }],
            cues: Vec::new(),
            active_speaker: 0,
            dim_db: -20.0,
            dim_gain: 0.1,
            talkback_gain: 1.0,
            dim: false,
            talkback: false,
        }
    }
}

pub struct ControlRoomRuntime {
    model: Mutex<Model>,
    cue_bus_ids: [AtomicU32; MAX_CUES],
    active_cue_id: AtomicU32,
    active_cue_bus_track_id: AtomicU32,
    active_cue_gain: AtomicU32,
    active_cue_output_channel: AtomicU32,
    active_cue_click_enabled: AtomicBool,
    monitor_gain: AtomicU32,
    talkback_gain: AtomicU32,
    talkback_input_channel: AtomicU32,
    talkback_enabled: AtomicBool,
}

impl ControlRoomRuntime {
    fn new() -> Self {
        let state = Self {
            model: Mutex::new(Model::new()),
            cue_bus_ids: std::array::from_fn(|_| AtomicU32::new(0)),
            active_cue_id: AtomicU32::new(0),
            active_cue_bus_track_id: AtomicU32::new(0),
            active_cue_gain: AtomicU32::new(0.0_f32.to_bits()),
            active_cue_output_channel: AtomicU32::new(0),
            active_cue_click_enabled: AtomicBool::new(false),
            monitor_gain: AtomicU32::new(0.0_f32.to_bits()),
            talkback_gain: AtomicU32::new(1.0_f32.to_bits()),
            talkback_input_channel: AtomicU32::new(0),
            talkback_enabled: AtomicBool::new(false),
        };
        {
            let model = state.lock_model();
            state.publish_monitor_gain(&model);
            state.publish_cue_state(&model);
        }
        state
    }

    fn lock_model(&self) -> std::sync::MutexGuard<'_, Model> {
        self.model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn publish_monitor_gain(&self, model: &Model) {
        let gain = model
            .speakers
            .get(model.active_speaker)
            .filter(|speaker| speaker.enabled)
            .map_or(0.0, |speaker| {
                speaker.gain * if model.dim { model.dim_gain } else { 1.0 }
            });
        self.monitor_gain.store(gain.to_bits(), Ordering::Release);
    }

    fn publish_cue_state(&self, model: &Model) {
        for (index, published) in self.cue_bus_ids.iter().enumerate() {
            published.store(
                model.cues.get(index).map_or(0, |cue| cue.bus_track_id),
                Ordering::Release,
            );
        }
        let active = self.active_cue_id.load(Ordering::Relaxed);
        let selected = model
            .cues
            .iter()
            .find(|cue| cue.id == active && cue.enabled && cue.bus_track_id != 0);
        self.active_cue_bus_track_id.store(
            selected.map_or(0, |cue| cue.bus_track_id),
            Ordering::Release,
        );
        self.active_cue_gain.store(
            selected.map_or(0.0, |cue| cue.gain).to_bits(),
            Ordering::Release,
        );
        self.active_cue_output_channel.store(
            selected.map_or(0, |cue| cue.output_channel),
            Ordering::Release,
        );
        self.active_cue_click_enabled.store(
            selected.is_some_and(|cue| cue.click_enabled),
            Ordering::Release,
        );
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpeakerSnapshot {
    pub name: [u8; 128],
    pub name_length: u32,
    pub gain: f32,
    pub enabled: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CueSnapshot {
    pub id: u32,
    pub gain: f32,
    pub enabled: u8,
    pub bus_track_id: u32,
    pub output_channel: u32,
    pub click_enabled: u8,
}

unsafe fn state(handle: *mut c_void) -> Option<&'static ControlRoomRuntime> {
    if handle.is_null() {
        None
    } else {
        Some(unsafe { &*handle.cast::<ControlRoomRuntime>() })
    }
}

fn valid_gain(gain: f32) -> bool {
    gain.is_finite() && (0.0..=4.0).contains(&gain)
}
fn valid_speaker_name(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.contains(&0)
}
fn valid_output_channel(channel: u32) -> bool {
    channel <= 30 && (channel == 0 || channel & 1 == 0)
}

#[no_mangle]
pub extern "C" fn hirari_control_room_state_create() -> *mut c_void {
    Box::into_raw(Box::new(ControlRoomRuntime::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_state_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle.cast::<ControlRoomRuntime>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_reset(handle: *mut c_void) {
    let Some(state) = (unsafe { state(handle) }) else {
        return;
    };
    let mut model = state.lock_model();
    *model = Model::new();
    state.active_cue_id.store(0, Ordering::Release);
    state.talkback_input_channel.store(0, Ordering::Release);
    state.talkback_enabled.store(false, Ordering::Release);
    state
        .talkback_gain
        .store(1.0_f32.to_bits(), Ordering::Release);
    state.publish_cue_state(&model);
    state.publish_monitor_gain(&model);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_add_speaker(
    handle: *mut c_void,
    name: *const u8,
    name_length: usize,
    gain: f32,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if name.is_null() || name_length == 0 || name_length > 128 || !valid_gain(gain) {
        return false;
    }
    let name = unsafe { std::slice::from_raw_parts(name, name_length) };
    if !valid_speaker_name(name) {
        return false;
    }
    let mut model = state.lock_model();
    model.speakers.push(SpeakerSet {
        name: name.to_vec(),
        gain,
        enabled: true,
    });
    if model.active_speaker >= model.speakers.len() {
        model.active_speaker = model.speakers.len() - 1;
    }
    state.publish_monitor_gain(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_select_speaker(
    handle: *mut c_void,
    index: usize,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    if index >= model.speakers.len() {
        return false;
    }
    model.active_speaker = index;
    state.publish_monitor_gain(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_remove_speaker(
    handle: *mut c_void,
    index: usize,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    if index >= model.speakers.len() || model.speakers.len() <= 1 {
        return false;
    }
    model.speakers.remove(index);
    if model.active_speaker > index {
        model.active_speaker -= 1;
    } else if model.active_speaker >= model.speakers.len() {
        model.active_speaker = model.speakers.len() - 1;
    }
    state.publish_monitor_gain(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_rename_speaker(
    handle: *mut c_void,
    index: usize,
    name: *const u8,
    name_length: usize,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if name.is_null() || name_length == 0 || name_length > 128 {
        return false;
    }
    let name = unsafe { std::slice::from_raw_parts(name, name_length) };
    if !valid_speaker_name(name) {
        return false;
    }
    let mut model = state.lock_model();
    let Some(speaker) = model.speakers.get_mut(index) else {
        return false;
    };
    speaker.name = name.to_vec();
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_speaker_gain(
    handle: *mut c_void,
    index: usize,
    gain: f32,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if !valid_gain(gain) {
        return false;
    }
    let mut model = state.lock_model();
    let Some(speaker) = model.speakers.get_mut(index) else {
        return false;
    };
    speaker.gain = gain;
    state.publish_monitor_gain(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_speaker_enabled(
    handle: *mut c_void,
    index: usize,
    enabled: bool,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    let Some(speaker) = model.speakers.get_mut(index) else {
        return false;
    };
    speaker.enabled = enabled;
    state.publish_monitor_gain(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_dim(handle: *mut c_void, enabled: bool) {
    let Some(state) = (unsafe { state(handle) }) else {
        return;
    };
    let mut model = state.lock_model();
    model.dim = enabled;
    state.publish_monitor_gain(&model);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_dim_db(handle: *mut c_void, db: f32) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if !db.is_finite() || !(-60.0..=0.0).contains(&db) {
        return false;
    }
    let mut model = state.lock_model();
    model.dim_db = db;
    model.dim_gain = 10.0_f32.powf(db / 20.0);
    state.publish_monitor_gain(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_dim_db(handle: *mut c_void) -> f32 {
    unsafe { state(handle) }.map_or(-20.0, |state| state.lock_model().dim_db)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_is_dimmed(handle: *mut c_void) -> bool {
    unsafe { state(handle) }.is_some_and(|state| state.lock_model().dim)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_talkback(
    handle: *mut c_void,
    enabled: bool,
    gain: f32,
) {
    let Some(state) = (unsafe { state(handle) }) else {
        return;
    };
    let mut model = state.lock_model();
    model.talkback = enabled;
    if gain.is_finite() {
        model.talkback_gain = gain.clamp(0.0, 4.0);
    }
    state.talkback_enabled.store(enabled, Ordering::Release);
    state
        .talkback_gain
        .store(model.talkback_gain.to_bits(), Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_talkback_enabled(handle: *const c_void) -> bool {
    let Some(state) = (unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }) else {
        return false;
    };
    state.talkback_enabled.load(Ordering::Acquire)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_talkback_channel(
    handle: *mut c_void,
    channel: u32,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if channel >= 32 {
        return false;
    }
    state
        .talkback_input_channel
        .store(channel, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_talkback_channel(handle: *const c_void) -> u32 {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }.map_or(0, |state| {
        state.talkback_input_channel.load(Ordering::Acquire)
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_monitor_gain(handle: *const c_void) -> f32 {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }.map_or(0.0, |state| {
        f32::from_bits(state.monitor_gain.load(Ordering::Acquire))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_process_monitor_state(
    handle: *const c_void,
    left: *mut f32,
    right: *mut f32,
    talkback: *const f32,
    frames: u32,
) {
    let Some(state) = (unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }) else {
        return;
    };
    unsafe {
        crate::control_room_audio::hirari_control_room_process_monitor(
            left,
            right,
            talkback,
            frames,
            f32::from_bits(state.monitor_gain.load(Ordering::Acquire)),
            f32::from_bits(state.talkback_gain.load(Ordering::Acquire)),
            !talkback.is_null() && state.talkback_enabled.load(Ordering::Acquire),
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_upsert_cue(
    handle: *mut c_void,
    id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if id == 0 || !valid_gain(gain) {
        return false;
    }
    let mut model = state.lock_model();
    if let Some(cue) = model.cues.iter_mut().find(|cue| cue.id == id) {
        cue.gain = gain;
        cue.enabled = enabled;
        if !enabled && state.active_cue_id.load(Ordering::Relaxed) == id {
            state.active_cue_id.store(0, Ordering::Release);
        }
    } else {
        if model.cues.len() >= MAX_CUES {
            return false;
        }
        model.cues.push(CueMix {
            id,
            gain,
            enabled,
            bus_track_id: 0,
            output_channel: 0,
            click_enabled: false,
        });
    }
    state.publish_cue_state(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_remove_cue(handle: *mut c_void, id: u32) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    let previous = model.cues.len();
    model.cues.retain(|cue| cue.id != id);
    if state.active_cue_id.load(Ordering::Relaxed) == id {
        state.active_cue_id.store(0, Ordering::Release);
    }
    state.publish_cue_state(&model);
    model.cues.len() != previous
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_cue_enabled(
    handle: *mut c_void,
    id: u32,
    enabled: bool,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    let Some(cue) = model.cues.iter_mut().find(|cue| cue.id == id) else {
        return false;
    };
    cue.enabled = enabled;
    if !enabled && state.active_cue_id.load(Ordering::Relaxed) == id {
        state.active_cue_id.store(0, Ordering::Release);
    }
    state.publish_cue_state(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_cue_bus(
    handle: *mut c_void,
    id: u32,
    bus_track_id: u32,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    if bus_track_id != 0
        && model
            .cues
            .iter()
            .any(|cue| cue.id != id && cue.bus_track_id == bus_track_id)
    {
        return false;
    }
    let Some(cue) = model.cues.iter_mut().find(|cue| cue.id == id) else {
        return false;
    };
    cue.bus_track_id = bus_track_id;
    if bus_track_id == 0 && state.active_cue_id.load(Ordering::Relaxed) == id {
        state.active_cue_id.store(0, Ordering::Release);
    }
    state.publish_cue_state(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_cue_output(
    handle: *mut c_void,
    id: u32,
    output_channel: u32,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    if !valid_output_channel(output_channel) {
        return false;
    }
    let mut model = state.lock_model();
    let Some(cue) = model.cues.iter_mut().find(|cue| cue.id == id) else {
        return false;
    };
    cue.output_channel = output_channel;
    state.publish_cue_state(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_set_cue_click(
    handle: *mut c_void,
    id: u32,
    enabled: bool,
) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let mut model = state.lock_model();
    let Some(cue) = model.cues.iter_mut().find(|cue| cue.id == id) else {
        return false;
    };
    cue.click_enabled = enabled;
    state.publish_cue_state(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_select_cue(handle: *mut c_void, id: u32) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let model = state.lock_model();
    if id != 0
        && !model
            .cues
            .iter()
            .any(|cue| cue.id == id && cue.enabled && cue.bus_track_id != 0)
    {
        return false;
    }
    state.active_cue_id.store(id, Ordering::Release);
    state.publish_cue_state(&model);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_active_cue_id(handle: *const c_void) -> u32 {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }
        .map_or(0, |s| s.active_cue_id.load(Ordering::Acquire))
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_active_cue_bus(handle: *const c_void) -> u32 {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }
        .map_or(0, |s| s.active_cue_bus_track_id.load(Ordering::Acquire))
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_cue_bus(handle: *mut c_void, id: u32) -> u32 {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    state
        .lock_model()
        .cues
        .iter()
        .find(|cue| cue.id == id)
        .map_or(0, |cue| cue.bus_track_id)
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_active_cue_gain(handle: *const c_void) -> f32 {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }.map_or(0.0, |s| {
        f32::from_bits(s.active_cue_gain.load(Ordering::Acquire))
    })
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_active_cue_output(handle: *const c_void) -> u32 {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }
        .map_or(0, |s| s.active_cue_output_channel.load(Ordering::Acquire))
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_active_cue_click(handle: *const c_void) -> bool {
    unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }
        .is_some_and(|s| s.active_cue_click_enabled.load(Ordering::Acquire))
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_is_cue_bus(handle: *const c_void, id: u32) -> bool {
    let Some(state) = (unsafe { handle.cast::<ControlRoomRuntime>().as_ref() }) else {
        return false;
    };
    id != 0
        && state
            .cue_bus_ids
            .iter()
            .any(|cue_id| cue_id.load(Ordering::Acquire) == id)
}
#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_cue_gain(handle: *mut c_void, id: u32) -> f32 {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0.0;
    };
    state
        .lock_model()
        .cues
        .iter()
        .find(|cue| cue.id == id)
        .map_or(0.0, |cue| if cue.enabled { cue.gain } else { 0.0 })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_validate(handle: *mut c_void) -> bool {
    let Some(state) = (unsafe { state(handle) }) else {
        return false;
    };
    let model = state.lock_model();
    if model.speakers.is_empty()
        || model.active_speaker >= model.speakers.len()
        || model.cues.len() > MAX_CUES
    {
        return false;
    }
    if model
        .speakers
        .iter()
        .any(|s| !valid_speaker_name(&s.name) || !valid_gain(s.gain))
    {
        return false;
    }
    for (index, cue) in model.cues.iter().enumerate() {
        if cue.id == 0 || !valid_gain(cue.gain) || !valid_output_channel(cue.output_channel) {
            return false;
        }
        if model.cues[..index].iter().any(|prev| {
            prev.id == cue.id || (cue.bus_track_id != 0 && prev.bus_track_id == cue.bus_track_id)
        }) {
            return false;
        }
    }
    let active = state.active_cue_id.load(Ordering::Relaxed);
    active == 0
        || model
            .cues
            .iter()
            .any(|cue| cue.id == active && cue.enabled && cue.bus_track_id != 0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_active_speaker(handle: *mut c_void) -> usize {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    state.lock_model().active_speaker
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_speaker_snapshot(
    handle: *mut c_void,
    output: *mut SpeakerSnapshot,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    let model = state.lock_model();
    if output.is_null() {
        return model.speakers.len();
    }
    let count = model.speakers.len().min(capacity);
    for index in 0..count {
        let speaker = &model.speakers[index];
        let mut snapshot = SpeakerSnapshot {
            name: [0; 128],
            name_length: speaker.name.len() as u32,
            gain: speaker.gain,
            enabled: u8::from(speaker.enabled),
        };
        snapshot.name[..speaker.name.len()].copy_from_slice(&speaker.name);
        unsafe {
            output.add(index).write(snapshot);
        }
    }
    count
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_cue_snapshot(
    handle: *mut c_void,
    output: *mut CueSnapshot,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state(handle) }) else {
        return 0;
    };
    let model = state.lock_model();
    if output.is_null() {
        return model.cues.len();
    }
    let count = model.cues.len().min(capacity);
    for index in 0..count {
        let cue = model.cues[index];
        unsafe {
            output.add(index).write(CueSnapshot {
                id: cue.id,
                gain: cue.gain,
                enabled: u8::from(cue.enabled),
                bus_track_id: cue.bus_track_id,
                output_channel: cue.output_channel,
                click_enabled: u8::from(cue.click_enabled),
            });
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_monitor_and_talkback_state_publish_to_audio_path() {
        let state = hirari_control_room_state_create();
        let room = b"Booth A";
        unsafe {
            assert_eq!(
                hirari_control_room_speaker_snapshot(state, std::ptr::null_mut(), 0),
                1
            );
            assert!(hirari_control_room_add_speaker(
                state,
                room.as_ptr(),
                room.len(),
                1.5
            ));
            assert!(hirari_control_room_select_speaker(state, 1));
            assert!(hirari_control_room_set_dim_db(state, -6.0));
            hirari_control_room_set_dim(state, true);
            assert!(hirari_control_room_is_dimmed(state));
            hirari_control_room_set_talkback(state, true, 2.0);
            assert!(hirari_control_room_talkback_enabled(state));
            assert!(hirari_control_room_set_talkback_channel(state, 3));
            assert_eq!(hirari_control_room_talkback_channel(state), 3);
        }
        let expected_monitor = 1.5_f32 * 10.0_f32.powf(-6.0 / 20.0);
        assert!(
            (unsafe { hirari_control_room_monitor_gain(state) } - expected_monitor).abs() < 1.0e-6
        );
        let input_l = [0.2_f32];
        let input_r = [-0.2_f32];
        let talkback = [0.1_f32];
        let mut output_l = input_l;
        let mut output_r = input_r;
        unsafe {
            hirari_control_room_process_monitor_state(
                state,
                output_l.as_mut_ptr(),
                output_r.as_mut_ptr(),
                talkback.as_ptr(),
                1,
            );
        }
        assert!((output_l[0] - (0.2 * expected_monitor + 0.2)).abs() < 1.0e-6);
        assert!((output_r[0] - (-0.2 * expected_monitor + 0.2)).abs() < 1.0e-6);

        let mut speakers = [SpeakerSnapshot {
            name: [0; 128],
            name_length: 0,
            gain: 0.0,
            enabled: 0,
        }; 2];
        unsafe {
            assert_eq!(
                hirari_control_room_speaker_snapshot(state, speakers.as_mut_ptr(), speakers.len()),
                2
            );
        }
        assert_eq!(&speakers[1].name[..speakers[1].name_length as usize], room);
        unsafe { hirari_control_room_state_destroy(state) };
    }

    #[test]
    fn cue_state_validation_and_project_reset_match_contract() {
        let state = hirari_control_room_state_create();
        unsafe {
            assert!(hirari_control_room_upsert_cue(state, 7, 0.75, true));
            assert!(hirari_control_room_set_cue_bus(state, 7, 42));
            assert!(hirari_control_room_set_cue_output(state, 7, 2));
            assert!(hirari_control_room_set_cue_click(state, 7, true));
            assert!(hirari_control_room_select_cue(state, 7));
            assert!(hirari_control_room_validate(state));
            assert_eq!(hirari_control_room_active_cue_id(state), 7);
            assert_eq!(hirari_control_room_active_cue_bus(state), 42);
            assert_eq!(hirari_control_room_active_cue_output(state), 2);
            assert!(hirari_control_room_active_cue_click(state));
            assert!(hirari_control_room_is_cue_bus(state, 42));
            assert!(!hirari_control_room_upsert_cue(state, 8, f32::NAN, true));
            assert!(hirari_control_room_upsert_cue(state, 8, 1.0, true));
            assert!(!hirari_control_room_set_cue_bus(state, 8, 42));
            assert!(hirari_control_room_remove_cue(state, 7));
            assert_eq!(hirari_control_room_active_cue_id(state), 0);
            hirari_control_room_reset(state);
            assert!(hirari_control_room_validate(state));
            assert_eq!(hirari_control_room_active_cue_id(state), 0);
            assert_eq!(hirari_control_room_monitor_gain(state), 1.0);
            hirari_control_room_state_destroy(state);
        }
    }
}
