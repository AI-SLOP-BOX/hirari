//! Per-track MIDI expression-map state and realtime event generation.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

const ARTICULATIONS: usize = 256;
const OUTPUTS_PER_ARTICULATION: usize = 16;
const GROUPS: usize = 17;
const PENDING_NOTE_OFFS: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GeneratedMidiEvent {
    pub sample_offset: u64,
    pub articulation_id: u8,
    pub size: u8,
    pub data: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimedKeySwitchOutput {
    pub channel: u8,
    pub pitch: u8,
    pub length_ticks: u32,
}

#[derive(Clone, Copy, Default)]
struct PendingNoteOff {
    channel: u8,
    pitch: u8,
    sample: u64,
}

struct RealtimeState {
    active_articulation_by_group: [u8; GROUPS],
    pending_note_offs: [PendingNoteOff; PENDING_NOTE_OFFS],
    pending_note_off_count: usize,
    scheduled_note_counts: [[u32; 128]; 16],
}

struct ArticulationRuntime {
    outputs: [[[AtomicU32; 5]; OUTPUTS_PER_ARTICULATION]; ARTICULATIONS],
    output_counts: [AtomicU8; ARTICULATIONS],
    realtime: UnsafeCell<RealtimeState>,
    expression_maps: [Mutex<Vec<u8>>; 2],
}

// The realtime fields are only accessed by the serialized audio callback.
// Control-thread map edits touch only the atomic output table and counts.
unsafe impl Sync for ArticulationRuntime {}

impl ArticulationRuntime {
    fn new() -> Self {
        Self {
            outputs: std::array::from_fn(|_| {
                std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0)))
            }),
            output_counts: std::array::from_fn(|_| AtomicU8::new(0)),
            realtime: UnsafeCell::new(RealtimeState {
                active_articulation_by_group: [0; GROUPS],
                pending_note_offs: [PendingNoteOff::default(); PENDING_NOTE_OFFS],
                pending_note_off_count: 0,
                scheduled_note_counts: [[0; 128]; 16],
            }),
            expression_maps: [Mutex::new(b"[]".to_vec()), Mutex::new(b"null".to_vec())],
        }
    }

    fn count(&self, articulation: usize) -> usize {
        self.output_counts[articulation].load(Ordering::Acquire) as usize
    }

    fn output(&self, articulation: usize, slot: usize) -> [u32; 5] {
        std::array::from_fn(|index| self.outputs[articulation][slot][index].load(Ordering::Relaxed))
    }
}

fn push_event(
    output: &mut [GeneratedMidiEvent],
    count: &mut usize,
    sample_offset: u64,
    articulation_id: u8,
    size: u8,
    data: [u8; 3],
) {
    if *count < output.len() {
        output[*count] = GeneratedMidiEvent {
            sample_offset,
            articulation_id,
            size,
            data,
        };
        *count += 1;
    }
}

fn push_controller(
    output: &mut [GeneratedMidiEvent],
    count: &mut usize,
    sample_offset: u64,
    articulation_id: u8,
    channel: u8,
    controller: u8,
    value: u8,
) {
    push_event(
        output,
        count,
        sample_offset,
        articulation_id,
        3,
        [0xb0 | channel, controller, value],
    );
}

fn push_program(
    output: &mut [GeneratedMidiEvent],
    count: &mut usize,
    sample_offset: u64,
    articulation_id: u8,
    channel: u8,
    program: u8,
    bank_msb: u8,
    bank_lsb: u8,
) {
    if bank_msb < 128 {
        push_controller(
            output,
            count,
            sample_offset,
            articulation_id,
            channel,
            0,
            bank_msb,
        );
    }
    if bank_lsb < 128 {
        push_controller(
            output,
            count,
            sample_offset,
            articulation_id,
            channel,
            32,
            bank_lsb,
        );
    }
    push_event(
        output,
        count,
        sample_offset,
        articulation_id,
        2,
        [0xc0 | channel, program, 0],
    );
}

