// Tempo and time-signature editing.
//
// The map, the undo history and the diagnostic ring these operations touch are
// all Rust-owned. They used to live in the C++ engine, which forwarded every
// one of them straight back through cxx and the AudioEngine shims, so an edit
// left Rust only to re-enter the same Rust objects. The engine now hands Rust
// the three addresses it already holds and the work stays here.
//
// Every check, threshold, conversion, history label and failure path below is
// the one the C++ body used, in the same order: an edit rejected before is
// rejected now, and an accepted one lands in the same history entry under the
// same name.

use crate::log_buffer::hirari_log_buffer_post;
use crate::tempo::{
    hirari_tempo_map_add_tempo, hirari_tempo_map_add_time_signature, hirari_tempo_map_beats_to_samples,
    hirari_tempo_map_clear, hirari_tempo_map_clear_time_signatures, hirari_tempo_map_copy_events,
    hirari_tempo_map_copy_signatures, hirari_tempo_map_get_current_bpm, hirari_tempo_map_get_event_at,
    hirari_tempo_map_get_event_count, hirari_tempo_map_get_signature_at,
    hirari_tempo_map_get_signature_count, hirari_tempo_map_remove_tempo,
    hirari_tempo_map_remove_time_signature, hirari_tempo_map_replace_events_at_beats,
    hirari_tempo_map_samples_to_beats, hirari_tempo_map_set_current_bpm,
};
use crate::undo_transaction::{
    hirari_undo_manager_discard_native_callback, hirari_undo_manager_invoke_native_callback,
    hirari_undo_manager_record, hirari_undo_manager_register_native_callback,
    hirari_undo_manager_timestamp_ms,
};

/// Packed tempo event, laid out as `src/core/rust_ffi.hpp` declares
/// `HirariTempoEvent`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TempoEvent {
    sample_pos: u64,
    bpm: f64,
    ramp: bool,
    world_beats: f64,
}

/// Packed time-signature event, laid out as `HirariTimeSignatureEvent`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SignatureEvent {
    sample_pos: u64,
    numerator: u8,
    denominator: u8,
    beat: f64,
}

// rust_ffi.hpp asserts these same two sizes against its own structs. If the
// halves ever disagreed, every field after the first would be read from the
// wrong offset, so both are pinned here as well.
const _: () = {
    assert!(std::mem::size_of::<TempoEvent>() == 32);
    assert!(std::mem::size_of::<SignatureEvent>() == 24);
};

/// The three Rust-owned objects an edit needs, addressed once per call.
#[derive(Clone, Copy)]
struct EditHandles {
    tempo: *mut std::ffi::c_void,
    undo: *mut std::ffi::c_void,
    log: *const std::ffi::c_void,
    sample_rate: f64,
}

/// The count/copy snapshot dance the C++ `readTempoSnapshot` helper ran: a
/// writer racing the read can report more entries than the vector was sized
/// for, so the copy is retried at the larger size rather than truncating.
fn read_events(tempo: *mut std::ffi::c_void) -> Vec<TempoEvent> {
    let mut events = vec![TempoEvent::default(); unsafe {
        hirari_tempo_map_get_event_count(tempo)
    }];
    loop {
        let output = if events.is_empty() {
            std::ptr::null_mut()
        } else {
            events.as_mut_ptr().cast()
        };
        let count = unsafe { hirari_tempo_map_copy_events(tempo, output, events.len()) };
        if count <= events.len() {
            events.truncate(count);
            return events;
        }
        events.resize(count, TempoEvent::default());
    }
}

/// The signature half of the same snapshot dance.
fn read_signatures(tempo: *mut std::ffi::c_void) -> Vec<SignatureEvent> {
    let mut events = vec![SignatureEvent::default(); unsafe {
        hirari_tempo_map_get_signature_count(tempo)
    }];
    loop {
        let output = if events.is_empty() {
            std::ptr::null_mut()
        } else {
            events.as_mut_ptr().cast()
        };
        let count = unsafe { hirari_tempo_map_copy_signatures(tempo, output, events.len()) };
        if count <= events.len() {
            events.truncate(count);
            return events;
        }
        events.resize(count, SignatureEvent::default());
    }
}

