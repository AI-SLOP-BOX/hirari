//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn chord_lane_card_view(&self) -> View {
        // CHORD LANE card (chord track display): the engine's chord_track is
        // populated by the CHORD piano-roll insert; this card lists the
        // persisted events with per-event delete and a clear-all.
        let chord_lane_card = {
            let clear_core = self.core.clone();
            let clear_button = ButtonView::new("CLEAR ALL")
                .action_id("chords.clear_all")
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &clear_core {
                        let _ = core.clear_chord_track();
                    }
                });
            let mut rows: Vec<View> = Vec::new();
            rows.push(View::HStack(vec![
                View::Text("CHORD LANE".to_owned()),
                clear_button.into(),
            ]));
            if let Some(core) = self.core.as_ref() {
                let events: Vec<serde_json::Value> =
                    serde_json::from_str(&core.chord_track_json()).unwrap_or_default();
                if events.is_empty() {
                    rows.push(View::Caption(
                        "No chord events yet — tap CHORD in the piano-roll chord tools to add one."
                            .to_owned(),
                    ));
                } else {
                    const TICKS_PER_BEAT: f64 = 960.0;
                    for event in events {
                        let tick = event
                            .get("tick")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        let name = event
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?")
                            .to_owned();
                        let root = event
                            .get("root")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(60);
                        let intervals = event
                            .get("intervals")
                            .and_then(serde_json::Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item.as_u64())
                                    .map(|semitone| (root + semitone).min(127) as u8)
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .map(|note| match note % 12 {
                                        0 => "C".to_owned(),
                                        1 => "C#".to_owned(),
                                        2 => "D".to_owned(),
                                        3 => "D#".to_owned(),
                                        4 => "E".to_owned(),
                                        5 => "F".to_owned(),
                                        6 => "F#".to_owned(),
                                        7 => "G".to_owned(),
                                        8 => "G#".to_owned(),
                                        9 => "A".to_owned(),
                                        10 => "A#".to_owned(),
                                        _ => "B".to_owned(),
                                    })
                                    .collect::<Vec<_>>()
                                    .join("·")
                            })
                            .unwrap_or_default();
                        let del_core = self.core.clone();
                        let del_tick = tick;
                        let delete_button = ButtonView::new("DEL")
                            .action_id(format!("chords.del.{tick}"))
                            .secondary()
                            .on_press(move |_| {
                                if let Some(core) = &del_core {
                                    let _ = core.remove_chord_events_range(del_tick, del_tick);
                                }
                            });
                        rows.push(View::HStack(vec![
                            View::Caption(format!("{:+.1}", tick as f64 / TICKS_PER_BEAT)),
                            View::Text(name),
                            View::Caption(intervals),
                            View::Spacer,
                            delete_button.into(),
                        ]));
                    }
                }
            }
            View::Card(vec![View::VStack(rows)])
        };

        chord_lane_card
    }
}
