//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn project_bar_view(&self) -> View {
        let status = self.status.get();
        // Project bar: open/save/new. The file dialog blocks the UI thread
        // while it is open, same as the project actions.
        let open_path = self.project_path.clone();
        let open_pending = self.pending_open.clone();
        let open_button = ButtonView::new("Open")
            .action_id("project.open")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("Hirari project", &["hirari"]);
                let dialog = match open_path.borrow().as_deref() {
                    Some(path) => dialog.set_directory(
                        std::path::Path::new(path)
                            .parent()
                            .map(|parent| parent.to_path_buf())
                            .unwrap_or_default(),
                    ),
                    None => dialog,
                };
                let pending = open_pending.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let save_core = self.core.clone();
        let save_path = self.project_path.clone();
        let save_pending = self.pending_save.clone();
        let save_status = self.status.clone();
        let save_button = ButtonView::new("Save")
            .action_id("project.save")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &save_core else { return };
                match save_path.borrow().clone() {
                    Some(path) => {
                        if core.save_project(&path) {
                            *save_path.borrow_mut() = Some(path.clone());
                            save_status.set(format!(
                                "SAVED {}",
                                std::path::Path::new(&path)
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or(path)
                            ));
                        } else {
                            save_status.set("SAVE FAILED".to_owned());
                        }
                    }
                    None => {
                        let dialog = ame_ui::platform::NativeFileDialog::new()
                            .set_file_name("Untitled.hirari")
                            .add_filter("Hirari project", &["hirari"]);
                        let pending = save_pending.clone();
                        dialog.save_file_async(move |path| {
                            if let Some(path) = path {
                                if let Ok(mut slot) = pending.lock() {
                                    *slot = Some(path);
                                }
                            }
                        });
                    }
                }
            });
        let new_core = self.core.clone();
        let new_path = self.project_path.clone();
        let new_selected = self.selected_clip.clone();
        let new_status = self.status.clone();
        let new_show_genesis = self.show_genesis.clone();
        let new_button = ButtonView::new("New")
            .action_id("project.new")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &new_core {
                    core.new_project();
                    *new_path.borrow_mut() = None;
                    new_selected.set(None);
                    new_show_genesis.set(true);
                    new_status.set("NEW PROJECT".to_owned());
                }
            });
        let diagnostics_open_for_button = self.show_diagnostics.clone();
        let spotlight_state = self.show_spotlight.clone();
        let spotlight_button = ButtonView::new("CMD+K")
            .action_id("spotlight.toggle")
            .secondary()
            .on_press(move |_| {
                spotlight_state.set(!spotlight_state.get());
            });
        let focus_state = self.focus_mode.clone();
        let focus_button = ButtonView::new(if self.focus_mode.get() {
            "FOCUS ON"
        } else {
            "FOCUS"
        })
        .action_id("focus.toggle")
        .secondary()
        .on_press(move |_| {
            let next = !focus_state.get();
            focus_state.set(next);
            let _ = crate::ui_shared::preferences::store_focus_mode(next);
        });
        let diagnostics_button = ButtonView::new(if self.show_diagnostics.get() {
            "SYSTEMS CLOSE"
        } else {
            "SYSTEM"
        })
        .action_id("diagnostics.toggle")
        .secondary()
        .on_press(move |_| {
            diagnostics_open_for_button.set(!diagnostics_open_for_button.get());
        });
        let sends_open_for_button = self.show_sends.clone();
        let sends_button = ButtonView::new(if self.show_sends.get() {
            "SENDS CLOSE"
        } else {
            "SENDS"
        })
        .action_id("sends.toggle")
        .secondary()
        .on_press(move |_| {
            sends_open_for_button.set(!sends_open_for_button.get());
        });
        let midi_open_for_button = self.show_midi.clone();
        let midi_button = ButtonView::new(if self.show_midi.get() {
            "MIDI CLOSE"
        } else {
            "MIDI LEARN"
        })
        .action_id("midi.toggle")
        .secondary()
        .on_press(move |_| {
            midi_open_for_button.set(!midi_open_for_button.get());
        });
        let chords_open_for_button = self.show_chords.clone();
        let chords_button = ButtonView::new(if self.show_chords.get() {
            "CHORDS CLOSE"
        } else {
            "CHORDS"
        })
        .action_id("chords.toggle")
        .secondary()
        .on_press(move |_| {
            chords_open_for_button.set(!chords_open_for_button.get());
        });
        let spectrum_open_for_button = self.show_spectrum.clone();
        let spectrum_button = ButtonView::new(if self.show_spectrum.get() {
            "SPECTRUM CLOSE"
        } else {
            "SPECTRUM"
        })
        .action_id("spectrum.toggle")
        .secondary()
        .on_press(move |_| {
            spectrum_open_for_button.set(!spectrum_open_for_button.get());
        });
        let control_room_open_for_button = self.show_control_room.clone();
        let control_room_button = ButtonView::new(if self.show_control_room.get() {
            "CTRL CLOSE"
        } else {
            "CTRL ROOM"
        })
        .action_id("control_room.toggle")
        .secondary()
        .on_press(move |_| {
            control_room_open_for_button.set(!control_room_open_for_button.get());
        });
        let notation_open_for_button = self.show_notation.clone();
        let notation_button = ButtonView::new(if self.show_notation.get() {
            "NOTATION CLOSE"
        } else {
            "NOTATION"
        })
        .action_id("notation.toggle")
        .secondary()
        .on_press(move |_| {
            notation_open_for_button.set(!notation_open_for_button.get());
        });
        let step_seq_open_for_button = self.show_step_seq.clone();
        let step_seq_button = ButtonView::new(if self.show_step_seq.get() {
            "SEQ CLOSE"
        } else {
            "STEP SEQ"
        })
        .action_id("step_seq.toggle")
        .secondary()
        .on_press(move |_| {
            step_seq_open_for_button.set(!step_seq_open_for_button.get());
        });
        let moufu_open_for_button = self.show_moufu.clone();
        let moufu_button = ButtonView::new(if self.show_moufu.get() {
            "MOUFU CLOSE"
        } else {
            "MOUFU"
        })
        .action_id("moufu.toggle")
        .secondary()
        .on_press(move |_| {
            moufu_open_for_button.set(!moufu_open_for_button.get());
        });
        let project_bar = View::Card(vec![View::HStack(vec![
            View::Text("PROJECT".to_owned()),
            open_button.into(),
            save_button.into(),
            new_button.into(),
            diagnostics_button.into(),
            moufu_button.into(),
            sends_button.into(),
            midi_button.into(),
            step_seq_button.into(),
            notation_button.into(),
            control_room_button.into(),
            spectrum_button.into(),
            chords_button.into(),
            focus_button.into(),
            spotlight_button.into(),
            View::Caption(status.clone()),
        ])]);

        project_bar
    }
}
