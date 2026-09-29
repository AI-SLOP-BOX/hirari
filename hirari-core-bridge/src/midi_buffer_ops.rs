use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const MIDI_EVENT_CAPACITY: usize = 1024;
const MIDI_EVENT_BYTES: usize = 256;

// C++ MidiBuffer maps these return codes to its existing overflow telemetry.
const WRITE_OK: u8 = 0;
const WRITE_TOO_LARGE: u8 = 1;
const WRITE_INVALID_DATA: u8 = 2;
const WRITE_FULL: u8 = 3;
const WRITE_INVALID_ARGUMENT: u8 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct MidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; MIDI_EVENT_BYTES],
    _articulation_id: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct OwnedMidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; MIDI_EVENT_BYTES],
    articulation_id: u8,
}

struct MidiBufferState {
    events: [OwnedMidiEvent; MIDI_EVENT_CAPACITY],
    count: usize,
    dropped_events: AtomicU64,
    oversize_events: AtomicU64,
    extended_events: AtomicU64,
    overflowed: AtomicBool,
}

const EMPTY_MIDI_EVENT: OwnedMidiEvent = OwnedMidiEvent {
    sample_offset: 0,
    size: 0,
    data: [0; MIDI_EVENT_BYTES],
    articulation_id: 0,
};

