//! CoreMIDI polling and project-persisted MIDI CC learn bindings.

use hirari_core_bridge::{project_contracts::MidiLearnMappingContract, HirariCore};
use serde::Deserialize;
use slint::{ComponentHandle, Model, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use crate::slint_ui::{sync_tracks_from_engine, AppWindow, MidiActions, Z_Track};
use hirari_core_bridge::project_contracts::MidiNoteContract;

#[derive(Clone, Debug, Default)]
pub struct MidiRecordingRequest {
    pub active: bool,
    pub audio_capture: bool,
    pub target_track_ids: Vec<u32>,
    pub start_sample: u64,
    pub start_host_time: u64,
    pub dropped_events_at_start: u64,
    pub stop_host_time: Option<u64>,
    pub punch_range: Option<(u64, u64)>,
    pub loop_range: Option<(u64, u64)>,
}

pub type SharedMidiRecordingRequest = Rc<RefCell<Option<MidiRecordingRequest>>>;

#[derive(Deserialize)]
struct InputPacket {
    source_unique_id: i32,
    #[serde(default)]
    host_time: u64,
    data_hex: String,
}

pub fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<slint::VecModel<Z_Track>>,
    recording_request: SharedMidiRecordingRequest,
) -> Timer {
    let armed_macro = Rc::new(Cell::new(None::<u32>));
    let input_started = Rc::new(Cell::new(false));

    ui.global::<MidiActions>().on_arm_midi_learn({
        let weak = ui.as_weak();
        let armed_macro = armed_macro.clone();
        move |macro_index| {
            if !(0..8).contains(&macro_index) {
                return;
            }
            armed_macro.set(Some(macro_index as u32));
            if let Some(ui) = weak.upgrade() {
                ui.set_midi_input_status(
                    format!("MOVE A MIDI CC CONTROL · MACRO {} ARMED", macro_index + 1).into(),
                );
            }
        }
    });

    let timer = Timer::default();
    let retrospective_events = Rc::new(RefCell::new(VecDeque::<BufferedMidiEvent>::new()));
    let retrospective_drop_baseline = Rc::new(Cell::new(0_u64));
    let retrospective_page_active = Rc::new(Cell::new(false));
    ui.global::<MidiActions>().on_insert_retrospective({
        let weak = ui.as_weak();
        let core = core.clone();
        let tracks = tracks.clone();
        let events = retrospective_events.clone();
        let drop_baseline = retrospective_drop_baseline.clone();
        move |track_id| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_is_rec() {
                ui.set_last_action("RETROSPECTIVE MIDI INSERT BLOCKED DURING RECORDING".into());
                return;
            }
            if core.midi_dropped_input_events() > drop_baseline.get() {
                ui.set_last_action(
                    "RETROSPECTIVE MIDI BUFFER RESET: INPUT OVERFLOW · PLAY AGAIN THEN INSERT"
                        .into(),
                );
                events.borrow_mut().clear();
                drop_baseline.set(core.midi_dropped_input_events());
                return;
            }
            match insert_retrospective_notes(track_id, &events, &core, &tracks) {
                Ok(note_count) => {
                    events.borrow_mut().clear();
                    sync_tracks_from_engine(&tracks, &core);
                    ui.set_last_action(
                        format!("RETROSPECTIVE MIDI INSERTED: {note_count} NOTES · UNDO AVAILABLE")
                            .into(),
                    );
                }
                Err(message) => ui.set_last_action(message.into()),
            }
        }
    });
    timer.start(TimerMode::Repeated, Duration::from_millis(32), {
        let weak = ui.as_weak();
        let core = core.clone();
        let tracks = tracks.clone();
        let armed_macro = armed_macro.clone();
        let input_started = input_started.clone();
        let recording_request = recording_request.clone();
        let retrospective_events = retrospective_events.clone();
        let retrospective_drop_baseline = retrospective_drop_baseline.clone();
        let retrospective_page_active = retrospective_page_active.clone();
        let mut active_take: Option<MidiTake> = None;
        let mut running_status_by_source = HashMap::<i32, Option<u8>>::new();
        move || {
            let Some(ui) = weak.upgrade() else {
                if input_started.replace(false) {
                    core.stop_midi_input();
                }
                return;
            };
            let mut request = recording_request.borrow().clone();
            if !ui.get_is_rec() && request.as_ref().is_some_and(|request| request.active) {
                if let Some(mut stopped) = request.clone() {
                    stopped.active = false;
                    stopped.stop_host_time = Some(core.midi_host_time_now());
                    *recording_request.borrow_mut() = Some(stopped.clone());
                    request = Some(stopped);
                }
            }
            if request.as_ref().is_some_and(|request| {
                request.active
                    && !request.audio_capture
                    && request.punch_range.is_some_and(|(_, punch_out)| {
                        core.is_playing()
                            && request.start_sample.saturating_add(
                                core.midi_host_time_delta_samples(
                                    request.start_host_time,
                                    core.midi_host_time_now(),
                                    core.get_sample_rate(),
                                ),
                            ) >= punch_out
                    })
            }) {
                if let Some(mut stopped) = request.clone() {
                    stopped.active = false;
                    stopped.stop_host_time = Some(core.midi_host_time_now());
                    *recording_request.borrow_mut() = Some(stopped.clone());
                    request = Some(stopped);
                    ui.set_is_rec(false);
                    ui.set_recording_lifecycle("Finalizing".into());
                    ui.set_last_action("MIDI PUNCH OUT: FINALIZING TAKE".into());
                }
            }
            let should_listen =
                ui.get_bot_view() == 17 || request.is_some() || active_take.is_some();
            let retrospective_active = ui.get_bot_view() == 17;
            if retrospective_active && !retrospective_page_active.replace(true) {
                retrospective_events.borrow_mut().clear();
                retrospective_drop_baseline.set(core.midi_dropped_input_events());
            } else if !retrospective_active && retrospective_page_active.replace(false) {
                retrospective_events.borrow_mut().clear();
            }
            if !should_listen {
                if input_started.replace(false) {
                    core.stop_midi_input();
                }
                armed_macro.set(None);
                return;
            }
            if !input_started.get() {
                if !core.start_midi_input() {
                    ui.set_midi_input_status("MIDI INPUT UNAVAILABLE".into());
                    if request.as_ref().is_some_and(|request| request.active) {
                        ui.set_last_action("MIDI RECORD FAILED: MIDI INPUT UNAVAILABLE".into());
                    }
                    return;
                }
                input_started.set(true);
                if ui.get_bot_view() == 17 {
                    ui.set_midi_input_status(
                        "MIDI INPUT READY · 30-SECOND RETROSPECTIVE BUFFER ACTIVE".into(),
                    );
                }
            }

            if active_take.is_none() {
                if let Some(request) = request.as_ref().filter(|request| request.active) {
                    active_take = Some(MidiTake::new(request.clone()));
                }
            }

            let packets = serde_json::from_str::<Vec<InputPacket>>(&core.poll_midi_input_json())
                .unwrap_or_default();
            for packet in packets {
                let device_id = format!("coremidi:{}:input", packet.source_unique_id);
                let running_status = running_status_by_source
                    .entry(packet.source_unique_id)
                    .or_default();
                for message in decode_midi_messages(&packet.data_hex, running_status) {
                    let event_host_time = if packet.host_time == 0 {
                        core.midi_host_time_now()
                    } else {
                        packet.host_time
                    };
                    let within_recording_window = request.as_ref().is_some_and(|request| {
                        event_host_time >= request.start_host_time
                            && (request.active
                                || request.stop_host_time.is_some_and(|stop_host_time| {
                                    event_host_time <= stop_host_time
                                }))
                    });
                    if within_recording_window {
                        if let Some(take) = active_take.as_mut() {
                            take.consume(packet.source_unique_id, event_host_time, message, &core);
                        }
                    }
                    if retrospective_active
                        && matches!(
                            message,
                            MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. }
                        )
                    {
                        let mut buffered = retrospective_events.borrow_mut();
                        buffered.push_back(BufferedMidiEvent {
                            source_id: packet.source_unique_id,
                            host_time: event_host_time,
                            message,
                        });
                        while buffered.len() > MAX_RETROSPECTIVE_MIDI_EVENTS {
                            buffered.pop_front();
                        }
                        while buffered.front().is_some_and(|event| {
                            core.midi_host_time_delta_samples(
                                event.host_time,
                                event_host_time,
                                core.get_sample_rate(),
                            ) > core.get_sample_rate().max(1.0) as u64 * RETROSPECTIVE_MIDI_SECONDS
                        }) {
                            buffered.pop_front();
                        }
                    }
                    let MidiMessage::ControlChange {
                        channel,
                        controller,
                        value,
                    } = message
                    else {
                        continue;
                    };
                    let learned = armed_macro.get().and_then(|macro_index| {
                        let mapping_id = format!(
                            "midi:{}:{}:{}:macro:{}",
                            packet.source_unique_id, channel, controller, macro_index
                        );
                        let mapping = MidiLearnMappingContract {
                            mapping_id,
                            device_id: device_id.clone(),
                            channel,
                            controller: controller as u16,
                            target_instance_id: format!("macro:{macro_index}"),
                            target_parameter_id: "value".to_owned(),
                            min: 0.0,
                            max: 1.0,
                            curve: 0.0,
                            pickup: false,
                            macro_group: None,
                        };
                        match core.add_midi_learn_mapping(mapping) {
                            Ok(()) => {
                                armed_macro.set(None);
                                Some(macro_index)
                            }
                            Err(error) => {
                                ui.set_midi_input_status(
                                    format!("MIDI LEARN FAILED: {error}").into(),
                                );
                                None
                            }
                        }
                    });

                    core.handle_midi_cc_from_device(&device_id, channel, controller, value);
                    let status = if let Some(macro_index) = learned {
                        format!(
                            "LEARNED CH {} · CC {} → MACRO {}",
                            channel + 1,
                            controller,
                            macro_index + 1
                        )
                    } else {
                        format!("MIDI CH {} · CC {} = {}", channel + 1, controller, value)
                    };
                    if ui.get_bot_view() == 17 {
                        ui.set_midi_input_status(status.into());
                    }
                }
            }

            let current_request = recording_request.borrow().clone();
            if let Some(take) = active_take.as_mut() {
                if let Some(request) = current_request.as_ref().filter(|request| !request.active) {
                    let stop_host_time = request
                        .stop_host_time
                        .unwrap_or_else(|| core.midi_host_time_now());
                    take.finish(stop_host_time, &core);
                    let input_overflowed =
                        core.midi_dropped_input_events() > request.dropped_events_at_start;
                    let existing =
                        serde_json::from_str::<Vec<MidiNoteContract>>(&core.midi_notes_json())
                            .unwrap_or_default();
                    let valid_midi_tracks = (0..tracks.row_count())
                        .filter_map(|row| tracks.row_data(row))
                        .filter(|track| {
                            matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument")
                        })
                        .map(|track| track.id.max(0) as u32)
                        .collect::<std::collections::HashSet<_>>();
                    let mut recorded_notes = take.notes.drain(..).collect::<Vec<_>>();
                    recorded_notes.retain(|note| valid_midi_tracks.contains(&note.track_id));
                    take.committed_note_count = recorded_notes.len();
                    let mut notes = existing;
                    notes.extend(recorded_notes);
                    let valid_targets = take
                        .target_track_ids
                        .iter()
                        .any(|id| valid_midi_tracks.contains(id));
                    let committed = if valid_targets && !input_overflowed {
                        core.begin_undo_transaction("Record MIDI Take");
                        if core.replace_midi_note_contracts(notes, true) {
                            if core.end_undo_transaction() {
                                true
                            } else {
                                let _ = core.abort_undo_transaction();
                                false
                            }
                        } else {
                            let _ = core.abort_undo_transaction();
                            false
                        }
                    } else {
                        false
                    };
                    if committed {
                        sync_tracks_from_engine(&tracks, &core);
                        ui.set_recording_lifecycle("Committed".into());
                        ui.set_last_action(
                            format!(
                                "MIDI RECORDED: {} NOTES ON {} TRACK(S)",
                                take.committed_note_count,
                                take.target_track_ids.len()
                            )
                            .into(),
                        );
                    } else if input_overflowed {
                        ui.set_recording_lifecycle("Failed".into());
                        ui.set_last_action(
                            "MIDI RECORD FAILED: INPUT OVERFLOW · TAKE DISCARDED".into(),
                        );
                    } else if take.committed_note_count > 0 {
                        sync_tracks_from_engine(&tracks, &core);
                        ui.set_recording_lifecycle("Failed".into());
                        ui.set_last_action("MIDI RECORD FAILED: NOTE TAKE REJECTED".into());
                    } else {
                        ui.set_recording_lifecycle("Stopped".into());
                        ui.set_last_action("MIDI RECORD: NO NOTES RECEIVED".into());
                    }
                    active_take = None;
                    *recording_request.borrow_mut() = None;
                }
            }
        }
    });
    timer
}