fn push_output_kind(
    output: &mut [GeneratedMidiEvent],
    count: &mut usize,
    sample_offset: u64,
    articulation_id: u8,
    kind: u32,
    channel: u8,
    a: u8,
    b: u8,
    c: u8,
    note_off: bool,
) {
    match kind {
        1 if note_off => push_event(output, count, sample_offset, 0, 3, [0x80 | channel, a, 0]),
        1 => push_event(
            output,
            count,
            sample_offset,
            articulation_id,
            3,
            [0x90 | channel, a, b],
        ),
        2 => push_program(
            output,
            count,
            sample_offset,
            articulation_id,
            channel,
            a,
            b,
            c,
        ),
        3 => push_controller(output, count, sample_offset, articulation_id, channel, a, b),
        4 => push_event(
            output,
            count,
            sample_offset,
            articulation_id,
            2,
            [0xd0 | channel, a, 0],
        ),
        5 => push_event(
            output,
            count,
            sample_offset,
            articulation_id,
            3,
            [0xe0 | channel, a, b],
        ),
        _ => {}
    }
}

fn generate_note_on(
    state: &ArticulationRuntime,
    runtime: &mut RealtimeState,
    channel: u8,
    pitch: u8,
    velocity: u8,
    sample_offset: u64,
    articulation_id: u8,
    output: &mut [GeneratedMidiEvent],
) -> usize {
    let mut count = 0;
    if articulation_id > 0 {
        let articulation = articulation_id as usize;
        let output_count = state.count(articulation);
        let mut group = 0;
        for slot in 0..output_count {
            let entry = state.output(articulation, slot);
            if entry[0] == 16 {
                group = entry[2] as u8;
                break;
            }
        }
        let mut emit_slot_outputs = true;
        if group > 0 && (group as usize) < GROUPS {
            let previous_id = runtime.active_articulation_by_group[group as usize];
            emit_slot_outputs = previous_id != articulation_id;
            if emit_slot_outputs && previous_id > 0 {
                let previous = previous_id as usize;
                for slot in 0..state.count(previous) {
                    let entry = state.output(previous, slot);
                    if !(11..=15).contains(&entry[0]) {
                        continue;
                    }
                    push_output_kind(
                        output,
                        &mut count,
                        sample_offset,
                        previous_id,
                        entry[0] - 10,
                        entry[1] as u8,
                        entry[2] as u8,
                        entry[3] as u8,
                        entry[4] as u8,
                        false,
                    );
                }
            }
            runtime.active_articulation_by_group[group as usize] = articulation_id;
        }
        for slot in 0..output_count {
            let entry = state.output(articulation, slot);
            let kind = entry[0];
            if kind == 16 || ((1..=5).contains(&kind) && !emit_slot_outputs) {
                continue;
            }
            push_output_kind(
                output,
                &mut count,
                sample_offset,
                articulation_id,
                kind,
                entry[1] as u8,
                entry[2] as u8,
                entry[3] as u8,
                entry[4] as u8,
                false,
            );
        }
    }
    push_event(
        output,
        &mut count,
        sample_offset,
        articulation_id,
        3,
        [0x90 | channel.saturating_sub(1), pitch, velocity],
    );
    if (1..=16).contains(&channel) {
        let active = &mut runtime.scheduled_note_counts[channel as usize - 1][pitch as usize];
        *active = active.saturating_add(1);
    }
    count
}