struct ChordTriggerState {
    scratch: MidiBufferState,
    intervals: [i32; 12],
    interval_count: usize,
    sample_rate_bits: AtomicU64,
    strum_ms_bits: AtomicU32,
    strum_samples: AtomicU32,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_init(
    storage: *mut c_void,
    bytes: usize,
) -> *mut c_void {
    if storage.is_null()
        || (storage as usize) % std::mem::align_of::<MidiBufferState>() != 0
        || bytes < std::mem::size_of::<MidiBufferState>()
    {
        return std::ptr::null_mut();
    }
    let state = storage.cast::<MidiBufferState>();
    unsafe { initialize_midi_buffer_state(state) };
    storage
}

unsafe fn initialize_midi_buffer_state(state: *mut MidiBufferState) {
    // Initialize in place to avoid a 278 KiB temporary on the caller's stack.
    unsafe {
        std::ptr::write_bytes(std::ptr::addr_of_mut!((*state).events), 0, 1);
        std::ptr::addr_of_mut!((*state).count).write(0);
        std::ptr::addr_of_mut!((*state).dropped_events).write(AtomicU64::new(0));
        std::ptr::addr_of_mut!((*state).oversize_events).write(AtomicU64::new(0));
        std::ptr::addr_of_mut!((*state).extended_events).write(AtomicU64::new(0));
        std::ptr::addr_of_mut!((*state).overflowed).write(AtomicBool::new(false));
    }
}

#[no_mangle]
pub extern "C" fn hirari_midi_buffer_create() -> *mut c_void {
    let mut storage = Box::<MidiBufferState>::new_uninit();
    let state = storage.as_mut_ptr();
    // SAFETY: The Box owns a properly aligned allocation large enough for the state.
    unsafe { initialize_midi_buffer_state(state) };
    Box::into_raw(storage).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: This pointer comes from hirari_midi_buffer_create and is destroyed once.
        drop(unsafe { Box::from_raw(state.cast::<MidiBufferState>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_drop_in_place(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: Caller initialized this state in its own aligned storage.
        unsafe { std::ptr::drop_in_place(state.cast::<MidiBufferState>()) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_add(
    state: *mut c_void,
    sample_offset: u64,
    data: *const u8,
    size: u32,
    articulation_id: u8,
) {
    let Some(state) = (unsafe { state.cast::<MidiBufferState>().as_mut() }) else {
        return;
    };
    let result = unsafe {
        hirari_midi_buffer_add_event(
            state.events.as_mut_ptr().cast(),
            MIDI_EVENT_CAPACITY,
            &mut state.count,
            sample_offset,
            data,
            size,
            articulation_id,
        )
    };
    match result {
        WRITE_OK | WRITE_INVALID_DATA | WRITE_INVALID_ARGUMENT => {}
        WRITE_TOO_LARGE => {
            state.oversize_events.fetch_add(1, Ordering::Relaxed);
            if !data.is_null() && size > 0 && (unsafe { *data } == 0xf0 || size >= 4) {
                state.extended_events.fetch_add(1, Ordering::Relaxed);
            }
            state.overflowed.store(true, Ordering::Release);
        }
        WRITE_FULL => {
            state.dropped_events.fetch_add(1, Ordering::Relaxed);
            state.overflowed.store(true, Ordering::Release);
        }
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_copy(
    state: *mut c_void,
    source: *const c_void,
) -> bool {
    let Some(state) = (unsafe { state.cast::<MidiBufferState>().as_mut() }) else {
        return false;
    };
    let result = unsafe {
        hirari_midi_buffer_copy_event(
            state.events.as_mut_ptr().cast(),
            MIDI_EVENT_CAPACITY,
            &mut state.count,
            source,
        )
    };
    match result {
        WRITE_OK => true,
        WRITE_TOO_LARGE => {
            let event = unsafe { &*source.cast::<MidiEvent>() };
            state.oversize_events.fetch_add(1, Ordering::Relaxed);
            if event.size >= 4 || (event.size > 0 && event.data[0] == 0xf0) {
                state.extended_events.fetch_add(1, Ordering::Relaxed);
            }
            state.overflowed.store(true, Ordering::Release);
            false
        }
        WRITE_FULL => {
            state.dropped_events.fetch_add(1, Ordering::Relaxed);
            state.overflowed.store(true, Ordering::Release);
            false
        }
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_clear(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<MidiBufferState>().as_mut() } {
        state.count = 0;
        state.dropped_events.store(0, Ordering::Relaxed);
        state.oversize_events.store(0, Ordering::Relaxed);
        state.extended_events.store(0, Ordering::Relaxed);
        state.overflowed.store(false, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_sort_owned(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<MidiBufferState>().as_mut() } {
        unsafe { hirari_midi_buffer_sort(state.events.as_mut_ptr().cast(), state.count) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_event_data(state: *const c_void) -> *mut c_void {
    unsafe { state.cast::<MidiBufferState>().as_ref() }.map_or(std::ptr::null_mut(), |state| {
        state.events.as_ptr().cast_mut().cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_event_count(state: *const c_void) -> usize {
    unsafe { state.cast::<MidiBufferState>().as_ref() }.map_or(0, |state| state.count)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_remaining_capacity(state: *const c_void) -> usize {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .map_or(0, |state| MIDI_EVENT_CAPACITY.saturating_sub(state.count))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_overflowed(state: *const c_void) -> bool {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .is_some_and(|state| state.overflowed.load(Ordering::Acquire))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_take_overflowed(state: *const c_void) -> bool {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .is_some_and(|state| state.overflowed.swap(false, Ordering::AcqRel))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_take_dropped(state: *const c_void) -> u64 {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .map_or(0, |state| state.dropped_events.swap(0, Ordering::AcqRel))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_dropped_count(state: *const c_void) -> u64 {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .map_or(0, |state| state.dropped_events.load(Ordering::Relaxed))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_take_oversize(state: *const c_void) -> u64 {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .map_or(0, |state| state.oversize_events.swap(0, Ordering::AcqRel))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_take_extended(state: *const c_void) -> u64 {
    unsafe { state.cast::<MidiBufferState>().as_ref() }
        .map_or(0, |state| state.extended_events.swap(0, Ordering::AcqRel))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_reject_extended(state: *const c_void) {
    if let Some(state) = unsafe { state.cast::<MidiBufferState>().as_ref() } {
        state.oversize_events.fetch_add(1, Ordering::Relaxed);
        state.extended_events.fetch_add(1, Ordering::Relaxed);
        state.overflowed.store(true, Ordering::Release);
    }
}

/// Expands note events into a chord, then replaces the input buffer with the
/// sorted result. Scratch storage is a second preallocated Rust-owned buffer.
#[no_mangle]
pub extern "C" fn hirari_chord_trigger_create() -> *mut c_void {
    let scratch = MidiBufferState {
        events: [EMPTY_MIDI_EVENT; MIDI_EVENT_CAPACITY],
        count: 0,
        dropped_events: AtomicU64::new(0),
        oversize_events: AtomicU64::new(0),
        extended_events: AtomicU64::new(0),
        overflowed: AtomicBool::new(false),
    };
    Box::into_raw(Box::new(ChordTriggerState {
        scratch,
        intervals: [0, 4, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        interval_count: 3,
        sample_rate_bits: AtomicU64::new(44_100.0f64.to_bits()),
        strum_ms_bits: AtomicU32::new(15.0f32.to_bits()),
        strum_samples: AtomicU32::new(661),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<ChordTriggerState>()) });
    }
}

fn chord_trigger_update_samples(state: &ChordTriggerState) {
    let sample_rate = f64::from_bits(state.sample_rate_bits.load(Ordering::Relaxed));
    let strum_ms = f32::from_bits(state.strum_ms_bits.load(Ordering::Relaxed));
    let samples = if sample_rate.is_finite() && sample_rate > 0.0 {
        ((strum_ms as f64 / 1000.0) * sample_rate) as u32
    } else {
        0
    };
    state.strum_samples.store(samples, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<ChordTriggerState>().as_ref() } {
        state
            .sample_rate_bits
            .store(sample_rate.to_bits(), Ordering::Relaxed);
        chord_trigger_update_samples(state);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_set_strum_ms(state: *mut c_void, value: f32) {
    if let Some(state) = unsafe { state.cast::<ChordTriggerState>().as_ref() } {
        let value = if value.is_finite() {
            value.clamp(0.0, 200.0)
        } else {
            15.0
        };
        state
            .strum_ms_bits
            .store(value.to_bits(), Ordering::Relaxed);
        chord_trigger_update_samples(state);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_get_strum_ms(state: *const c_void) -> f32 {
    unsafe { state.cast::<ChordTriggerState>().as_ref() }.map_or(0.0, |state| {
        f32::from_bits(state.strum_ms_bits.load(Ordering::Relaxed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_save_state(
    state: *const c_void,
    bypassed: bool,
    mix: f32,
    sidechain_bus_id: u32,
    output: *mut u8,
    capacity: usize,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ChordTriggerState>().as_ref() }) else {
        return false;
    };
    if output.is_null() || capacity < 24 || !mix.is_finite() || !(0.0..=1.0).contains(&mix) {
        return false;
    }
    let mut bytes = [0_u8; 24];
    bytes[0..4].copy_from_slice(&0x4155_5241_u32.to_ne_bytes());
    bytes[4..6].copy_from_slice(&1_u16.to_ne_bytes());
    bytes[6..8].copy_from_slice(&(u16::from(bypassed)).to_ne_bytes());
    bytes[8..12].copy_from_slice(&mix.to_ne_bytes());
    bytes[12..16].copy_from_slice(&sidechain_bus_id.to_ne_bytes());
    bytes[16..20].copy_from_slice(
        &f32::from_bits(state.strum_ms_bits.load(Ordering::Relaxed)).to_ne_bytes(),
    );
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_restore_state(
    state: *mut c_void,
    input: *const u8,
    length: usize,
    bypassed: *mut bool,
    mix: *mut f32,
    sidechain_bus_id: *mut u32,
) -> bool {
    let Some(state) = (unsafe { state.cast::<ChordTriggerState>().as_mut() }) else {
        return false;
    };
    if input.is_null()
        || length != 24
        || bypassed.is_null()
        || mix.is_null()
        || sidechain_bus_id.is_null()
    {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(input, length) };
    let read_u16 = |range: std::ops::Range<usize>| {
        u16::from_ne_bytes(bytes[range].try_into().expect("fixed-width state field"))
    };
    let read_u32 = |range: std::ops::Range<usize>| {
        u32::from_ne_bytes(bytes[range].try_into().expect("fixed-width state field"))
    };
    let magic = read_u32(0..4);
    let version = read_u16(4..6);
    let flags = read_u16(6..8);
    let decoded_mix = f32::from_ne_bytes(bytes[8..12].try_into().expect("fixed-width state field"));
    let decoded_sidechain = read_u32(12..16);
    let strum = f32::from_ne_bytes(bytes[16..20].try_into().expect("fixed-width state field"));
    if magic != 0x4155_5241
        || version != 1
        || flags & !1 != 0
        || !decoded_mix.is_finite()
        || !(0.0..=1.0).contains(&decoded_mix)
        || !strum.is_finite()
        || !(0.0..=200.0).contains(&strum)
    {
        return false;
    }
    state
        .strum_ms_bits
        .store(strum.to_bits(), Ordering::Relaxed);
    chord_trigger_update_samples(state);
    unsafe {
        bypassed.write(flags & 1 != 0);
        mix.write(decoded_mix);
        sidechain_bus_id.write(decoded_sidechain);
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<ChordTriggerState>().as_mut() } {
        state.scratch.count = 0;
        state.scratch.dropped_events.store(0, Ordering::Relaxed);
        state.scratch.oversize_events.store(0, Ordering::Relaxed);
        state.scratch.extended_events.store(0, Ordering::Relaxed);
        state.scratch.overflowed.store(false, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_chord_trigger_process(
    state: *mut c_void,
    input: *mut c_void,
) -> bool {
    if state.is_null() || input.is_null() {
        return false;
    }
    let state = unsafe { &mut *state.cast::<ChordTriggerState>() };
    let input = unsafe { &mut *input.cast::<MidiBufferState>() };
    let intervals = &state.intervals[..state.interval_count.min(state.intervals.len())];
    chord_trigger_process_buffers(
        input,
        &mut state.scratch,
        intervals,
        state.strum_samples.load(Ordering::Relaxed),
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_chord_trigger(
    input: *mut c_void,
    scratch: *mut c_void,
    intervals: *const i32,
    interval_count: usize,
    strum_samples: u32,
) -> bool {
    if input.is_null()
        || scratch.is_null()
        || input == scratch
        || (interval_count > 0 && intervals.is_null())
    {
        return false;
    }
    let input = unsafe { &mut *input.cast::<MidiBufferState>() };
    let scratch = unsafe { &mut *scratch.cast::<MidiBufferState>() };
    let interval_count = interval_count.min(128);
    let intervals = if interval_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(intervals, interval_count) }
    };
    chord_trigger_process_buffers(input, scratch, intervals, strum_samples)
}

fn chord_trigger_process_buffers(
    input: &mut MidiBufferState,
    scratch: &mut MidiBufferState,
    intervals: &[i32],
    strum_samples: u32,
) -> bool {
    if std::ptr::eq(input, scratch) {
        return false;
    }
    scratch.count = 0;
    scratch.dropped_events.store(0, Ordering::Relaxed);
    scratch.oversize_events.store(0, Ordering::Relaxed);
    scratch.extended_events.store(0, Ordering::Relaxed);
    scratch.overflowed.store(false, Ordering::Release);
    for index in 0..input.count {
        let source = input.events[index];
        if source.size < 3 {
            append_chord_passthrough(scratch, source);
            continue;
        }
        let status = source.data[0] & 0xf0;
        if status == 0x90 && source.data[2] != 0 {
            for (chord_index, interval) in intervals.iter().enumerate() {
                let mut event = OwnedMidiEvent {
                    sample_offset: source.sample_offset.wrapping_add(
                        (u64::from(strum_samples) * chord_index as u64).min(u64::from(u32::MAX)),
                    ),
                    size: 3,
                    data: [0; MIDI_EVENT_BYTES],
                    articulation_id: source.articulation_id,
                };
                event.data[0] = source.data[0];
                event.data[1] = (i32::from(source.data[1]) + *interval).clamp(0, 127) as u8;
                event.data[2] = source.data[2];
                append_chord_event(scratch, event);
            }
        } else if status == 0x80 || (status == 0x90 && source.data[2] == 0) {
            for interval in intervals {
                let mut event = OwnedMidiEvent {
                    sample_offset: source.sample_offset,
                    size: 3,
                    data: [0; MIDI_EVENT_BYTES],
                    articulation_id: 0,
                };
                event.data[0] = 0x80 | (source.data[0] & 0x0f);
                event.data[1] = (i32::from(source.data[1]) + *interval).clamp(0, 127) as u8;
                append_chord_event(scratch, event);
            }
        } else {
            append_chord_passthrough(scratch, source);
        }
    }
    input.count = 0;
    input.dropped_events.store(0, Ordering::Relaxed);
    input.oversize_events.store(0, Ordering::Relaxed);
    input.extended_events.store(0, Ordering::Relaxed);
    input.overflowed.store(false, Ordering::Release);
    for index in 0..scratch.count {
        append_chord_event(input, scratch.events[index]);
    }
    unsafe { hirari_midi_buffer_sort(input.events.as_mut_ptr().cast(), input.count) };
    true
}

fn append_chord_passthrough(state: &mut MidiBufferState, mut event: OwnedMidiEvent) {
    if event.size as usize > MIDI_EVENT_BYTES {
        state.oversize_events.fetch_add(1, Ordering::Relaxed);
        if event.size >= 4 || (event.size > 0 && event.data[0] == 0xf0) {
            state.extended_events.fetch_add(1, Ordering::Relaxed);
        }
        state.overflowed.store(true, Ordering::Release);
        return;
    }
    event.data[event.size as usize..].fill(0);
    append_chord_event(state, event);
}

fn append_chord_event(state: &mut MidiBufferState, event: OwnedMidiEvent) {
    if state.count >= MIDI_EVENT_CAPACITY {
        state.dropped_events.fetch_add(1, Ordering::Relaxed);
        state.overflowed.store(true, Ordering::Release);
        return;
    }
    state.events[state.count] = event;
    state.count += 1;
}

#[cfg(test)]
mod owned_state_tests {
    use super::*;

    unsafe extern "C" {
        fn hirari_midi_buffer_cpp_wrapper_smoke() -> bool;
        fn hirari_chord_trigger_cpp_wrapper_smoke() -> bool;
    }

    #[test]
    fn rust_owned_fixed_storage_sorts_events_and_reports_overflow() {
        let bytes = std::mem::size_of::<MidiBufferState>();
        let words = bytes.div_ceil(std::mem::size_of::<u64>());
        let mut storage = vec![0_u64; words];
        let state =
            unsafe { hirari_midi_buffer_init(storage.as_mut_ptr().cast(), storage.len() * 8) };
        assert!(!state.is_null());
        let note_on = [0x90, 60, 100];
        let note_off = [0x80, 60, 0];
        unsafe {
            hirari_midi_buffer_add(state, 4, note_on.as_ptr(), 3, 7);
            hirari_midi_buffer_add(state, 4, note_off.as_ptr(), 3, 0);
            hirari_midi_buffer_sort_owned(state);
        }
        assert_eq!(unsafe { hirari_midi_buffer_event_count(state) }, 2);
        let events = unsafe { hirari_midi_buffer_event_data(state).cast::<MidiEvent>() };
        assert_eq!(unsafe { (*events).data[0] }, 0x80);
        assert_eq!(unsafe { (*events.add(1)).data[0] }, 0x90);

        for _ in 0..=MIDI_EVENT_CAPACITY {
            unsafe { hirari_midi_buffer_add(state, 0, note_on.as_ptr(), 3, 0) };
        }
        assert!(unsafe { hirari_midi_buffer_overflowed(state) });
        assert_eq!(unsafe { hirari_midi_buffer_take_dropped(state) }, 3);
        unsafe {
            hirari_midi_buffer_clear(state);
            hirari_midi_buffer_drop_in_place(state);
        }
    }

    #[test]
    fn owned_state_rejects_misaligned_or_short_storage() {
        let mut storage = [0_u8; 64];
        assert!(unsafe {
            hirari_midi_buffer_init(storage.as_mut_ptr().add(1).cast(), storage.len() - 1)
        }
        .is_null());
        assert!(
            unsafe { hirari_midi_buffer_init(storage.as_mut_ptr().cast(), storage.len()) }
                .is_null()
        );
    }

    #[test]
    fn cpp_compatibility_wrapper_uses_rust_owned_storage_and_accounting() {
        assert!(unsafe { hirari_midi_buffer_cpp_wrapper_smoke() });
    }

    #[test]
    fn cpp_chord_trigger_runs_through_the_rust_realtime_buffer_path() {
        assert!(unsafe { hirari_chord_trigger_cpp_wrapper_smoke() });
    }
}

unsafe fn event_storage_mut(
    events: *mut c_void,
    count: *mut usize,
) -> Option<(*mut MidiEvent, *mut usize)> {
    if events.is_null() || count.is_null() {
        return None;
    }
    Some((events.cast::<MidiEvent>(), count))
}

/// Appends a bounded event into the caller-owned fixed event array. All bytes
/// after `size` are cleared, matching MidiBuffer's previous C++ contract.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_add_event(
    events: *mut c_void,
    capacity: usize,
    count: *mut usize,
    sample_offset: u64,
    data: *const u8,
    size: u32,
    articulation_id: u8,
) -> u8 {
    if size as usize > MIDI_EVENT_BYTES {
        return WRITE_TOO_LARGE;
    }
    if size != 0 && data.is_null() {
        return WRITE_INVALID_DATA;
    }
    let Some((events, count)) = event_storage_mut(events, count) else {
        return WRITE_INVALID_ARGUMENT;
    };
    let count = &mut *count;
    if *count >= capacity.min(MIDI_EVENT_CAPACITY) {
        return WRITE_FULL;
    }
    let destination = &mut *events.add(*count);
    destination.sample_offset = sample_offset;
    destination.size = size;
    destination._articulation_id = articulation_id;
    if size > 0 {
        std::ptr::copy_nonoverlapping(data, destination.data.as_mut_ptr(), size as usize);
    }
    destination.data[size as usize..].fill(0);
    *count += 1;
    WRITE_OK
}

/// Copies a complete fixed-layout event into the caller-owned event array,
/// clearing any unused payload bytes before publishing the new count.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_copy_event(
    events: *mut c_void,
    capacity: usize,
    count: *mut usize,
    source: *const c_void,
) -> u8 {
    if source.is_null() {
        return WRITE_INVALID_ARGUMENT;
    }
    let source = std::ptr::read(source.cast::<MidiEvent>());
    if source.size as usize > MIDI_EVENT_BYTES {
        return WRITE_TOO_LARGE;
    }
    let Some((events, count)) = event_storage_mut(events, count) else {
        return WRITE_INVALID_ARGUMENT;
    };
    let count = &mut *count;
    if *count >= capacity.min(MIDI_EVENT_CAPACITY) {
        return WRITE_FULL;
    }
    let destination = &mut *events.add(*count);
    *destination = source;
    destination.data[source.size as usize..].fill(0);
    *count += 1;
    WRITE_OK
}

fn same_sample_priority(event: &MidiEvent) -> u8 {
    if event.size < 3 {
        return 2;
    }
    match event.data[0] & 0xf0 {
        0xb0 if event.data[1] == 123 => 0,
        0x80 => 1,
        0x90 if event.data[2] == 0 => 1,
        0x90 => 3,
        _ => 2,
    }
}

/// Stable sample-offset sort with note-off/all-notes-off ordering for one
/// realtime block. The caller owns the fixed-capacity event storage.
#[no_mangle]
pub unsafe extern "C" fn hirari_midi_buffer_sort(events: *mut c_void, count: usize) {
    if events.is_null() || count < 2 {
        return;
    }
    let events = events.cast::<MidiEvent>();
    let count = count.min(MIDI_EVENT_CAPACITY);
    for index in 1..count {
        let current = *events.add(index);
        let current_priority = same_sample_priority(&current);
        let mut destination = index;
        while destination > 0 {
            let previous = &*events.add(destination - 1);
            let should_move = previous.sample_offset > current.sample_offset
                || (previous.sample_offset == current.sample_offset
                    && same_sample_priority(previous) > current_priority);
            if !should_move {
                break;
            }
            *events.add(destination) = *previous;
            destination -= 1;
        }
        *events.add(destination) = current;
    }
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "midi_buffer_ops_differential_tests.rs"]
mod differential_tests;