#[derive(Clone, Copy, Debug)]
enum MidiMessage {
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    NoteOn {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        pitch: u8,
    },
}

const RETROSPECTIVE_MIDI_SECONDS: u64 = 30;
const MAX_RETROSPECTIVE_MIDI_EVENTS: usize = 8192;

#[derive(Clone, Copy)]
struct BufferedMidiEvent {
    source_id: i32,
    host_time: u64,
    message: MidiMessage,
}

fn insert_retrospective_notes(
    track_id: i32,
    buffered_events: &Rc<RefCell<VecDeque<BufferedMidiEvent>>>,
    core: &HirariCore,
    tracks: &slint::VecModel<Z_Track>,
) -> Result<usize, String> {
    let track_id = u32::try_from(track_id)
        .map_err(|_| "RETROSPECTIVE MIDI INSERT FAILED: INVALID TRACK".to_owned())?;
    let is_midi_track = (0..tracks.row_count())
        .filter_map(|row| tracks.row_data(row))
        .any(|track| {
            track.id == track_id as i32
                && matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument")
        });
    if !is_midi_track {
        return Err("RETROSPECTIVE MIDI INSERT FAILED: SELECT A MIDI TRACK".to_owned());
    }
    let core_now = core.midi_host_time_now();
    let sample_rate = core.get_sample_rate();
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err("RETROSPECTIVE MIDI INSERT FAILED: INVALID SAMPLE RATE".to_owned());
    }
    let mut buffer = buffered_events.borrow_mut();
    while buffer.front().is_some_and(|event| {
        core.midi_host_time_delta_samples(event.host_time, core_now, sample_rate)
            > sample_rate as u64 * RETROSPECTIVE_MIDI_SECONDS
    }) {
        buffer.pop_front();
    }
    let mut events = buffer.iter().copied().collect::<Vec<_>>();
    drop(buffer);
    events.sort_by_key(|event| event.host_time);
    let Some(first_event) = events.first() else {
        return Err("RETROSPECTIVE MIDI: NO RECENT NOTE INPUT".to_owned());
    };
    let elapsed = core.midi_host_time_delta_samples(first_event.host_time, core_now, sample_rate);
    let current_sample = core.get_playhead();
    let loop_range = if core.is_loop_enabled() {
        let start = core.cycle_start_sample();
        let end = core.cycle_end_sample();
        (end > start).then_some((start, end))
    } else {
        None
    };
    let start_sample = if let Some((loop_start, loop_end)) = loop_range {
        let span = loop_end - loop_start;
        let current_offset = current_sample.saturating_sub(loop_start) % span;
        let elapsed_offset = elapsed % span;
        let start_offset = if current_offset >= elapsed_offset {
            current_offset - elapsed_offset
        } else {
            span - (elapsed_offset - current_offset)
        };
        loop_start + start_offset
    } else {
        current_sample.saturating_sub(elapsed)
    };
    let mut take = MidiTake::new(MidiRecordingRequest {
        active: true,
        audio_capture: false,
        target_track_ids: vec![track_id],
        start_sample,
        start_host_time: first_event.host_time,
        dropped_events_at_start: core.midi_dropped_input_events(),
        stop_host_time: Some(core_now),
        punch_range: None,
        loop_range,
    });
    for event in events {
        take.consume(event.source_id, event.host_time, event.message, core);
    }
    take.finish(core_now, core);
    let recorded_count = take.notes.len();
    if recorded_count == 0 {
        return Err("RETROSPECTIVE MIDI: NO COMPLETE NOTES IN BUFFER".to_owned());
    }
    let mut notes =
        serde_json::from_str::<Vec<MidiNoteContract>>(&core.midi_notes_json()).unwrap_or_default();
    notes.extend(take.notes);
    core.begin_undo_transaction("Insert Retrospective MIDI");
    if !core.replace_midi_note_contracts(notes, true) {
        let _ = core.abort_undo_transaction();
        return Err("RETROSPECTIVE MIDI INSERT FAILED: CORE REJECTED NOTES".to_owned());
    }
    if !core.end_undo_transaction() {
        let _ = core.abort_undo_transaction();
        return Err("RETROSPECTIVE MIDI INSERT FAILED: UNDO COMMIT".to_owned());
    }
    Ok(recorded_count)
}

