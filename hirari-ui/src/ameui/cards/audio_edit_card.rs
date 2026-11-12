//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn audio_edit_card_view(&self) -> View {
        // AUDIO EDIT card (P14 partial): operate on the selected clip.
        let audio_edit_card = if let Some((track_id, clip_id)) = self.selected_clip.get() {
            let clips: Vec<_> = self
                .tracks
                .get()
                .iter()
                .find(|track| track.id == track_id)
                .map(|track| track.clips.clone())
                .unwrap_or_default();
            let selected_clip = clips.iter().find(|clip| clip.id == clip_id).cloned();
            let edit_core = self.core.clone();
            let edit_refresh = self.tracks.clone();
            let reverse_current = selected_clip.as_ref().map(|c| c.reverse).unwrap_or(false);
            let reverse_button = ButtonView::new(if reverse_current {
                "UNREVERSE"
            } else {
                "REVERSE"
            })
            .action_id("audioedit.reverse")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &edit_core else { return };
                let _ = core.set_region_reverse(track_id, clip_id as u32, !reverse_current);
                edit_refresh.set(parse_arrange_tracks(
                    &core.get_project_layout_json(),
                    |samples| core.samples_to_beats(samples),
                ));
            });
            let pitch_label = format!(
                "PITCH: {}",
                selected_clip
                    .as_ref()
                    .map(|clip| format!("{:+.1} st", clip.pitch_semitones))
                    .unwrap_or_else(|| "?".to_owned())
            );
            let pitch_down_core = self.core.clone();
            let pitch_down_current = selected_clip
                .as_ref()
                .map(|c| c.pitch_semitones)
                .unwrap_or(0.0);
            let pitch_down_refresh = self.tracks.clone();
            let pitch_down = ButtonView::new("PITCH-")
                .action_id("audioedit.pitch_down")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &pitch_down_core else { return };
                    let _ = core.set_region_pitch_semitones(
                        track_id,
                        clip_id as u32,
                        pitch_down_current - 1.0,
                    );
                    pitch_down_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let pitch_up_core = self.core.clone();
            let pitch_up_current = selected_clip
                .as_ref()
                .map(|c| c.pitch_semitones)
                .unwrap_or(0.0);
            let pitch_up_refresh = self.tracks.clone();
            let pitch_up = ButtonView::new("PITCH+")
                .action_id("audioedit.pitch_up")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &pitch_up_core else { return };
                    let _ = core.set_region_pitch_semitones(
                        track_id,
                        clip_id as u32,
                        pitch_up_current + 1.0,
                    );
                    pitch_up_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let split_core = self.core.clone();
            let split_refresh = self.tracks.clone();
            let split_playhead = self.playhead_beats.clone();
            let split_button = ButtonView::new("SPLIT")
                .action_id("audioedit.split")
                .primary()
                .on_press(move |_| {
                    let Some(core) = &split_core else { return };
                    let beat = split_playhead.get();
                    let _ = core.split_region(track_id, clip_id as u32, beat);
                    split_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            // Fades: one slider each for fade-in and fade-out, applied against
            // the layout-tracked current value on every change.
            let clip_fade_in = selected_clip
                .as_ref()
                .map(|clip| clip.fade_in.clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let clip_fade_out = selected_clip
                .as_ref()
                .map(|clip| clip.fade_out.clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let fade_in_core = self.core.clone();
            let fade_in_refresh = self.tracks.clone();
            let fade_out_current = clip_fade_out;
            let fade_in_slider = SliderView::new(clip_fade_in, 0.0, 1.0)
                .action_id("audioedit.fade_in")
                .on_change(move |value| {
                    let clamped = value.clamp(0.0, 1.0);
                    if let Some(core) = &fade_in_core {
                        let _ = core.set_region_fades(
                            track_id,
                            clip_id as u32,
                            clamped,
                            fade_out_current,
                        );
                        fade_in_refresh.set(parse_arrange_tracks(
                            &core.get_project_layout_json(),
                            |samples| core.samples_to_beats(samples),
                        ));
                    }
                });
            let fade_out_core = self.core.clone();
            let fade_out_refresh = self.tracks.clone();
            let fade_in_current = clip_fade_in;
            let fade_out_slider = SliderView::new(clip_fade_out, 0.0, 1.0)
                .action_id("audioedit.fade_out")
                .on_change(move |value| {
                    let clamped = value.clamp(0.0, 1.0);
                    if let Some(core) = &fade_out_core {
                        let _ = core.set_region_fades(
                            track_id,
                            clip_id as u32,
                            fade_in_current,
                            clamped,
                        );
                        fade_out_refresh.set(parse_arrange_tracks(
                            &core.get_project_layout_json(),
                            |samples| core.samples_to_beats(samples),
                        ));
                    }
                });
            let loop_core = self.core.clone();
            let loop_refresh = self.tracks.clone();
            let loop_current = selected_clip
                .as_ref()
                .map(|clip| clip.loop_count)
                .unwrap_or(1);
            let loop_button = ButtonView::new(format!("LOOP {loop_current}×"))
                .action_id("audioedit.loop")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &loop_core else { return };
                    let next = match loop_current {
                        0 | 1 => 2,
                        2 => 4,
                        _ => 1,
                    };
                    let _ = core.set_region_loop_count(track_id, clip_id as u32, next);
                    loop_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let duplicate_core = self.core.clone();
            let duplicate_refresh = self.tracks.clone();
            let duplicate_playhead = self.playhead_beats.clone();
            let duplicate_button = ButtonView::new("DUPLICATE")
                .action_id("audioedit.duplicate")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &duplicate_core else { return };
                    let beat = duplicate_playhead.get();
                    let _ = core.duplicate_region(track_id, clip_id as u32, beat);
                    duplicate_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            let delete_core = self.core.clone();
            let delete_refresh = self.tracks.clone();
            let delete_button = ButtonView::new("DELETE")
                .action_id("audioedit.delete")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &delete_core else { return };
                    let _ = core.remove_region(track_id, clip_id as u32);
                    delete_refresh.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                });
            View::Card(vec![View::HStack(vec![
                View::Text("AUDIO EDIT".to_owned()),
                View::Caption(pitch_label),
                reverse_button.into(),
                pitch_down.into(),
                pitch_up.into(),
                split_button.into(),
                View::Caption("FADE IN".to_owned()),
                fade_in_slider.into(),
                View::Caption("FADE OUT".to_owned()),
                fade_out_slider.into(),
                loop_button.into(),
                duplicate_button.into(),
                delete_button.into(),
            ])])
        } else {
            View::Card(vec![View::HStack(vec![View::Caption(
                "Select a clip in the arrangement to enable audio edits (REVERSE, PITCH±, SPLIT)."
                    .to_owned(),
            )])])
        };

        audio_edit_card
    }
}
