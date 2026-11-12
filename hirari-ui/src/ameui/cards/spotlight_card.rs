//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn spotlight_card_view(&self) -> View {
        let spotlight_card = {
            let query_state = self.spotlight_query.clone();
            let query_field = TextFieldView::new(
                self.spotlight_query.get(),
                "Filter commands (play, save, render...)",
            )
            .change_action_id("spotlight.query")
            .submit_action_id("spotlight.query")
            .on_change(move |value| query_state.set(value))
            .on_submit(|| {});
            let spotlight_core = self.core.clone();
            let play_toggle = self.playing.clone();
            let loop_toggle = self.loop_enabled.clone();
            let metro_toggle = self.metronome_enabled.clone();
            let filter = self.spotlight_query.get().to_lowercase();
            let commands: [(&str, SpotlightCommand); 13] = [
                ("TOGGLE PLAY", SpotlightCommand::TogglePlay),
                ("TOGGLE LOOP", SpotlightCommand::ToggleLoop),
                ("TOGGLE METRONOME", SpotlightCommand::ToggleMetronome),
                ("SAVE PROJECT", SpotlightCommand::Save),
                ("NEW PROJECT", SpotlightCommand::New),
                ("CYCLE SNAP", SpotlightCommand::SnapCycle),
                ("ZOOM IN", SpotlightCommand::ZoomIn),
                ("ZOOM OUT", SpotlightCommand::ZoomOut),
                ("FIT ARRANGEMENT", SpotlightCommand::Fit),
                ("RENDER MASTER", SpotlightCommand::RenderMaster),
                ("EXPORT MIDI", SpotlightCommand::ExportMidi),
                ("TOGGLE AUTO-LOCK", SpotlightCommand::LockToggle),
                ("UNDO", SpotlightCommand::Undo),
            ];
            let matches: Vec<(String, SpotlightCommand)> = if filter.is_empty() {
                commands
                    .into_iter()
                    .map(|(label, command)| (label.to_owned(), command))
                    .collect()
            } else {
                commands
                    .into_iter()
                    .filter(|(label, _)| label.to_lowercase().contains(&filter))
                    .map(|(label, command)| (label.to_owned(), command))
                    .collect()
            };
            let mut buttons: Vec<View> = Vec::new();
            for (label, command) in matches {
                let core = spotlight_core.clone();
                let playing = play_toggle.clone();
                let loop_on = loop_toggle.clone();
                let metro = metro_toggle.clone();
                let snap_state = self.arrange_snap_division.clone();
                let zoom_state = self.arrange_zoom.clone();
                let lock_state = self.arrange_locked.clone();
                let button = ButtonView::new(label.to_owned())
                    .action_id(format!("spotlight.{}", label.to_lowercase()))
                    .secondary()
                    .on_press(move |_| {
                        let Some(core) = &core else { return };
                        match command {
                            SpotlightCommand::TogglePlay => {
                                let next = !playing.get();
                                core.set_playing(next);
                                playing.set(next);
                            }
                            SpotlightCommand::ToggleLoop => {
                                let next = !loop_on.get();
                                core.set_loop(next);
                                loop_on.set(next);
                            }
                            SpotlightCommand::ToggleMetronome => {
                                let next = !metro.get();
                                core.set_metronome_enabled(next);
                                metro.set(next);
                            }
                            SpotlightCommand::Save => {
                                let _ = core.save_project(&core_default_project_path());
                            }
                            SpotlightCommand::New => {
                                core.new_project();
                            }
                            SpotlightCommand::SnapCycle => {
                                const DIVISIONS: [u32; 5] = [1, 2, 4, 8, 16];
                                let current = snap_state.get() as u32;
                                let next = DIVISIONS
                                    .iter()
                                    .find(|division| **division > current)
                                    .copied()
                                    .unwrap_or(DIVISIONS[0]);
                                snap_state.set(next as f32);
                            }
                            SpotlightCommand::ZoomIn => {
                                zoom_state.set((zoom_state.get() * 1.25).min(ARRANGE_ZOOM_MAX));
                            }
                            SpotlightCommand::ZoomOut => {
                                zoom_state.set((zoom_state.get() / 1.25).max(ARRANGE_ZOOM_MIN));
                            }
                            SpotlightCommand::Fit => {
                                zoom_state.set(1.0);
                            }
                            SpotlightCommand::RenderMaster => {
                                let path = default_render_output_path();
                                core.set_offline_render_tail_seconds(0.0);
                                let _ =
                                    core.start_render_async_to_format(&path.to_string_lossy(), 0);
                            }
                            SpotlightCommand::ExportMidi => {
                                let _ = core;
                            }
                            SpotlightCommand::LockToggle => {
                                lock_state.set(!lock_state.get());
                            }
                            SpotlightCommand::Undo => {
                                let _ = core.undo();
                            }
                        }
                    });
                buttons.push(button.into());
            }
            View::Card(vec![
                View::Text("CMD+K · QUICK".to_owned()),
                query_field.into(),
                View::HStack(buttons),
            ])
        };

        spotlight_card
    }
}