#[derive(Clone, Copy)]
struct OpenNote {
    start_sample: u64,
    start_elapsed_samples: u64,
    velocity: u8,
}

struct MidiTake {
    target_track_ids: Vec<u32>,
    start_host_time: u64,
    start_sample: u64,
    punch_range: Option<(u64, u64)>,
    loop_range: Option<(u64, u64)>,
    open_notes: HashMap<(i32, u8, u8), VecDeque<OpenNote>>,
    notes: Vec<MidiNoteContract>,
    committed_note_count: usize,
}

impl MidiTake {
    fn new(request: MidiRecordingRequest) -> Self {
        Self {
            target_track_ids: request.target_track_ids,
            start_host_time: request.start_host_time,
            start_sample: request.start_sample,
            punch_range: request.punch_range,
            loop_range: request.loop_range,
            open_notes: HashMap::new(),
            notes: Vec::new(),
            committed_note_count: 0,
        }
    }

    fn elapsed_at(&self, event_host_time: u64, core: &HirariCore) -> u64 {
        core.midi_host_time_delta_samples(
            self.start_host_time,
            event_host_time,
            core.get_sample_rate(),
        )
    }

    fn sample_at_elapsed(&self, elapsed: u64) -> u64 {
        let unwrapped = self.start_sample.saturating_add(elapsed);
        if let Some((loop_start, loop_end)) = self.loop_range {
            let span = loop_end.saturating_sub(loop_start);
            if span > 0 {
                let loop_origin = if self.start_sample >= loop_end {
                    loop_start
                } else {
                    self.start_sample
                };
                let position = loop_origin.saturating_add(elapsed);
                return if position >= loop_end {
                    loop_start.saturating_add(position.saturating_sub(loop_end) % span)
                } else {
                    position
                };
            }
        }
        unwrapped
    }

