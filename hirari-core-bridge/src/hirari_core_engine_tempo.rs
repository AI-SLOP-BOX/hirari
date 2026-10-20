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

impl HirariCore {
    /// The engine's Rust-owned tempo map, undo history and diagnostic ring,
    /// plus the sample rate every beat/sample conversion below uses.
    ///
    /// A zero tempo address is exactly the deleted `!m_engine` branch of the
    /// AudioEngine shims: it means the engine handle was never created, so an
    /// operation returns what those branches returned -- false for mutators,
    /// an empty vector for the snapshot getters, no work for the clears.
    #[inline]
    fn tempo_edit(&self) -> Option<EditHandles> {
        if self.engine.is_null() || self.tempo_state == 0 {
            return None;
        }
        Some(EditHandles {
            tempo: self.tempo_state as *mut _,
            undo: self.undo_state as *mut _,
            log: self.log_state as *const _,
            sample_rate: self
                .engine
                .as_ref()
                .map_or(0.0, |engine| engine.get_sample_rate()),
        })
    }

    /// Bumps the revision the UI polls to learn that the project changed,
    /// exactly where the deleted shims did: once, after an edit that
    /// succeeded -- and for the two clears, whenever the engine existed at
    /// all, because their C++ bodies could refuse a value without saying so
    /// and the shim marked the project anyway.
    #[inline]
    fn mark_project_changed(&self) {
        if let Some(engine) = self.engine.as_ref() {
            engine.mark_project_state_changed();
        }
    }

    /// `(beat, bpm, ramp)` triples over every tempo event, as the engine
    /// packed them.
    pub(crate) fn tempo_read_events(&self) -> Vec<f64> {
        let Some(h) = self.tempo_edit() else {
            return Vec::new();
        };
        let events = read_events(h.tempo);
        let mut packed = Vec::with_capacity(events.len() * 3);
        for event in events {
            packed.push(samples_to_beats(h, event.sample_pos));
            packed.push(event.bpm);
            packed.push(if event.ramp { 1.0 } else { 0.0 });
        }
        packed
    }

    /// `(beat, numerator, denominator)` triples over every time signature.
    pub(crate) fn tempo_read_signatures(&self) -> Vec<f64> {
        let Some(h) = self.tempo_edit() else {
            return Vec::new();
        };
        let events = read_signatures(h.tempo);
        let mut packed = Vec::with_capacity(events.len() * 3);
        for event in events {
            packed.push(samples_to_beats(h, event.sample_pos));
            packed.push(f64::from(event.numerator));
            packed.push(f64::from(event.denominator));
        }
        packed
    }

    /// Current transport BPM, straight from the map the audio thread reads.
    ///
    /// Two defaults have to survive the move: no engine object at all answered
    /// `0.0` (the Core wrapper), and an engine object without an engine behind
    /// it answered `120.0f` (the shim).
    pub(crate) fn tempo_read(&self) -> f32 {
        match self.tempo_edit() {
            Some(h) => unsafe { hirari_tempo_map_get_current_bpm(h.tempo) },
            None if !self.engine.is_null() => 120.0,
            None => 0.0,
        }
    }

    /// Drops every time signature, leaving the map's default in place.
    pub(crate) fn tempo_clear_signatures(&self) {
        let Some(h) = self.tempo_edit() else {
            return;
        };
        unsafe {
            hirari_tempo_map_clear_time_signatures(h.tempo);
        }
        self.mark_project_changed();
    }

    /// Resets the map to a single event at `initial_bpm`. A BPM outside
    /// 20..=300 leaves the map untouched -- the C++ body said nothing about
    /// it -- and the project is still marked changed, as the shim did.
    pub(crate) fn tempo_clear_events(&self, initial_bpm: f64) {
        let Some(h) = self.tempo_edit() else {
            return;
        };
        if initial_bpm.is_finite() && (20.0..=300.0).contains(&initial_bpm) {
            unsafe {
                hirari_tempo_map_clear(h.tempo, h.sample_rate, initial_bpm);
                hirari_tempo_map_set_current_bpm(h.tempo, initial_bpm as f32);
            }
        }
        self.mark_project_changed();
    }

    /// Writes a time signature at `beat` and records the pair as one entry.
    pub(crate) fn tempo_write_signature(
        &self,
        beat: f64,
        numerator: u8,
        denominator: u8,
    ) -> bool {
        if !beat.is_finite()
            || beat < 0.0
            || numerator == 0
            || numerator > 32
            || !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32)
        {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        if !h.sample_rate.is_finite() || h.sample_rate <= 0.0 {
            return false;
        }
        let before = signature_at(h.tempo, beat);
        unsafe {
            hirari_tempo_map_add_time_signature(h.tempo, beat, numerator, denominator, h.sample_rate);
        }
        let target = SignatureEvent {
            sample_pos: 0,
            numerator,
            denominator,
            beat,
        };
        record_applied_action(
            h,
            "Time Signature".to_owned(),
            move || apply_signature(h, beat, before),
            move || apply_signature(h, beat, Some(target)),
            false,
        );
        self.mark_project_changed();
        true
    }

    /// Removes the signature at `beat`, with the previous value kept for Undo.
    pub(crate) fn tempo_erase_signature(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        if !h.sample_rate.is_finite() || h.sample_rate <= 0.0 {
            return false;
        }
        let Some(before) = signature_at(h.tempo, beat) else {
            return false;
        };
        perform_action(
            h,
            "Remove Time Signature".to_owned(),
            move || apply_signature(h, beat, Some(before)),
            move || apply_signature(h, beat, None),
        );
        self.mark_project_changed();
        true
    }

