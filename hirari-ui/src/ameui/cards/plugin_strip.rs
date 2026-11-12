//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn plugin_strip_view(&self) -> Option<View> {
        let tracks = self.tracks.get();
        // Plugin strip for the selected track: bypass toggles, removal,
        // and an add stepper over the built-in chain (fx list).
        let plugin_strip = tracks
            .iter()
            .find(|track| track.id == self.piano_track.get())
            .filter(|track| !track.plugin_types.is_empty())
            .map(|track| {
                let track_id = track.id;
                let mut rows: Vec<View> = vec![View::Text("PLUGINS".to_owned())];
                for (index, plugin_type) in track.plugin_types.iter().enumerate() {
                    let bypassed = track.plugin_bypass.get(index).copied().unwrap_or(false);
                    let bypass_core = self.core.clone();
                    let bypass_tracks = self.tracks.clone();
                    let bypass_locked = self.arrange_locked.get();
                    let bypass_button = ButtonView::new(if bypassed { "Enable" } else { "Bypass" })
                        .action_id(format!("plugin.bypass.{index}"))
                        .secondary()
                        .on_press(move |_| {
                            if bypass_locked {
                                return;
                            }
                            if let Some(core) = &bypass_core {
                                if core.set_plugin_bypass(track_id, index as u32, !bypassed) {
                                    bypass_tracks.set(parse_arrange_tracks(
                                        &core.get_project_layout_json(),
                                        |samples| core.samples_to_beats(samples),
                                    ));
                                }
                            }
                        });
                    let remove_core = self.core.clone();
                    let remove_tracks = self.tracks.clone();
                    let remove_button = ButtonView::new("Remove")
                        .action_id(format!("plugin.remove.{index}"))
                        .secondary()
                        .on_press(move |_| {
                            if bypass_locked {
                                return;
                            }
                            if let Some(core) = &remove_core {
                                if core.remove_plugin(track_id, index as u32) {
                                    remove_tracks.set(parse_arrange_tracks(
                                        &core.get_project_layout_json(),
                                        |samples| core.samples_to_beats(samples),
                                    ));
                                }
                            }
                        });
                    let select_state = self.selected_plugin.clone();
                    let select_active = self.selected_plugin.get() == index as u32;
                    let select_button =
                        ButtonView::new(if select_active { "Selected" } else { "Params" })
                            .action_id(format!("plugin.select.{index}"))
                            .secondary()
                            .on_press(move |_| {
                                select_state.set(index as u32);
                            });
                    rows.push(View::HStack(vec![
                        View::Text(plugin_name(*plugin_type, index)),
                        select_button.into(),
                        bypass_button.into(),
                        remove_button.into(),
                    ]));
                }
                let add_type_state = self.add_plugin_type.clone();
                let add_button =
                    ButtonView::new(format!("ADD {}", plugin_name(add_type_state.get(), 0)))
                        .action_id("plugin.add")
                        .secondary()
                        .on_press({
                            let add_core = self.core.clone();
                            let add_tracks = self.tracks.clone();
                            let add_locked = self.arrange_locked.get();
                            let add_type = add_type_state.get();
                            move |_| {
                                if add_locked {
                                    return;
                                }
                                if let Some(core) = &add_core {
                                    if core.add_plugin(track_id, add_type) {
                                        add_tracks.set(parse_arrange_tracks(
                                            &core.get_project_layout_json(),
                                            |samples| core.samples_to_beats(samples),
                                        ));
                                    }
                                }
                            }
                        });
                let cycle_state = self.add_plugin_type.clone();
                let cycle_button = ButtonView::new("TYPE")
                    .action_id("plugin.add_type_cycle")
                    .secondary()
                    .on_press(move |_| {
                        cycle_state.set((cycle_state.get() + 1) % 11);
                    });
                rows.push(View::HStack(vec![cycle_button.into(), add_button.into()]));
                View::Card(vec![View::VStack(rows)])
            });

        plugin_strip
    }
}