    fn consume(
        &mut self,
        source_id: i32,
        event_host_time: u64,
        message: MidiMessage,
        core: &HirariCore,
    ) {
        let elapsed_samples = self.elapsed_at(event_host_time, core);
        let sample = self.sample_at_elapsed(elapsed_samples);
        match message {
            MidiMessage::NoteOn {
                channel,
                pitch,
                velocity,
            } => {
                if velocity == 0 {
                    self.end_note(source_id, channel, pitch, elapsed_samples);
                } else if self.punch_range.is_some_and(|(start, end)| {
                    sample < start
                        || sample >= end
                        || elapsed_samples >= end.saturating_sub(self.start_sample)
                }) {
                    return;
                } else {
                    let key = (source_id, channel, pitch);
                    let queue = self.open_notes.entry(key).or_default();
                    if queue.len() < 64 {
                        queue.push_back(OpenNote {
                            start_sample: sample,
                            start_elapsed_samples: elapsed_samples,
                            velocity,
                        });
                    }
                }
            }
            MidiMessage::NoteOff { channel, pitch } => {
                self.end_note(source_id, channel, pitch, elapsed_samples)
            }
            MidiMessage::ControlChange { .. } => {}
        }
    }

    fn end_note(&mut self, source_id: i32, channel: u8, pitch: u8, end_elapsed_samples: u64) {
        let key = (source_id, channel, pitch);
        let Some(note) = self.open_notes.get_mut(&key).and_then(VecDeque::pop_front) else {
            return;
        };
        if self.open_notes.get(&key).is_some_and(VecDeque::is_empty) {
            self.open_notes.remove(&key);
        }
        let punch_limited_end = self
            .punch_range
            .map(|(_, punch_out)| punch_out.saturating_sub(self.start_sample))
            .map(|punch_elapsed| end_elapsed_samples.min(punch_elapsed))
            .unwrap_or(end_elapsed_samples);
        let length_samples = punch_limited_end
            .saturating_sub(note.start_elapsed_samples)
            .max(1);
        self.push_note(channel, pitch, note, length_samples);
    }

