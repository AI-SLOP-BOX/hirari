//! Realtime MIDI transformations shared with the native compatibility layer.
//!
//! This module owns MPE note state and the event transformation rules. The
//! C++ adapter keeps only the existing realtime queue and publication API.

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

const MAX_EVENTS_PER_BLOCK: usize = 1024;
const MAX_ARTICULATION_MAPS: usize = 64;
const MAX_SYSEX_BYTES: usize = 4096;

#[repr(C)]
struct MidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ArticulationMap {
    pub id: u32,
    pub name: [u8; 64],
    pub trigger_channel: u32,
}

struct ArticulationSlot {
    id: AtomicU32,
    trigger_channel: AtomicU32,
}

impl ArticulationSlot {
    fn new() -> Self {
        Self {
            id: AtomicU32::new(0),
            trigger_channel: AtomicU32::new(0),
        }
    }
}

struct SysExMessage {
    manufacturer_id: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct MpeNote {
    active: bool,
    note_number: u8,
    pressure: f32,
    timbre: f32,
    bend: f32,
}

/// Per-orchestrator state. It is created and destroyed on the control side;
/// processing mutates it only on the audio thread.
pub struct MpeState {
    enabled: AtomicBool,
    reset_requested: AtomicBool,
    notes: UnsafeCell<[MpeNote; 16]>,
    articulation_maps: [Box<[ArticulationSlot]>; 2],
    articulation_counts: [AtomicUsize; 2],
    active_articulation_map: AtomicUsize,
    sysex_queue: Mutex<VecDeque<SysExMessage>>,
}

// The event state has one audio-thread owner. Control threads touch only the
// atomics above; reset requests are applied by the audio thread before MIDI
// processing begins.
unsafe impl Send for MpeState {}
unsafe impl Sync for MpeState {}

impl Default for MpeState {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            reset_requested: AtomicBool::new(false),
            notes: UnsafeCell::new([MpeNote::default(); 16]),
            articulation_maps: [
                (0..MAX_ARTICULATION_MAPS)
                    .map(|_| ArticulationSlot::new())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                (0..MAX_ARTICULATION_MAPS)
                    .map(|_| ArticulationSlot::new())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ],
            articulation_counts: [AtomicUsize::new(0), AtomicUsize::new(0)],
            active_articulation_map: AtomicUsize::new(0),
            sysex_queue: Mutex::new(VecDeque::with_capacity(32)),
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_mpe_state_create() -> *mut MpeState {
    Box::into_raw(Box::new(MpeState::default()))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_state_free(state: *mut MpeState) {
    if !state.is_null() {
        drop(Box::from_raw(state));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_state_reset(state: *mut MpeState) {
    if let Some(state) = state.as_ref() {
        state.enabled.store(false, Ordering::Release);
        state.reset_requested.store(true, Ordering::Release);
        for count in &state.articulation_counts {
            count.store(0, Ordering::Release);
        }
        state.active_articulation_map.store(0, Ordering::Release);
        state
            .sysex_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_state_set_enabled(state: *mut MpeState, enabled: bool) {
    if let Some(state) = state.as_ref() {
        state.enabled.store(enabled, Ordering::Release);
        state.reset_requested.store(true, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_state_is_enabled(state: *const MpeState) -> bool {
    state
        .as_ref()
        .is_some_and(|state| state.enabled.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_process_mpe(
    state: *mut MpeState,
    events: *const c_void,
    event_count: usize,
) {
    let Some(state) = state.as_ref() else {
        return;
    };
    if state.reset_requested.swap(false, Ordering::AcqRel) {
        *state.notes.get() = [MpeNote::default(); 16];
    }
    if !state.enabled.load(Ordering::Acquire) {
        return;
    }
    if events.is_null() || event_count == 0 {
        return;
    }

    // MidiBuffer has a fixed 1024-event capacity. Clamp the FFI boundary too,
    // so malformed native callers cannot create an unbounded slice.
    let events = slice::from_raw_parts(
        events.cast::<MidiEvent>(),
        event_count.min(MAX_EVENTS_PER_BLOCK),
    );
    // SAFETY: this state is mutated only by the audio thread. Other entry
    // points write the atomic reset/enabled controls and never touch notes.
    let notes = &mut *state.notes.get();
    for event in events {
        let size = event.size as usize;
        if size == 0 {
            continue;
        }
        let status = event.data[0] & 0xf0;
        let channel = (event.data[0] & 0x0f) as usize;
        if status == 0xd0 {
            if size < 2 {
                continue;
            }
        } else if size < 3 {
            continue;
        }

        match status {
            0x90 if event.data[2] > 0 => {
                notes[channel] = MpeNote {
                    active: true,
                    note_number: event.data[1],
                    ..MpeNote::default()
                };
            }
            0x80 | 0x90 => notes[channel].active = false,
            0xd0 if notes[channel].active => {
                notes[channel].pressure = event.data[1] as f32 / 127.0;
            }
            0xa0 if notes[channel].active && event.data[1] == notes[channel].note_number => {
                notes[channel].pressure = event.data[2] as f32 / 127.0;
            }
            0xb0 if event.data[1] == 74 && notes[channel].active => {
                notes[channel].timbre = event.data[2] as f32 / 127.0;
            }
            0xe0 if notes[channel].active => {
                let bend = ((event.data[2] as u16) << 7) | event.data[1] as u16;
                notes[channel].bend = (bend as i32 - 8192) as f32 / 8192.0;
            }
            _ => {}
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_apply_articulations(
    events: *mut c_void,
    event_count: usize,
    maps: *const c_void,
    map_count: usize,
) {
    if events.is_null() || maps.is_null() || event_count == 0 || map_count == 0 {
        return;
    }

    let events = slice::from_raw_parts_mut(
        events.cast::<MidiEvent>(),
        event_count.min(MAX_EVENTS_PER_BLOCK),
    );
    let maps = slice::from_raw_parts(
        maps.cast::<ArticulationMap>(),
        map_count.min(MAX_ARTICULATION_MAPS),
    );
    for event in events {
        if event.size < 3 || event.articulation_id == 0 {
            continue;
        }
        let Some(map) = maps.iter().find(|map| {
            map.id == event.articulation_id as u32
                && map.trigger_channel > 0
                && map.trigger_channel <= 16
        }) else {
            continue;
        };

        let status = event.data[0] & 0xf0;
        if status == 0x90 || status == 0x80 {
            event.data[0] = status | (map.trigger_channel as u8 - 1);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_set_articulation_map(
    state: *mut MpeState,
    maps: *const ArticulationMap,
    map_count: usize,
) {
    let Some(state) = (unsafe { state.as_ref() }) else {
        return;
    };
    let map_count = map_count.min(MAX_ARTICULATION_MAPS);
    if map_count != 0 && maps.is_null() {
        return;
    }
    // A writer may only publish into the inactive fixed table. Atomic slots
    // also make repeated rapid control updates safe if an audio block still
    // holds a snapshot of the former table.
    let active = state.active_articulation_map.load(Ordering::Acquire);
    let next = 1 - active;
    let table = &state.articulation_maps[next];
    if map_count != 0 {
        let input = unsafe { slice::from_raw_parts(maps, map_count) };
        for (slot, map) in table.iter().zip(input) {
            slot.id.store(map.id, Ordering::Relaxed);
            slot.trigger_channel
                .store(map.trigger_channel, Ordering::Relaxed);
        }
    }
    state.articulation_counts[next].store(map_count, Ordering::Relaxed);
    state.active_articulation_map.store(next, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_apply_articulations_from_state(
    state: *const MpeState,
    events: *mut c_void,
    event_count: usize,
) {
    let Some(state) = (unsafe { state.as_ref() }) else {
        return;
    };
    if events.is_null() {
        return;
    }
    if event_count == 0 {
        return;
    }
    let map_index = state.active_articulation_map.load(Ordering::Acquire);
    let map_count = state.articulation_counts[map_index].load(Ordering::Acquire);
    if map_count == 0 {
        return;
    }
    let events = unsafe {
        slice::from_raw_parts_mut(
            events.cast::<MidiEvent>(),
            event_count.min(MAX_EVENTS_PER_BLOCK),
        )
    };
    let maps = &state.articulation_maps[map_index];
    for event in events {
        if event.size < 3 || event.articulation_id == 0 {
            continue;
        }
        for map in maps.iter().take(map_count) {
            if map.id.load(Ordering::Relaxed) != event.articulation_id as u32 {
                continue;
            }
            let trigger_channel = map.trigger_channel.load(Ordering::Relaxed);
            if trigger_channel == 0 || trigger_channel > 16 {
                break;
            }
            let status = event.data[0] & 0xf0;
            if status == 0x90 || status == 0x80 {
                event.data[0] = status | (trigger_channel as u8 - 1);
            }
            break;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_sysex_send(
    state: *mut MpeState,
    manufacturer_id: u32,
    data: *const u8,
    size: usize,
) -> bool {
    let Some(state) = (unsafe { state.as_ref() }) else {
        return false;
    };
    if data.is_null() || !(1..=MAX_SYSEX_BYTES).contains(&size) {
        return false;
    }
    let mut queue = state
        .sysex_queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if queue.len() >= 32 {
        return false;
    }
    let data = unsafe { slice::from_raw_parts(data, size) }.to_vec();
    queue.push_back(SysExMessage {
        manufacturer_id,
        data,
    });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_sysex_incoming(
    state: *mut MpeState,
    data: *const u8,
    size: usize,
) -> bool {
    if !unsafe { hirari_midi_sysex_is_valid(data, size) } {
        return false;
    }
    let manufacturer_id = unsafe { hirari_midi_sysex_manufacturer_id(data, size) };
    unsafe { hirari_mpe_sysex_send(state, manufacturer_id, data, size) }
}

/// If capacity is too small, returns the required length without removing the
/// front message. Only the non-audio SysEx consumer may call this operation.
#[no_mangle]
pub unsafe extern "C" fn hirari_mpe_sysex_pop(
    state: *mut MpeState,
    manufacturer_id: *mut u32,
    output: *mut u8,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 0;
    };
    let mut queue = state
        .sysex_queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(message) = queue.front() else {
        return 0;
    };
    if manufacturer_id.is_null() || output.is_null() || capacity < message.data.len() {
        return message.data.len();
    }
    unsafe {
        manufacturer_id.write(message.manufacturer_id);
        std::ptr::copy_nonoverlapping(message.data.as_ptr(), output, message.data.len());
    }
    let size = message.data.len();
    queue.pop_front();
    size
}

#[no_mangle]
pub extern "C" fn hirari_midi_sysex_size_is_valid(size: usize) -> bool {
    (1..=MAX_SYSEX_BYTES).contains(&size)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sysex_is_valid(data: *const u8, size: usize) -> bool {
    if data.is_null() || !(3..=MAX_SYSEX_BYTES).contains(&size) {
        return false;
    }
    let data = slice::from_raw_parts(data, size);
    data[0] == 0xf0 && data[size - 1] == 0xf7
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_sysex_manufacturer_id(data: *const u8, size: usize) -> u32 {
    if !hirari_midi_sysex_is_valid(data, size) {
        return 0;
    }
    let data = slice::from_raw_parts(data, size);
    if data[1] == 0 && size >= 5 {
        ((data[2] as u32) << 8) | data[3] as u32
    } else {
        data[1] as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(status: u8, articulation_id: u8) -> MidiEvent {
        let mut data = [0; 256];
        data[..3].copy_from_slice(&[status, 60, 100]);
        MidiEvent {
            sample_offset: 17,
            size: 3,
            data,
            articulation_id,
        }
    }

    #[test]
    fn articulation_map_routes_matching_notes_to_trigger_channel() {
        let state = MpeState::default();
        let map = ArticulationMap {
            id: 7,
            name: [0; 64],
            trigger_channel: 4,
        };
        let mut events = [event(0x91, 7), event(0x81, 7), event(0x92, 8)];
        unsafe {
            hirari_mpe_set_articulation_map(&state as *const _ as *mut _, &map, 1);
            hirari_midi_apply_articulations_from_state(
                &state,
                events.as_mut_ptr().cast(),
                events.len(),
            );
        }
        assert_eq!(events[0].data[0], 0x93);
        assert_eq!(events[1].data[0], 0x83);
        assert_eq!(events[2].data[0], 0x92);
        assert_eq!(events[0].data[1..3], [60, 100]);
        assert_eq!(events[0].sample_offset, 17);
    }

    #[test]
    fn sysex_queue_validates_bounds_and_preserves_message_until_copied() {
        let state = MpeState::default();
        let message = [0xf0, 0x00, 0x12, 0x34, 0x01, 0xf7];
        assert!(!unsafe {
            hirari_mpe_sysex_incoming(&state as *const _ as *mut _, [0xf0].as_ptr(), 1)
        });
        assert!(unsafe {
            hirari_mpe_sysex_incoming(
                &state as *const _ as *mut _,
                message.as_ptr(),
                message.len(),
            )
        });

        let mut manufacturer = 0;
        let required = unsafe {
            hirari_mpe_sysex_pop(
                &state as *const _ as *mut _,
                &mut manufacturer,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(required, message.len());
        let mut output = [0; 8];
        let copied = unsafe {
            hirari_mpe_sysex_pop(
                &state as *const _ as *mut _,
                &mut manufacturer,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        assert_eq!(copied, message.len());
        assert_eq!(manufacturer, 0x1234);
        assert_eq!(&output[..copied], &message);
        assert_eq!(
            unsafe {
                hirari_mpe_sysex_pop(
                    &state as *const _ as *mut _,
                    &mut manufacturer,
                    output.as_mut_ptr(),
                    output.len(),
                )
            },
            0
        );

        assert!(!unsafe {
            hirari_mpe_sysex_send(
                &state as *const _ as *mut _,
                1,
                message.as_ptr(),
                MAX_SYSEX_BYTES + 1,
            )
        });
    }

    #[test]
    fn reset_clears_articulation_maps_and_sysex_messages() {
        let state = MpeState::default();
        let map = ArticulationMap {
            id: 1,
            name: [0; 64],
            trigger_channel: 2,
        };
        unsafe {
            hirari_mpe_set_articulation_map(&state as *const _ as *mut _, &map, 1);
            let msg = [0xf0, 1, 2, 0xf7];
            assert!(hirari_mpe_sysex_incoming(
                &state as *const _ as *mut _,
                msg.as_ptr(),
                msg.len()
            ));
            hirari_mpe_state_reset(&state as *const _ as *mut _);
        }
        let mut event = event(0x90, 1);
        unsafe {
            hirari_midi_apply_articulations_from_state(
                &state,
                (&mut event as *mut MidiEvent).cast(),
                1,
            );
        }
        assert_eq!(event.data[0], 0x90);
        let mut output = [0; 8];
        let mut manufacturer = 0;
        assert_eq!(
            unsafe {
                hirari_mpe_sysex_pop(
                    &state as *const _ as *mut _,
                    &mut manufacturer,
                    output.as_mut_ptr(),
                    output.len(),
                )
            },
            0
        );
    }
}