    /// Writes a tempo event at `beat`, performing the edit (not merely
    /// recording it) under the position in its history label, as the engine
    /// did with `"Tempo Event " + to_string(sample)`.
    pub(crate) fn tempo_write_event(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() || bpm < 20.0 || bpm > 300.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let sample = beats_to_samples(h, beat);
        let before = event_at(h.tempo, sample);
        let after = TempoEvent {
            sample_pos: sample,
            bpm,
            ramp,
            world_beats: 0.0,
        };
        perform_action(
            h,
            format!("Tempo Event {sample}"),
            move || apply_tempo_event(h, sample, before),
            move || apply_tempo_event(h, sample, Some(after)),
        );
        self.mark_project_changed();
        true
    }

    /// Removes the tempo event at `beat`; beat zero's event may be removed too,
    /// which is why the undo side has to remember whether one was there.
    pub(crate) fn tempo_erase_event(&self, beat: f64) -> bool {
        if !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let sample = beats_to_samples(h, beat);
        let Some(before) = event_at(h.tempo, sample) else {
            return false;
        };
        perform_action(
            h,
            "Remove Tempo Event".to_owned(),
            move || apply_tempo_event(h, sample, Some(before)),
            move || apply_tempo_event(h, sample, None),
        );
        self.mark_project_changed();
        true
    }

    /// Moves an event between two beat positions, refusing positions that are
    /// the same event, the map's first sample, or already occupied.
    pub(crate) fn tempo_move_event(&self, from_beat: f64, to_beat: f64) -> bool {
        if !from_beat.is_finite()
            || !to_beat.is_finite()
            || from_beat <= 0.0
            || to_beat <= 0.0
            || (from_beat - to_beat).abs() < 1.0e-6
        {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let from = beats_to_samples(h, from_beat);
        let to = beats_to_samples(h, to_beat);
        if from == 0 || to == 0 || from == to {
            return false;
        }
        if event_at(h.tempo, from).is_none() || event_at(h.tempo, to).is_some() {
            return false;
        }
        perform_action(
            h,
            "Move Tempo Event".to_owned(),
            move || apply_tempo_move(h, from, to, false),
            move || apply_tempo_move(h, from, to, true),
        );
        self.mark_project_changed();
        true
    }

    /// Sets the ramp target at `beat`: the event at that beat changes BPM and
    /// the event that leads into it takes the ramp, which is why a position
    /// with nothing behind it is refused outright.
    pub(crate) fn tempo_write_transition(&self, beat: f64, bpm: f64, ramp: bool) -> bool {
        if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() || bpm < 20.0 || bpm > 300.0 {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let before = read_events(h.tempo);
        if before.is_empty() {
            return false;
        }

        let mut after = before.clone();
        let found = after
            .iter()
            .position(|event| (event.world_beats - beat).abs() <= 0.001);
        let target_beat = found.map_or(beat, |index| after[index].world_beats);
        let index = match found {
            Some(index) => {
                after[index].bpm = bpm;
                index
            }
            None => {
                after.push(TempoEvent {
                    sample_pos: 0,
                    bpm,
                    ramp: false,
                    world_beats: target_beat,
                });
                after.len() - 1
            }
        };

        if target_beat <= 0.001 {
            after[index].world_beats = 0.0;
            after[index].bpm = bpm;
            after[index].ramp = ramp;
        } else {
            let mut previous: Option<usize> = None;
            for (position, event) in after.iter().enumerate() {
                if event.world_beats < target_beat - 0.001
                    && previous.is_none_or(|held| event.world_beats > after[held].world_beats)
                {
                    previous = Some(position);
                }
            }
            let Some(previous) = previous else {
                return false;
            };
            after[previous].ramp = ramp;
        }

        // `partial_cmp(..., Equal)` orders exactly as the C++ `<` comparator
        // did, including treating 0.0 and -0.0 as equal, so equal events stay
        // in the order they were already in.
        after.sort_by(|left, right| {
            left.world_beats
                .partial_cmp(&right.world_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if !replace_events(h, &after) {
            return false;
        }
        unsafe { hirari_tempo_map_set_current_bpm(h.tempo, after[0].bpm as f32) };
        record_applied_action(
            h,
            "Set Tempo Transition".to_owned(),
            move || apply_tempo_transition(h, &before),
            move || apply_tempo_transition(h, &after),
            false,
        );
        self.mark_project_changed();
        true
    }

    /// The ordinary tempo control: clamps into 20..=300 and writes one
    /// history entry, see [`set_tempo_at`].
    pub(crate) fn tempo_write(&self, bpm: f32) -> bool {
        if !bpm.is_finite() {
            return false;
        }
        let Some(h) = self.tempo_edit() else {
            return false;
        };
        let playhead = self.engine.as_ref().map_or(0, |engine| engine.get_playhead());
        if set_tempo_at(h, playhead, bpm) {
            self.mark_project_changed();
            true
        } else {
            false
        }
    }
}

/// C++ entry point for `HirariUnifiedEngine::set_tempo`, kept only because
/// `tests/timeline_bpm_ghost_contract` drives that method straight off the
/// engine singleton. The body is [`set_tempo_at`] either way, so there is one
/// implementation of the single-entry invariant, not two.
#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_edit_set_tempo(
    tempo: *mut std::ffi::c_void,
    undo: *mut std::ffi::c_void,
    log: *const std::ffi::c_void,
    playhead: u64,
    sample_rate: f64,
    bpm: f32,
) -> bool {
    if tempo.is_null() {
        return false;
    }
    set_tempo_at(
        EditHandles {
            tempo,
            undo,
            log,
            sample_rate,
        },
        playhead,
        bpm,
    )
}