    fn finish(&mut self, stop_host_time: u64, core: &HirariCore) {
        let stop_elapsed_samples = self.elapsed_at(stop_host_time, core);
        let stop_elapsed_samples = self
            .punch_range
            .map(|(_, punch_out)| {
                stop_elapsed_samples.min(punch_out.saturating_sub(self.start_sample))
            })
            .unwrap_or(stop_elapsed_samples);
        let open = std::mem::take(&mut self.open_notes);
        for ((_, channel, pitch), queue) in open {
            for note in queue {
                let length_samples = stop_elapsed_samples
                    .saturating_sub(note.start_elapsed_samples)
                    .max(1);
                self.push_note(channel, pitch, note, length_samples);
            }
        }
        self.committed_note_count = self.notes.len();
    }

    fn push_note(&mut self, channel: u8, pitch: u8, note: OpenNote, length_samples: u64) {
        let length = length_samples.max(1);
        if let Some((loop_start, loop_end)) = self.loop_range {
            let span = loop_end.saturating_sub(loop_start);
            if span > 0 {
                let mut cursor = note.start_sample;
                let mut remaining = length;
                while remaining > 0 {
                    if cursor >= loop_end {
                        cursor = loop_start.saturating_add(cursor.saturating_sub(loop_end) % span);
                    }
                    let until_loop_end = loop_end.saturating_sub(cursor);
                    if until_loop_end == 0 {
                        cursor = loop_start;
                        continue;
                    }
                    let segment_length = remaining.min(until_loop_end);
                    self.push_note_segment(
                        channel,
                        pitch,
                        OpenNote {
                            start_sample: cursor,
                            ..note
                        },
                        segment_length,
                    );
                    remaining -= segment_length;
                    cursor = cursor.saturating_add(segment_length);
                    if cursor >= loop_end {
                        cursor = loop_start;
                    }
                }
                return;
            }
        }
        self.push_note_segment(channel, pitch, note, length);
    }