/// The event sitting at `sample_pos`, or nothing -- the shape of the C++
/// `readTempoEventAt` helper.
fn event_at(tempo: *mut std::ffi::c_void, sample_pos: u64) -> Option<TempoEvent> {
    let mut event = TempoEvent::default();
    let found = unsafe {
        hirari_tempo_map_get_event_at(tempo, sample_pos, (&mut event as *mut TempoEvent).cast())
    };
    found.then_some(event)
}

/// The signature sitting at `beat`, or nothing -- `readTempoSignatureAt`.
fn signature_at(tempo: *mut std::ffi::c_void, beat: f64) -> Option<SignatureEvent> {
    let mut event = SignatureEvent::default();
    let found = unsafe {
        hirari_tempo_map_get_signature_at(tempo, beat, (&mut event as *mut SignatureEvent).cast())
    };
    found.then_some(event)
}

#[inline]
fn beats_to_samples(h: EditHandles, beat: f64) -> u64 {
    unsafe { hirari_tempo_map_beats_to_samples(h.tempo, beat, h.sample_rate) }
}

#[inline]
fn samples_to_beats(h: EditHandles, sample_pos: u64) -> f64 {
    unsafe { hirari_tempo_map_samples_to_beats(h.tempo, sample_pos, h.sample_rate) }
}

/// Replaces the whole event list by beat position. An empty list cannot be
/// written -- the map keeps at least the default event -- so it is refused
/// before the FFI is touched, as the C++ guard did.
fn replace_events(h: EditHandles, events: &[TempoEvent]) -> bool {
    !events.is_empty()
        && unsafe {
            hirari_tempo_map_replace_events_at_beats(
                h.tempo,
                events.as_ptr().cast(),
                events.len(),
                h.sample_rate,
            )
        }
}

/// The lambda registered with the history by both time-signature operations:
/// re-add the recorded signature, or drop whatever sits at that beat when the
/// recorded state was "none". The setter alone spares beat zero, where the
/// always-present default signature was never removed.
fn apply_signature(h: EditHandles, beat: f64, event: Option<SignatureEvent>) {
    match event {
        Some(event) => unsafe {
            hirari_tempo_map_add_time_signature(
                h.tempo,
                beat,
                event.numerator,
                event.denominator,
                h.sample_rate,
            );
        },
        None if beat > 0.0 => {
            let _ = unsafe { hirari_tempo_map_remove_time_signature(h.tempo, beat, h.sample_rate) };
        }
        None => {}
    }
}

/// The lambda behind every tempo-event history entry: put the event back, and
/// when it belongs at the very start of the map, move the displayed BPM with
/// it; with no recorded event, drop whatever sits at that position.
fn apply_tempo_event(h: EditHandles, sample: u64, event: Option<TempoEvent>) {
    match event {
        Some(event) => {
            unsafe {
                hirari_tempo_map_add_tempo(h.tempo, sample, event.bpm, h.sample_rate, event.ramp);
            }
            if sample == 0 {
                unsafe { hirari_tempo_map_set_current_bpm(h.tempo, event.bpm as f32) };
            }
        }
        None => {
            unsafe {
                hirari_tempo_map_remove_tempo(h.tempo, sample, h.sample_rate);
            }
        }
    }
}

/// Undo/redo body of a move: lift the event out of whichever end the direction
/// is leaving and set it down at the other, so a redo travels forward and an
/// undo travels back.
fn apply_tempo_move(h: EditHandles, from: u64, to: u64, forward: bool) {
    let remove_sample = if forward { from } else { to };
    let Some(event) = event_at(h.tempo, remove_sample) else {
        return;
    };
    unsafe {
        hirari_tempo_map_remove_tempo(h.tempo, remove_sample, h.sample_rate);
    }
    let add_sample = if forward { to } else { from };
    unsafe {
        hirari_tempo_map_add_tempo(h.tempo, add_sample, event.bpm, h.sample_rate, event.ramp);
    }
}

