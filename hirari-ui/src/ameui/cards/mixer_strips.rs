//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn mixer_strips_view(&self) -> View {
        let tracks = self.tracks.get();
        // Mixer strips: the canvas draws geometry only (no text), so the
        // list below carries names plus the fader/pan/mute/solo/arm
        // controls, wired to the same engine setters as the mixer.
        let mixer_core = self.core.clone();
        let track_list = View::ScrollView {
            height: 260,
            follow_bottom: false,
            scroll_to: None,
            children: vec![View::VStack(
                tracks
                    .iter()
                    .map(|track| {
                        let track_id = track.id;
                        let volume_core = mixer_core.clone();
                        let volume = SliderView::new(track.volume, 0.0, 2.0)
                            .action_id(format!("mixer.volume.{}", track.id))
                            .on_change(move |value| {
                                if let Some(core) = &volume_core {
                                    core.set_track_fader(track_id, value.clamp(0.0, 2.0));
                                }
                            });
                        let pan_core = mixer_core.clone();
                        let pan = SliderView::new(track.pan, -1.0, 1.0)
                            .action_id(format!("mixer.pan.{}", track.id))
                            .on_change(move |value| {
                                if let Some(core) = &pan_core {
                                    core.set_track_pan(track_id, value.clamp(-1.0, 1.0));
                                }
                            });
                        let mute_core = mixer_core.clone();
                        let muted = track.muted;
                        let mute_button = ButtonView::new(if muted { "Unmute" } else { "Mute" })
                            .action_id(format!("mixer.mute.{}", track.id))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &mute_core {
                                    core.set_mute(track_id, !muted);
                                }
                            });
                        let solo_core = mixer_core.clone();
                        let solo = track.solo;
                        let solo_button = ButtonView::new(if solo { "Unsolo" } else { "Solo" })
                            .action_id(format!("mixer.solo.{}", track.id))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &solo_core {
                                    core.set_solo(track_id, !solo);
                                }
                            });
                        let arm_core = mixer_core.clone();
                        let armed = track.record_armed;
                        let arm_button = ButtonView::new(if armed { "Disarm" } else { "Arm" })
                            .action_id(format!("mixer.arm.{}", track.id))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &arm_core {
                                    let _ = core.set_track_armed(track_id, !armed);
                                }
                            });
                        View::Card(vec![View::VStack(vec![
                            View::HStack(vec![
                                View::Text(format!("#{}", track.id)),
                                View::Text(track.name.clone()).grow(1),
                                View::Caption(format!(
                                    "{} · {} clips",
                                    track.kind,
                                    track.clips.len()
                                )),
                            ]),
                            View::HStack(vec![
                                View::Caption("Vol".to_owned()),
                                volume.into(),
                                View::Caption("Pan".to_owned()),
                                pan.into(),
                            ]),
                            View::HStack(vec![
                                mute_button.into(),
                                solo_button.into(),
                                arm_button.into(),
                            ]),
                        ])])
                    })
                    .collect(),
            )],
        };

        track_list
    }
}