    fn push_note_segment(&mut self, channel: u8, pitch: u8, note: OpenNote, length_samples: u64) {
        for track_id in &self.target_track_ids {
            self.notes.push(MidiNoteContract {
                region_id: 0,
                track_id: *track_id,
                pitch,
                midi_channel: channel,
                articulation: 0,
                velocity: note.velocity.clamp(1, 127),
                start_sample: note.start_sample,
                length_samples: length_samples.max(1),
                lyric: String::new(),
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                vibrato_rate_millihz: 5_000,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            });
        }
    }
}

fn decode_midi_messages(data_hex: &str, running_status: &mut Option<u8>) -> Vec<MidiMessage> {
    let bytes = data_hex
        .as_bytes()
        .chunks_exact(2)
        .filter_map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte >= 0xf8 {
            index += 1;
            continue;
        }
        if byte >= 0x80 {
            if byte >= 0xf0 {
                *running_status = None;
                // Skip SysEx and system-common data until the next status.
                index += 1;
                while index < bytes.len() && bytes[index] < 0x80 {
                    index += 1;
                }
                continue;
            }
            *running_status = Some(byte);
            index += 1;
            continue;
        }
        let Some(status) = *running_status else {
            index += 1;
            continue;
        };
        let kind = status & 0xf0;
        let data_len = if matches!(kind, 0xc0 | 0xd0) { 1 } else { 2 };
        if index + data_len > bytes.len() {
            break;
        }
        let data1 = bytes[index];
        let data2 = if data_len == 2 { bytes[index + 1] } else { 0 };
        if data1 < 128 && data2 < 128 {
            let channel = status & 0x0f;
            match kind {
                0xb0 => events.push(MidiMessage::ControlChange {
                    channel,
                    controller: data1,
                    value: data2,
                }),
                0x80 => events.push(MidiMessage::NoteOff {
                    channel,
                    pitch: data1,
                }),
                0x90 if data2 > 0 => events.push(MidiMessage::NoteOn {
                    channel,
                    pitch: data1,
                    velocity: data2,
                }),
                0x90 => events.push(MidiMessage::NoteOff {
                    channel,
                    pitch: data1,
                }),
                _ => {}
            }
        }
        index += data_len;
    }
    events
}
