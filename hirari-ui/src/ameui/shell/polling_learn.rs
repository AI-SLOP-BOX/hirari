//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Refresh the persisted track-stack view once per tune; the engine
    /// owns the truth, so any local toggles are re-read from JSON.
    pub(in crate::ameui) fn poll_track_stacks(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let persisted = core.track_stacks_json();
        let stacks = parse_track_stacks(&persisted);
        let selected = self.stack_selected_id.get();
        match stacks.iter().find(|stack| stack.id == selected) {
            Some(stack) => {
                self.stack_gain.set(stack.master_gain.clamp(0.0, 2.0));
                self.stack_collapsed.set(stack.collapsed);
                self.stack_name.set(stack.name.clone());
            }
            None => {
                if selected != 0 {
                    self.stack_selected_id.set(0);
                    self.stack_gain.set(1.0);
                    self.stack_collapsed.set(false);
                    self.stack_name.set(String::new());
                }
            }
        }
    }

    /// MIDI LEARN poll. Runs while the MIDI LEARN panel is open: starts
    /// CoreMIDI on entry, drains input packets, pushes Note On/Off events
    /// into the 30-second retrospective buffer, binds an armed macro to the
    /// next CC message seen, and mirrors the last event into the status.
    pub(in crate::ameui) fn poll_midi_learn(&mut self, core: &HirariCore) {
        if !self.midi_input_started {
            if !core.start_midi_input() {
                self.midi_status.set("MIDI INPUT UNAVAILABLE".to_owned());
                return;
            }
            self.midi_input_started = true;
            self.midi_status
                .set("MIDI INPUT READY · ARM A MACRO THEN MOVE A CC".to_owned());
        }
        // A retrospective insert was requested by the card; perform it here so
        // this borrows `self` freely.
        if let Some(track_id) = self.midi_insert_pending.borrow_mut().take() {
            let is_midi = self.tracks.get().iter().any(|track| {
                track.id == track_id
                    && matches!(track.kind.as_str(), "Midi" | "MIDI" | "Instrument")
            });
            match midi_learn::insert_retrospective_notes(
                track_id,
                &self.midi_retrospective,
                core,
                is_midi,
            ) {
                Ok(count) => {
                    self.midi_retrospective.borrow_mut().clear();
                    self.midi_status
                        .set(format!("INSERTED {count} NOTES · UNDO AVAILABLE"));
                    self.refresh_arrange_state(core);
                }
                Err(message) => {
                    self.midi_status.set(message);
                    if is_midi {
                        self.midi_retrospective.borrow_mut().clear();
                    }
                }
            }
        }
        for packet in
            serde_json::from_str::<Vec<midi_learn::InputPacket>>(&core.poll_midi_input_json())
                .unwrap_or_default()
        {
            let device_id = format!("coremidi:{}:input", packet.source_unique_id);
            let running_status = self
                .midi_running_status
                .entry(packet.source_unique_id)
                .or_default();
            for message in midi_learn::decode_midi_messages(&packet.data_hex, running_status) {
                let event_host_time = if packet.host_time == 0 {
                    core.midi_host_time_now()
                } else {
                    packet.host_time
                };
                if matches!(
                    message,
                    midi_learn::MidiMessage::NoteOn { .. }
                        | midi_learn::MidiMessage::NoteOff { .. }
                ) {
                    let sample_rate = core.get_sample_rate().max(1.0);
                    let mut buffer = self.midi_retrospective.borrow_mut();
                    buffer.push_back(midi_learn::BufferedMidiEvent {
                        source_id: packet.source_unique_id,
                        host_time: event_host_time,
                        message,
                    });
                    while buffer.len() > midi_learn::MAX_RETROSPECTIVE_MIDI_EVENTS {
                        buffer.pop_front();
                    }
                    while buffer.front().is_some_and(|event| {
                        core.midi_host_time_delta_samples(
                            event.host_time,
                            event_host_time,
                            sample_rate,
                        ) > sample_rate as u64 * midi_learn::RETROSPECTIVE_MIDI_SECONDS
                    }) {
                        buffer.pop_front();
                    }
                }
                let midi_learn::MidiMessage::ControlChange {
                    channel,
                    controller,
                    value,
                } = message
                else {
                    continue;
                };
                let armed = self.midi_armed_macro.get();
                let learned = if armed > 0 {
                    let macro_index = armed - 1;
                    let mapping = MidiLearnMappingContract {
                        mapping_id: format!(
                            "midi:{}:{}:{}:macro:{}",
                            packet.source_unique_id, channel, controller, macro_index
                        ),
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
                            self.midi_armed_macro.set(0);
                            Some(macro_index)
                        }
                        Err(error) => {
                            self.midi_status.set(format!("MIDI LEARN FAILED: {error}"));
                            None
                        }
                    }
                } else {
                    None
                };
                if learned.is_some() {
                    self.refresh_arrange_state(core);
                }
                core.handle_midi_cc_from_device(&device_id, channel, controller, value);
                self.midi_status.set(match learned {
                    Some(macro_index) => format!(
                        "LEARNED CH {} · CC {} → MACRO {}",
                        channel + 1,
                        controller,
                        macro_index + 1
                    ),
                    None => format!("MIDI CH {} · CC {} = {}", channel + 1, controller, value),
                });
            }
        }
    }

    pub(in crate::ameui) fn stop_midi_learn(&mut self, core: &HirariCore) {
        if self.midi_input_started {
            core.stop_midi_input();
            self.midi_input_started = false;
        }
        self.midi_armed_macro.set(0);
        self.midi_retrospective.borrow_mut().clear();
    }
}
