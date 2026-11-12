//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn flex_card_view(&self) -> View {
        // Flex & Pitch card (P8): warp/pitch preservation + sync group + sync-group quantize.
        let flex_card = if let Some((track_id, clip_id)) = self.selected_clip.get() {
            let clips = self
                .tracks
                .get()
                .iter()
                .find(|track| track.id == track_id)
                .map(|track| track.clips.clone())
                .unwrap_or_default();
            let selected_clip = clips.iter().find(|clip| clip.id == clip_id).cloned();
            let clip_is_audio = selected_clip
                .as_ref()
                .map(|clip| !clip.midi)
                .unwrap_or(false);
            if clip_is_audio {
                let preserve_current = selected_clip
                    .as_ref()
                    .map(|clip| clip.pitch_preserve_warp)
                    .unwrap_or(false);
                let preserve_core = self.core.clone();
                let preserve_refresh = self.tracks.clone();
                let preserve_button = CheckboxView::new("Preserve pitch (WSOLA)", preserve_current)
                    .action_id("flex.preserve_pitch")
                    .on_toggle(move |next| {
                        if let Some(core) = &preserve_core {
                            let _ =
                                core.set_region_pitch_preserve_warp(track_id, clip_id as u32, next);
                            preserve_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                let warp_value = selected_clip
                    .as_ref()
                    .map(|clip| clip.warp_ratio.clamp(0.5, 2.0))
                    .unwrap_or(1.0);
                let warp_core = self.core.clone();
                let warp_refresh = self.tracks.clone();
                let warp_slider = SliderView::new(warp_value as f32, 0.5, 2.0)
                    .action_id("flex.warp_ratio")
                    .on_change(move |value| {
                        if let Some(core) = &warp_core {
                            let clamped = value.clamp(0.5, 2.0) as f64;
                            let _ = core.set_region_warp_ratio(track_id, clip_id as u32, clamped);
                            warp_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                let pitch_value = selected_clip
                    .as_ref()
                    .map(|clip| clip.pitch_semitones.clamp(-24.0, 24.0))
                    .unwrap_or(0.0);
                let pitch_core = self.core.clone();
                let pitch_refresh = self.tracks.clone();
                let pitch_slider = SliderView::new(pitch_value, -24.0, 24.0)
                    .action_id("flex.pitch_semitones")
                    .on_change(move |value| {
                        if let Some(core) = &pitch_core {
                            let _ =
                                core.set_region_pitch_semitones(track_id, clip_id as u32, value);
                            pitch_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                let sync_value = selected_clip
                    .as_ref()
                    .map(|clip| clip.sync_group.to_string())
                    .unwrap_or_else(|| "0".to_owned());
                let sync_text = self.flex_sync_group_text.clone();
                let sync_text_submit = self.flex_sync_group_text.clone();
                let sync_apply_core = self.core.clone();
                let sync_refresh = self.tracks.clone();
                let sync_field = TextFieldView::new(sync_value, "sync group id (0=unlinked)")
                    .change_action_id("flex.sync.text")
                    .submit_action_id("flex.sync.apply")
                    .on_change(move |value| {
                        sync_text.set(value);
                    })
                    .on_submit(move || {
                        let parsed = sync_text_submit.get().trim().parse::<u32>().ok();
                        let Some(group) = parsed else { return };
                        let Some(core) = &sync_apply_core else { return };
                        let _ = core.set_region_sync_group(track_id, clip_id as u32, group);
                        sync_refresh.set(parse_arrange_tracks(
                            &core.get_project_layout_json(),
                            |samples| core.samples_to_beats(samples),
                        ));
                    });
                const GRID_NAMES: [&str; 13] = [
                    "1/4", "1/8", "1/16", "1/32", "1/64", "1/4T", "1/8T", "1/16T", "1/32T", "1/4D",
                    "1/8D", "1/16D", "1/32D",
                ];
                let grid_state = self.flex_quantize_grid.clone();
                let grid_label = GRID_NAMES
                    .get(self.flex_quantize_grid.get() as usize)
                    .copied()
                    .unwrap_or("1/16");
                let grid_button = ButtonView::new(format!("GRID {grid_label}"))
                    .action_id("flex.grid.cycle")
                    .secondary()
                    .on_press(move |_| {
                        grid_state.set((grid_state.get() + 1) % GRID_NAMES.len() as u32);
                    });
                let strength_state = self.flex_quantize_strength.clone();
                let strength_slider = SliderView::new(strength_state.get(), 0.0, 1.0)
                    .action_id("flex.quant.strength")
                    .on_change(move |value| {
                        strength_state.set(value.clamp(0.0, 1.0));
                    });
                let swing_state = self.flex_quantize_swing.clone();
                let swing_slider = SliderView::new(swing_state.get(), -1.0, 1.0)
                    .action_id("flex.quant.swing")
                    .on_change(move |value| {
                        swing_state.set(value.clamp(-1.0, 1.0));
                    });
                let quant_core = self.core.clone();
                let quant_refresh = self.tracks.clone();
                let quant_strength = self.flex_quantize_strength.clone();
                let quant_grid = self.flex_quantize_grid.clone();
                let quant_swing = self.flex_quantize_swing.clone();
                let quantize_button = ButtonView::new("QUANTIZE SYNC GROUP")
                    .action_id("flex.quantize")
                    .primary()
                    .on_press(move |_| {
                        if let Some(core) = &quant_core {
                            let _ = core.quantize_region_sync_group(
                                track_id,
                                clip_id as u32,
                                quant_strength.get(),
                                quant_grid.get() as u8,
                                quant_swing.get(),
                            );
                            quant_refresh.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        }
                    });
                View::Card(vec![View::VStack(vec![
                    View::Text("FLEX & PITCH".to_owned()),
                    View::HStack(vec![
                        View::Caption("MODE".to_owned()),
                        preserve_button.into(),
                    ]),
                    View::HStack(vec![View::Caption("SPEED".to_owned()), warp_slider.into()]),
                    View::HStack(vec![View::Caption("PITCH".to_owned()), pitch_slider.into()]),
                    View::HStack(vec![
                        View::Caption("SYNC GROUP".to_owned()),
                        sync_field.into(),
                    ]),
                    View::HStack(vec![
                        View::Caption("QUANT STR".to_owned()),
                        strength_slider.into(),
                    ]),
                    View::HStack(vec![View::Caption("GRID".to_owned()), grid_button.into()]),
                    View::HStack(vec![View::Caption("SWING".to_owned()), swing_slider.into()]),
                    View::HStack(vec![quantize_button.into()]),
                ])])
            } else {
                View::Card(vec![View::HStack(vec![View::Caption(
                    "Flex & Pitch edits require an Audio or Vocal region selected.".to_owned(),
                )])])
            }
        } else {
            View::Card(vec![View::HStack(vec![View::Caption(
                "Select an Audio or Vocal clip in Arrange to edit flex-pitch.".to_owned(),
            )])])
        };

        flex_card
    }
}