/// Shared body of both halves of a transition's history entry: write the list,
/// then follow it with the displayed BPM taken from its first event. A refused
/// write means nothing moves at all.
fn apply_tempo_transition(h: EditHandles, events: &[TempoEvent]) {
    if !replace_events(h, events) {
        return;
    }
    if let Some(front) = events.first() {
        unsafe { hirari_tempo_map_set_current_bpm(h.tempo, front.bpm as f32) };
    }
}

/// Hands a callback to the history the way the C++ facade's `registerCallback`
/// did: boxed, passed by address, and released again if the history did not
/// take it. The returned id is what later invokes or discards it.
fn register_callback(h: EditHandles, callback: impl Fn() + 'static) -> u64 {
    let context = Box::into_raw(Box::new(Box::new(callback) as Box<dyn Fn()>))
        as *mut std::ffi::c_void;
    let id = unsafe {
        hirari_undo_manager_register_native_callback(
            h.undo,
            context,
            Some(invoke_callback),
            Some(destroy_callback),
        )
    };
    if id == 0 {
        unsafe { drop(Box::from_raw(context.cast::<Box<dyn Fn()>>())) };
    }
    id
}

unsafe extern "C" fn invoke_callback(context: *mut std::ffi::c_void) {
    // The C++ facade caught every exception crossing this boundary ("Never
    // unwind across the Rust ABI"); a panic leaving `extern "C"` aborts the
    // process instead, so it is caught here for the same reason -- a failing
    // undo callback must not take the engine down with it.
    let callback = unsafe { &*context.cast::<Box<dyn Fn()>>() };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
}

unsafe extern "C" fn destroy_callback(context: *mut std::ffi::c_void) {
    unsafe { drop(Box::from_raw(context.cast::<Box<dyn Fn()>>())) };
}

fn discard_callback(h: EditHandles, id: u64) {
    if id != 0 {
        unsafe { hirari_undo_manager_discard_native_callback(h.undo, id) };
    }
}

fn record(h: EditHandles, name: &str, undo_id: u64, redo_id: u64, coalesce: bool) -> bool {
    unsafe {
        hirari_undo_manager_record(
            h.undo,
            name.as_ptr(),
            name.len(),
            undo_id,
            redo_id,
            hirari_undo_manager_timestamp_ms(),
            coalesce,
        )
    }
}

/// The facade's `performAction`: register both sides, run the redo now (the
/// edit is applied by this call, not by its caller), record the pair with
/// coalescing on, and only then log it. Any failure discards both ids and
/// leaves nothing registered and nothing recorded, as before.
fn perform_action(
    h: EditHandles,
    name: String,
    undo: impl Fn() + 'static,
    redo: impl Fn() + 'static,
) {
    let undo_id = register_callback(h, undo);
    let redo_id = register_callback(h, redo);
    if undo_id == 0
        || redo_id == 0
        || !unsafe { hirari_undo_manager_invoke_native_callback(h.undo, redo_id) }
        || !record(h, &name, undo_id, redo_id, true)
    {
        discard_callback(h, undo_id);
        discard_callback(h, redo_id);
        return;
    }
    post_action_log(h, "UNDO | PERFORMED | ", &name);
}

/// The facade's `recordAppliedAction`: for an edit the caller already made, so
/// nothing is invoked here -- the pair is recorded (coalescing off unless the
/// caller asked for it) and logged.
fn record_applied_action(
    h: EditHandles,
    name: String,
    undo: impl Fn() + 'static,
    redo: impl Fn() + 'static,
    coalesce: bool,
) {
    let undo_id = register_callback(h, undo);
    let redo_id = register_callback(h, redo);
    if undo_id == 0 || redo_id == 0 || !record(h, &name, undo_id, redo_id, coalesce) {
        discard_callback(h, undo_id);
        discard_callback(h, redo_id);
        return;
    }
    post_action_log(h, "UNDO | RECORDED | ", &name);
}

