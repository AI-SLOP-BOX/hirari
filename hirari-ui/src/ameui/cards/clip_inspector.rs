//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn clip_inspector_view(&self) -> Option<View> {
        let tracks = self.tracks.get();
        // Clip inspector for the selected audio clip: fade in/out and
        // gain sliders, committing through the engine undo transactions
        // like the clip editor. MIDI clips have no region to edit.
        let inspector = self
            .selected_clip
            .get()
            .and_then(|(selected_track, selected_id)| {
                let clip = tracks
                    .iter()
                    .find(|track| track.id == selected_track)
                    .and_then(|track| track.clips.iter().find(|clip| clip.id == selected_id))?;
                if clip.midi || clip.id <= 0 {
                    return None;
                }
                let clip = clip.clone();
                let clip_fade_in = clip.fade_in;
                let clip_fade_out = clip.fade_out;
                let fade_core = self.core.clone();
                let fade_tracks = self.tracks.clone();
                let fade_locked = self.arrange_locked.get();
                let fade_in = SliderView::new(clip.fade_in, 0.0, 1.0)
                    .action_id(format!("clip.fade_in.{}", clip.id))
                    .disabled(fade_locked)
                    .on_release(move |value| {
                        if fade_locked {
                            return;
                        }
                        let Some(core) = &fade_core else { return };
                        core.begin_undo_transaction("Set Selected Clip Fades");
                        let accepted = core.set_region_fades(
                            selected_track,
                            clip.id as u32,
                            value,
                            clip_fade_out,
                        );
                        if accepted {
                            let _ = core.end_undo_transaction();
                            fade_tracks.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        } else {
                            let _ = core.abort_undo_transaction();
                        }
                    });
                let fade_out_core = self.core.clone();
                let fade_out_tracks = self.tracks.clone();
                let fade_out = SliderView::new(clip.fade_out, 0.0, 1.0)
                    .action_id(format!("clip.fade_out.{}", clip.id))
                    .disabled(fade_locked)
                    .on_release(move |value| {
                        if fade_locked {
                            return;
                        }
                        let Some(core) = &fade_out_core else { return };
                        core.begin_undo_transaction("Set Selected Clip Fades");
                        let accepted = core.set_region_fades(
                            selected_track,
                            clip.id as u32,
                            clip_fade_in,
                            value,
                        );
                        if accepted {
                            let _ = core.end_undo_transaction();
                            fade_out_tracks.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        } else {
                            let _ = core.abort_undo_transaction();
                        }
                    });
                let gain_core = self.core.clone();
                let gain_tracks = self.tracks.clone();
                let gain = SliderView::new(clip.gain, 0.0, 2.0)
                    .action_id(format!("clip.gain.{}", clip.id))
                    .disabled(fade_locked)
                    .on_release(move |value| {
                        if fade_locked {
                            return;
                        }
                        let Some(core) = &gain_core else { return };
                        core.begin_undo_transaction("Set Selected Clip Gain");
                        let accepted = core.set_region_gain(
                            selected_track,
                            clip.id as u32,
                            value.clamp(0.0, 2.0),
                        );
                        if accepted {
                            let _ = core.end_undo_transaction();
                            gain_tracks.set(parse_arrange_tracks(
                                &core.get_project_layout_json(),
                                |samples| core.samples_to_beats(samples),
                            ));
                        } else {
                            let _ = core.abort_undo_transaction();
                        }
                    });
                Some(View::Card(vec![View::HStack(vec![
                    View::Text(format!("CLIP {} · {}", clip.id, clip.name)),
                    View::Caption("Fade In".to_owned()),
                    fade_in.into(),
                    View::Caption("Fade Out".to_owned()),
                    fade_out.into(),
                    View::Caption("Gain".to_owned()),
                    gain.into(),
                ])]))
            });

        inspector
    }
}