fn generate_note_off(
    state: &ArticulationRuntime,
    runtime: &mut RealtimeState,
    channel: u8,
    pitch: u8,
    sample_offset: u64,
    articulation_id: u8,
    output: &mut [GeneratedMidiEvent],
) -> usize {
    let mut count = 0;
    if articulation_id > 0 {
        let articulation = articulation_id as usize;
        for slot in 0..state.count(articulation) {
            let entry = state.output(articulation, slot);
            if !(6..=10).contains(&entry[0]) {
                continue;
            }
            push_output_kind(
                output,
                &mut count,
                sample_offset,
                articulation_id,
                entry[0] - 5,
                entry[1] as u8,
                entry[2] as u8,
                entry[3] as u8,
                entry[4] as u8,
                true,
            );
        }
    }
    push_event(
        output,
        &mut count,
        sample_offset,
        0,
        3,
        [0x80 | channel.saturating_sub(1), pitch, 0],
    );
    if (1..=16).contains(&channel) {
        let active = &mut runtime.scheduled_note_counts[channel as usize - 1][pitch as usize];
        *active = active.saturating_sub(1);
    }
    count
}

#[no_mangle]
pub extern "C" fn hirari_track_midi_articulation_create() -> *mut c_void {
    Box::into_raw(Box::new(ArticulationRuntime::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_articulation_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<ArticulationRuntime>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_expression_map_set(
    state: *const c_void,
    map_kind: u8,
    data: *const u8,
    length: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return false;
    };
    let Some(map) = state.expression_maps.get(map_kind as usize) else {
        return false;
    };
    if length > 0 && data.is_null() {
        return false;
    }
    let bytes = if length == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, length) }.to_vec()
    };
    *map.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = bytes;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_expression_map_copy(
    state: *const c_void,
    map_kind: u8,
    output: *mut u8,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    let Some(map) = state.expression_maps.get(map_kind as usize) else {
        return 0;
    };
    let bytes = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if output.is_null() || capacity < bytes.len() {
        return bytes.len();
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
    bytes.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_articulation_begin(
    state: *const c_void,
    articulation_id: u8,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return false;
    };
    if articulation_id == 0 {
        return false;
    }
    state.output_counts[articulation_id as usize].store(0, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_articulation_set_output(
    state: *const c_void,
    articulation_id: u8,
    slot: u8,
    kind: u8,
    channel: u8,
    a: u8,
    b: u8,
    c: u8,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return false;
    };
    if articulation_id == 0
        || slot as usize >= OUTPUTS_PER_ARTICULATION
        || !(1..=16).contains(&kind)
        || channel >= 16
    {
        return false;
    }
    for (atomic, value) in state.outputs[articulation_id as usize][slot as usize]
        .iter()
        .zip([kind, channel, a, b, c])
    {
        atomic.store(value as u32, Ordering::Relaxed);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_articulation_finish(
    state: *const c_void,
    articulation_id: u8,
    count: u8,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return false;
    };
    if articulation_id == 0 || count as usize > OUTPUTS_PER_ARTICULATION {
        return false;
    }
    state.output_counts[articulation_id as usize].store(count, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_articulation_pack(
    state: *const c_void,
    output: *mut u32,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    let required: usize = (1..ARTICULATIONS).map(|id| state.count(id) * 6).sum();
    if output.is_null() || capacity < required {
        return required;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    let mut cursor = 0;
    for id in 1..ARTICULATIONS {
        for slot in 0..state.count(id) {
            let entry = state.output(id, slot);
            output[cursor..cursor + 6]
                .copy_from_slice(&[id as u32, entry[1], entry[0], entry[2], entry[3], entry[4]]);
            cursor += 6;
        }
    }
    required
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_timed_outputs(
    state: *const c_void,
    articulation_id: u8,
    transitions: bool,
    output: *mut TimedKeySwitchOutput,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    if articulation_id == 0 || output.is_null() || capacity == 0 {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    let mut written = 0;
    let mut selected = articulation_id;
    let mut wanted_kind = 1;
    if transitions {
        let mut group = 0;
        for slot in 0..state.count(articulation_id as usize) {
            let entry = state.output(articulation_id as usize, slot);
            if entry[0] == 16 {
                group = entry[2] as u8;
                break;
            }
        }
        if group == 0 || (group as usize) >= GROUPS {
            return 0;
        }
        selected = unsafe { (*state.realtime.get()).active_articulation_by_group[group as usize] };
        wanted_kind = 11;
    }
    if selected == 0 {
        return 0;
    }
    for slot in 0..state.count(selected as usize) {
        let entry = state.output(selected as usize, slot);
        if entry[0] != wanted_kind {
            continue;
        }
        if transitions && !(11..=15).contains(&entry[0]) {
            continue;
        }
        if entry[4] == 0 {
            continue;
        }
        if written < output.len() {
            output[written] = TimedKeySwitchOutput {
                channel: entry[1] as u8,
                pitch: entry[2] as u8,
                length_ticks: entry[4],
            };
            written += 1;
        }
    }
    written
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_schedule_note_off(
    state: *const c_void,
    channel: u8,
    pitch: u8,
    sample: u64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return false;
    };
    if channel >= 16 || pitch >= 128 {
        return false;
    }
    let runtime = unsafe { &mut *state.realtime.get() };
    if runtime.pending_note_off_count >= PENDING_NOTE_OFFS {
        return false;
    }
    runtime.pending_note_offs[runtime.pending_note_off_count] = PendingNoteOff {
        channel,
        pitch,
        sample,
    };
    runtime.pending_note_off_count += 1;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_note_on(
    state: *const c_void,
    channel: u8,
    pitch: u8,
    velocity: u8,
    sample_offset: u64,
    articulation_id: u8,
    output: *mut GeneratedMidiEvent,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    if output.is_null() || capacity == 0 || channel == 0 || channel > 16 {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    let runtime = unsafe { &mut *state.realtime.get() };
    generate_note_on(
        state,
        runtime,
        channel,
        pitch,
        velocity,
        sample_offset,
        articulation_id,
        output,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_note_off(
    state: *const c_void,
    channel: u8,
    pitch: u8,
    sample_offset: u64,
    articulation_id: u8,
    output: *mut GeneratedMidiEvent,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    if output.is_null() || capacity == 0 || channel == 0 || channel > 16 {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    let runtime = unsafe { &mut *state.realtime.get() };
    generate_note_off(
        state,
        runtime,
        channel,
        pitch,
        sample_offset,
        articulation_id,
        output,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_all_notes_off(
    state: *const c_void,
    sample_offset: u64,
    output: *mut GeneratedMidiEvent,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    if capacity != 0 && output.is_null() {
        return 0;
    }
    let output = if capacity == 0 {
        &mut [][..]
    } else {
        unsafe { std::slice::from_raw_parts_mut(output, capacity) }
    };
    let runtime = unsafe { &mut *state.realtime.get() };
    runtime.pending_note_off_count = 0;
    runtime.active_articulation_by_group.fill(0);
    let mut count = 0;
    for channel in 1..=16u8 {
        push_event(
            output,
            &mut count,
            sample_offset,
            0,
            3,
            [0xb0 | (channel - 1), 123, 0],
        );
    }
    for channel in 1..=16usize {
        for pitch in 0..128usize {
            let active = &mut runtime.scheduled_note_counts[channel - 1][pitch];
            while *active > 0 && count < output.len() {
                push_event(
                    output,
                    &mut count,
                    sample_offset,
                    0,
                    3,
                    [0x80 | (channel as u8 - 1), pitch as u8, 0],
                );
                *active -= 1;
            }
            *active = 0;
        }
    }
    count
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_midi_flush_scheduled_note_offs(
    state: *const c_void,
    playhead: u64,
    block_size: u32,
    output: *mut GeneratedMidiEvent,
    capacity: usize,
) -> usize {
    let Some(state) = (unsafe { state.cast::<ArticulationRuntime>().as_ref() }) else {
        return 0;
    };
    if output.is_null() || capacity == 0 {
        return 0;
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
    let runtime = unsafe { &mut *state.realtime.get() };
    let block_end = playhead.saturating_add(block_size as u64);
    let mut kept = 0;
    let mut written = 0;
    for index in 0..runtime.pending_note_off_count {
        let event = runtime.pending_note_offs[index];
        if event.sample < block_end && written < output.len() {
            push_event(
                output,
                &mut written,
                event.sample.saturating_sub(playhead),
                0,
                3,
                [0x80 | event.channel, event.pitch, 0],
            );
        } else {
            runtime.pending_note_offs[kept] = event;
            kept += 1;
        }
    }
    runtime.pending_note_off_count = kept;
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_output(state: &ArticulationRuntime, id: u8, slot: usize, values: [u32; 5]) {
        for (atomic, value) in state.outputs[id as usize][slot].iter().zip(values) {
            atomic.store(value, Ordering::Relaxed);
        }
    }

    fn finish(state: &ArticulationRuntime, id: u8, count: u8) {
        state.output_counts[id as usize].store(count, Ordering::Release);
    }

    #[test]
    fn note_on_emits_keyswitch_program_and_note_with_packed_mapping_order() {
        let state = ArticulationRuntime::new();
        set_output(&state, 7, 0, [1, 2, 60, 100, 0]);
        set_output(&state, 7, 1, [2, 1, 12, 0, 32]);
        finish(&state, 7, 2);
        let mut runtime = RealtimeState {
            active_articulation_by_group: [0; GROUPS],
            pending_note_offs: [PendingNoteOff::default(); PENDING_NOTE_OFFS],
            pending_note_off_count: 0,
            scheduled_note_counts: [[0; 128]; 16],
        };
        let mut events = [GeneratedMidiEvent::default(); 8];
        let count = generate_note_on(&state, &mut runtime, 3, 64, 99, 12, 7, &mut events);
        assert_eq!(count, 5);
        assert_eq!(events[0].data, [0x92, 60, 100]);
        assert_eq!(events[1].data, [0xb1, 0, 0]);
        assert_eq!(events[2].data, [0xb1, 32, 32]);
        assert_eq!(events[3].data, [0xc1, 12, 0]);
        assert_eq!(events[4].data, [0x92, 64, 99]);
        assert_eq!(runtime.scheduled_note_counts[2][64], 1);
    }

    #[test]
    fn same_group_articulation_suppresses_repeated_keyswitch_and_transition_releases_previous() {
        let state = ArticulationRuntime::new();
        set_output(&state, 1, 0, [11, 4, 45, 0, 240]);
        set_output(&state, 1, 1, [16, 0, 2, 0, 0]);
        set_output(&state, 2, 0, [1, 3, 70, 100, 0]);
        set_output(&state, 2, 1, [16, 0, 2, 0, 0]);
        finish(&state, 1, 2);
        finish(&state, 2, 2);
        let mut runtime = RealtimeState {
            active_articulation_by_group: [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            pending_note_offs: [PendingNoteOff::default(); PENDING_NOTE_OFFS],
            pending_note_off_count: 0,
            scheduled_note_counts: [[0; 128]; 16],
        };
        let mut events = [GeneratedMidiEvent::default(); 8];
        let count = generate_note_on(&state, &mut runtime, 1, 60, 100, 4, 2, &mut events);
        assert_eq!(count, 3);
        assert_eq!(events[0].data, [0x94, 45, 0]);
        assert_eq!(events[1].data, [0x93, 70, 100]);
        assert_eq!(events[2].data, [0x90, 60, 100]);
        assert_eq!(runtime.active_articulation_by_group[2], 2);
        let repeated = generate_note_on(&state, &mut runtime, 1, 61, 90, 5, 2, &mut events);
        assert_eq!(repeated, 1);
        assert_eq!(events[0].data, [0x90, 61, 90]);
    }

    #[test]
    fn scheduled_note_offs_flush_sample_accurately_and_all_notes_off_clears_counts() {
        let state = ArticulationRuntime::new();
        let runtime = unsafe { &mut *state.realtime.get() };
        runtime.pending_note_offs[0] = PendingNoteOff {
            channel: 0,
            pitch: 60,
            sample: 105,
        };
        runtime.pending_note_offs[1] = PendingNoteOff {
            channel: 1,
            pitch: 62,
            sample: 200,
        };
        runtime.pending_note_off_count = 2;
        runtime.scheduled_note_counts[0][60] = 2;
        let mut events = [GeneratedMidiEvent::default(); 16];
        let flushed = unsafe {
            hirari_track_midi_flush_scheduled_note_offs(
                (&state as *const ArticulationRuntime).cast(),
                100,
                16,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        assert_eq!(flushed, 1);
        assert_eq!(events[0].sample_offset, 5);
        assert_eq!(runtime.pending_note_off_count, 1);
        let all_off = unsafe {
            hirari_track_midi_all_notes_off(
                (&state as *const ArticulationRuntime).cast(),
                20,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        assert_eq!(all_off, 16);
        assert_eq!(events[0].data, [0xb0, 123, 0]);
        assert_eq!(events[15].data, [0xbf, 123, 0]);
        assert_eq!(runtime.pending_note_off_count, 0);
        assert_eq!(runtime.scheduled_note_counts[0][60], 0);
    }

    #[test]
    fn all_notes_off_resets_runtime_even_when_the_midi_buffer_is_full() {
        let state = ArticulationRuntime::new();
        let runtime = unsafe { &mut *state.realtime.get() };
        runtime.pending_note_offs[0] = PendingNoteOff {
            channel: 0,
            pitch: 60,
            sample: 105,
        };
        runtime.pending_note_off_count = 1;
        runtime.active_articulation_by_group[1] = 5;
        runtime.scheduled_note_counts[0][60] = 3;
        let count = unsafe {
            hirari_track_midi_all_notes_off(
                (&state as *const ArticulationRuntime).cast(),
                10,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(count, 0);
        assert_eq!(runtime.pending_note_off_count, 0);
        assert_eq!(runtime.active_articulation_by_group[1], 0);
        assert_eq!(runtime.scheduled_note_counts[0][60], 0);
    }

    #[test]
    fn ffi_publishes_expression_map_and_returns_sample_ready_midi_events() {
        let state = hirari_track_midi_articulation_create();
        assert!(!state.is_null());
        unsafe {
            assert!(hirari_track_midi_articulation_begin(state, 3));
            assert!(hirari_track_midi_articulation_set_output(
                state, 3, 0, 1, 2, 40, 100, 120,
            ));
            assert!(hirari_track_midi_articulation_finish(state, 3, 1));

            let required = hirari_track_midi_articulation_pack(state, std::ptr::null_mut(), 0);
            assert_eq!(required, 6);
            let mut packed = [0; 6];
            assert_eq!(
                hirari_track_midi_articulation_pack(state, packed.as_mut_ptr(), 6),
                6
            );
            assert_eq!(packed, [3, 2, 1, 40, 100, 120]);

            let mut timed = [TimedKeySwitchOutput::default(); 16];
            let timed_count =
                hirari_track_midi_timed_outputs(state, 3, false, timed.as_mut_ptr(), timed.len());
            assert_eq!(timed_count, 1);
            assert_eq!(timed[0].channel, 2);
            assert_eq!(timed[0].pitch, 40);
            assert_eq!(timed[0].length_ticks, 120);

            let mut generated = [GeneratedMidiEvent::default(); 128];
            let count = hirari_track_midi_note_on(
                state,
                1,
                60,
                90,
                7,
                3,
                generated.as_mut_ptr(),
                generated.len(),
            );
            assert_eq!(count, 2);
            assert_eq!(generated[0].sample_offset, 7);
            assert_eq!(generated[0].articulation_id, 3);
            assert_eq!(generated[0].data, [0x92, 40, 100]);
            assert_eq!(generated[1].data, [0x90, 60, 90]);
            hirari_track_midi_articulation_destroy(state);
        }
    }
}