/// The facade's `postActionLog`: prefix and name, clipped to the ring's
/// 95-byte text field, posted at component 0xA001 at the same level. The clip
/// is by byte count, as C++ `substr` was; the raw pointer keeps a cut that
/// lands mid-character from panicking here.
fn post_action_log(h: EditHandles, prefix: &str, name: &str) {
    let mut message = String::with_capacity(prefix.len() + name.len());
    message.push_str(prefix);
    message.push_str(name);
    let length = message.len().min(95);
    unsafe { hirari_log_buffer_post(h.log, 0, 0xA001, message.as_ptr(), length) };
}

/// `recordAppliedAction` for an edit whose state lives outside the tempo
/// controls: the same history and log handles, coalescing off, and the same
/// `UNDO | RECORDED |` diagnostics line.
pub(crate) fn record_external_action(
    undo: *mut std::ffi::c_void,
    log: *const std::ffi::c_void,
    name: &str,
    undo_callback: impl Fn() + 'static,
    redo_callback: impl Fn() + 'static,
) {
    record_external_action_with(undo, log, name, undo_callback, redo_callback, false);
}

/// The same entry as `record_external_action`, for edits whose native call
/// passed `recordAppliedAction(..., true)` and folded a repeated edit inside
/// the coalescing window into one history entry.
pub(crate) fn record_external_action_coalesced(
    undo: *mut std::ffi::c_void,
    log: *const std::ffi::c_void,
    name: &str,
    undo_callback: impl Fn() + 'static,
    redo_callback: impl Fn() + 'static,
) {
    record_external_action_with(undo, log, name, undo_callback, redo_callback, true);
}

fn record_external_action_with(
    undo: *mut std::ffi::c_void,
    log: *const std::ffi::c_void,
    name: &str,
    undo_callback: impl Fn() + 'static,
    redo_callback: impl Fn() + 'static,
    coalesce: bool,
) {
    record_applied_action(
        EditHandles {
            tempo: std::ptr::null_mut(),
            undo,
            log,
            sample_rate: 0.0,
        },
        name.to_owned(),
        undo_callback,
        redo_callback,
        coalesce,
    );
}

/// The whole of the C++ `set_tempo`: clamp the control, write an event at the
/// playhead, move the displayed BPM with it, and put both halves in one
/// history entry. Carried over verbatim from the comment that guarded it:
///
/// The ordinary tempo control writes an event at the current playhead. Keep
/// that event and the transport's displayed BPM in one history entry;
/// otherwise Undo changes the number shown by the UI but leaves the tempo map
/// used by beat/sample conversion behind.
fn set_tempo_at(h: EditHandles, playhead: u64, bpm: f32) -> bool {
    if !bpm.is_finite() {
        return false;
    }
    let bounded = bpm.clamp(20.0, 300.0);
    if !h.sample_rate.is_finite() || h.sample_rate <= 0.0 {
        return false;
    }
    let sample = playhead;
    let before_event = event_at(h.tempo, sample);
    let before_tempo = unsafe { hirari_tempo_map_get_current_bpm(h.tempo) };

    unsafe {
        hirari_tempo_map_add_tempo(h.tempo, sample, f64::from(bounded), h.sample_rate, false);
        hirari_tempo_map_set_current_bpm(h.tempo, bounded);
    }

    record_applied_action(
        h,
        "Set Tempo".to_owned(),
        move || {
            match before_event {
                Some(event) => unsafe {
                    hirari_tempo_map_add_tempo(
                        h.tempo,
                        sample,
                        event.bpm,
                        h.sample_rate,
                        event.ramp,
                    );
                },
                None if sample != 0 => {
                    let _ = unsafe {
                        hirari_tempo_map_remove_tempo(h.tempo, sample, h.sample_rate)
                    };
                }
                None => {}
            }
            unsafe { hirari_tempo_map_set_current_bpm(h.tempo, before_tempo) };
        },
        move || unsafe {
            hirari_tempo_map_add_tempo(h.tempo, sample, f64::from(bounded), h.sample_rate, false);
            hirari_tempo_map_set_current_bpm(h.tempo, bounded);
        },
        false,
    );
    true
}

#[path = "hirari_core_engine_tempo_impl.rs"]
mod impl_;
pub(crate) use impl_::*;
