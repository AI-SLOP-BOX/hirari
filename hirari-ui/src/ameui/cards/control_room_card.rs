//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn control_room_card_view(&self) -> View {
        // CONTROL ROOM card (control room): dim + talkback on the monitor bus,
        // engine-driven so the values round-trip through the poll each refresh.
        // CONTROL ROOM card (control room / monitor settings): DIM with
        // dB depth, TALKBACK with gain, plus speaker registration, selection
        // and gain — engine-backed so selection round-trips to the monitor.
        let control_room_card = {
            let dim_core = self.core.clone();
            let dim_state = self.control_room_dim.clone();
            let dim_checkbox = CheckboxView::new("DIM", self.control_room_dim.get())
                .action_id("control_room.dim")
                .on_toggle(move |next| {
                    if let Some(core) = &dim_core {
                        core.set_control_room_dim(next);
                    }
                    dim_state.set(next);
                });
            let dim_db_state = self.control_room_dim_db.clone();
            let dim_db_core = self.core.clone();
            let dim_db_slider = SliderView::new(self.control_room_dim_db.get(), -60.0, 0.0)
                .action_id("control_room.dim_db")
                .on_change(move |value| {
                    let clamped = value.clamp(-60.0, 0.0);
                    dim_db_state.set(clamped);
                    if let Some(core) = &dim_db_core {
                        let _ = core.set_control_room_dim_db(clamped);
                    }
                });
            let talkback_state = self.talkback_enabled.clone();
            let talkback_gain_state = self.talkback_gain.clone();
            let talkback_core = self.core.clone();
            let talkback_checkbox = CheckboxView::new("TALKBACK", self.talkback_enabled.get())
                .action_id("control_room.talkback")
                .on_toggle(move |next| {
                    if let Some(core) = &talkback_core {
                        core.set_control_room_talkback(next, talkback_gain_state.get());
                    }
                    talkback_state.set(next);
                });
            let gain_state = self.talkback_gain.clone();
            let gain_core = self.core.clone();
            let gain_talkback_state = self.talkback_enabled.clone();
            let gain_slider = SliderView::new(self.talkback_gain.get(), 0.0, 4.0)
                .action_id("control_room.talkback_gain")
                .on_change(move |value| {
                    let clamped = value.clamp(0.0, 4.0);
                    gain_state.set(clamped);
                    if let Some(core) = &gain_core {
                        core.set_control_room_talkback(gain_talkback_state.get(), clamped);
                    }
                });
            let speakers = self.control_room_speakers.get();
            let selected_index = self
                .control_room_speaker_index
                .get()
                .min(speakers.len().saturating_sub(1) as u32);
            let add_core = self.core.clone();
            let add_rows = self.control_room_speakers.clone();
            let add_index = self.control_room_speaker_index.clone();
            let add_button = ButtonView::new("ADD SPK")
                .action_id("control_room.add_speaker")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &add_core else { return };
                    let current = add_rows.get();
                    let next_index = current.len() as u32 + 1;
                    let name = format!("SPK {next_index}");
                    if core.add_control_room_speaker(&name, 1.0) {
                        add_rows.set({
                            let mut next = current;
                            next.push(name);
                            next
                        });
                        add_index.set(next_index.saturating_sub(1));
                    }
                });
            let speaker_gain_core = self.core.clone();
            let speaker_gain_index = self.control_room_speaker_index.clone();
            let speaker_gain_rows = self.control_room_speakers.clone();
            let speaker_gain = SliderView::new(1.0, 0.0, 4.0)
                .action_id("control_room.speaker_gain")
                .on_change(move |value| {
                    let index = speaker_gain_index
                        .get()
                        .min(speaker_gain_rows.get().len().saturating_sub(1) as u32);
                    if let Some(core) = &speaker_gain_core {
                        let _ = core.set_control_room_speaker_gain(index, value.clamp(0.0, 4.0));
                    }
                });
            let remove_core = self.core.clone();
            let remove_rows = self.control_room_speakers.clone();
            let remove_index = self.control_room_speaker_index.clone();
            let remove_button = ButtonView::new("DEL SPK")
                .action_id("control_room.remove_speaker")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &remove_core else { return };
                    let mut current = remove_rows.get();
                    let index = remove_index.get() as usize;
                    if index >= current.len() {
                        return;
                    }
                    let _ = core.remove_control_room_speaker(index as u32);
                    current.remove(index);
                    if current.is_empty() {
                        remove_index.set(0);
                    } else if (index as u32) >= current.len() as u32 {
                        remove_index.set(current.len() as u32 - 1);
                    }
                    remove_rows.set(current);
                });
            let mut speaker_views = vec![View::Text("CTRL ROOM".to_owned())];
            speaker_views.push(View::HStack(vec![
                dim_checkbox.into(),
                View::Caption("dB".to_owned()),
                dim_db_slider.into(),
                View::Caption(format!("{:+.0}", self.control_room_dim_db.get())),
                talkback_checkbox.into(),
                gain_slider.into(),
                View::Caption(format!("GAIN {:.2}", self.talkback_gain.get())),
            ]));
            speaker_views.push(View::HStack(vec![
                View::Caption("MONITORS".to_owned()),
                add_button.into(),
                speaker_gain.into(),
                remove_button.into(),
            ]));
            if speakers.is_empty() {
                speaker_views.push(View::Caption(
                    "No monitor speakers registered — ADD SPK creates one.".to_owned(),
                ));
            } else {
                for (index, name) in speakers.iter().enumerate() {
                    let select_core = self.core.clone();
                    let select_index = self.control_room_speaker_index.clone();
                    let make_selected = index == selected_index as usize;
                    let select_button = ButtonView::new(format!(
                        "{}{}",
                        if make_selected { "● " } else { "" },
                        name
                    ))
                    .action_id(format!("control_room.select.{index}"))
                    .secondary()
                    .on_press(move |_| {
                        if let Some(core) = &select_core {
                            let _ = core.select_control_room_speaker(index as u32);
                            let _ = core.set_control_room_speaker_enabled(index as u32, true);
                        }
                        select_index.set(index as u32);
                    });
                    speaker_views.push(View::HStack(vec![select_button.into()]));
                }
            }
            View::Card(vec![View::VStack(speaker_views)])
        };

        control_room_card
    }
}
