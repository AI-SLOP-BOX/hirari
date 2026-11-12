//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn midi_learn_card_view(&self) -> View {
        // MIDI LEARN card (MIDI Learn page): ARM MACRO 1-8 to bind the next
        // CoreMIDI CC to the engine macro, and INSERT RECENT MIDI to drop the
        // last 30 s of buffered Note On/Off onto the selected MIDI/Instrument
        // track. The poll feeds the retrospective buffer; INSERT runs in the
        // poll so undo/refresh happen in one place.
        let armed = self.midi_armed_macro.get();
        let mut midi_arm_buttons: Vec<View> = Vec::new();
        for macro_index in 1..=8u32 {
            let armed_state = self.midi_armed_macro.clone();
            let label = if armed == macro_index {
                format!("M{macro_index} ARMED")
            } else {
                format!("MACRO {macro_index}")
            };
            let arm_button = ButtonView::new(label)
                .action_id(format!("midi.arm.{macro_index}"))
                .secondary()
                .on_press(move |_| {
                    let next = if armed_state.get() == macro_index {
                        0
                    } else {
                        macro_index
                    };
                    armed_state.set(next);
                });
            midi_arm_buttons.push(arm_button.into());
        }
        let piano_track_id = self.piano_track.get();
        let piano_track_kind = self
            .tracks
            .get()
            .into_iter()
            .find(|track| track.id == piano_track_id)
            .map(|track| track.kind)
            .unwrap_or_default();
        let is_midi_target = matches!(piano_track_kind.as_str(), "Midi" | "MIDI" | "Instrument");
        let insert_pending = self.midi_insert_pending.clone();
        let insert_status = self.midi_status.clone();
        let insert_button = ButtonView::new("INSERT RECENT MIDI")
            .action_id("midi.insert_recent")
            .secondary()
            .on_press(move |_| {
                if is_midi_target {
                    *insert_pending.borrow_mut() = Some(piano_track_id);
                } else {
                    insert_status
                        .set("RECENT MIDI NEEDS A MIDI OR INSTRUMENT TRACK SELECTED".to_owned());
                }
            });
        let midi_learn_card = View::Card(vec![
            View::Text("MIDI LEARN".to_owned()),
            View::HStack(midi_arm_buttons),
            View::HStack(vec![insert_button.into()]),
            View::Caption(self.midi_status.get()),
            View::Caption(
                "Arm a macro then move any CC control. INSERT RECENT MIDI drops the last 30 s of held Note On/Off onto the selected track (also inside LOOP ranges)."
                    .to_owned(),
            ),
        ]);

        midi_learn_card
    }
}
