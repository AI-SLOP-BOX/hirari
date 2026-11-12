//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn tempo_card_view(&self) -> View {
        // TEMPO card (global tempo editor parity): base tempo ±,
        // tap tempo, time-signature cycle (4/4 3/4 6/8 7/8), tempo-map
        // event cycler with add/remove and ramp/step mode.
        let tempo_current = self.tempo.clone();
        let tempo_core = self.core.clone();
        let tempo_down = ButtonView::new("BPM-")
            .action_id("tempo.decrement")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &tempo_core {
                    let current = tempo_current.get();
                    let _ = core.set_tempo((current - 1.0).clamp(20.0, 300.0));
                }
            });
        let tempo_current = self.tempo.clone();
        let tempo_core = self.core.clone();
        let tempo_up = ButtonView::new("BPM+")
            .action_id("tempo.increment")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &tempo_core {
                    let current = tempo_current.get();
                    let _ = core.set_tempo((current + 1.0).clamp(20.0, 300.0));
                }
            });
        let tap_core = self.core.clone();
        let tap_taps = self.tempo_taps.clone();
        let tap_status = self.status.clone();
        let tempo_tap = ButtonView::new("TAP TEMPO")
            .action_id("tempo.tap")
            .secondary()
            .on_press(move |_| {
                let now = std::time::Instant::now();
                let mut taps = tap_taps.borrow_mut();
                if taps.back().is_some_and(|previous| {
                    let elapsed = now.duration_since(*previous).as_secs_f64();
                    !(0.2..=3.0).contains(&elapsed)
                }) {
                    taps.clear();
                }
                taps.push_back(now);
                while taps.len() > 5 {
                    taps.pop_front();
                }
                if taps.len() >= 2 {
                    let intervals: Vec<f64> = taps
                        .make_contiguous()
                        .windows(2)
                        .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64())
                        .collect();
                    let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
                    let bpm = (60.0 / average).clamp(20.0, 300.0) as f32;
                    if let Some(core) = &tap_core {
                        let _ = core.set_tempo(bpm);
                        tap_status.set(format!("TAP TEMPO: {bpm:.2} BPM"));
                    }
                } else {
                    tap_status.set("TAP TEMPO: TAP AGAIN".to_owned());
                }
            });
        let signature_state = self.tempo_time_signature.clone();
        let signature_core = self.core.clone();
        let signature_playhead = self.playhead_beats.clone();
        let signature_button = ButtonView::new(format!(
            "TS {}",
            ["4/4", "3/4", "6/8", "7/8"][self.tempo_time_signature.get() as usize]
        ))
        .action_id("tempo.signature_cycle")
        .secondary()
        .on_press(move |_| {
            const TABLE: [(u8, u8); 4] = [(4, 4), (3, 4), (6, 8), (7, 8)];
            let next = (signature_state.get() + 1) % 4;
            signature_state.set(next);
            let Some(core) = &signature_core else { return };
            let (numerator, denominator) = TABLE[next as usize];
            let beat = signature_playhead.get().max(0.0);
            let _ = core.set_time_signature_event(beat, numerator, denominator);
        });
        let tempo_events: Vec<(f64, f64, bool)> = self
            .core
            .as_ref()
            .map(|core| {
                core.get_tempo_events()
                    .chunks_exact(3)
                    .map(|chunk| (chunk[0], chunk[1], chunk[2] != 0.0))
                    .collect()
            })
            .unwrap_or_default();
        let event_total = tempo_events.len();
        let event_selected = self.tempo_event_selected.get() as usize;
        let event_summary = tempo_events
            .get(event_selected)
            .map(|(beat, bpm, ramp)| {
                format!(
                    "EVT {}/{} @ {:.2}B {:.1}{}",
                    event_selected + 1,
                    event_total,
                    beat,
                    bpm,
                    if *ramp { " RAMP" } else { " STEP" }
                )
            })
            .unwrap_or_else(|| "NO EVENTS".to_owned());
        let event_selected_state = self.tempo_event_selected.clone();
        let event_prev = ButtonView::new("EV-")
            .action_id("tempo.event_prev")
            .secondary()
            .on_press(move |_| {
                let selected = event_selected_state.get();
                if selected > 0 {
                    event_selected_state.set(selected - 1);
                }
            });
        let event_vector = tempo_events.clone();
        let event_selected_state = self.tempo_event_selected.clone();
        let event_next = ButtonView::new("EV+")
            .action_id("tempo.event_next")
            .secondary()
            .on_press(move |_| {
                let selected = event_selected_state.get();
                if (selected as usize) + 1 < event_vector.len() {
                    event_selected_state.set(selected + 1);
                }
            });
        let event_ramp_state = self.tempo_event_ramp.clone();
        let event_ramp = ButtonView::new(if self.tempo_event_ramp.get() {
            "RAMP ON"
        } else {
            "STEP ON"
        })
        .action_id("tempo.ramp_toggle")
        .secondary()
        .on_press(move |_| {
            event_ramp_state.set(!event_ramp_state.get());
        });
        let event_core = self.core.clone();
        let event_add_playhead = self.playhead_beats.clone();
        let event_add_tempo = self.tempo.clone();
        let event_add_ramp = self.tempo_event_ramp.clone();
        let event_add_selected = self.tempo_event_selected.clone();
        let event_add_count = event_total as u32;
        let event_add = ButtonView::new("ADD EVT")
            .action_id("tempo.event_add")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &event_core else { return };
                let beat = event_add_playhead.get().max(0.0);
                let bpm = f64::from(event_add_tempo.get());
                let ramp = event_add_ramp.get();
                if core.set_tempo_event(beat, bpm, ramp) {
                    event_add_selected.set(event_add_count);
                }
            });
        let event_del_core = self.core.clone();
        let event_del_target = tempo_events.get(event_selected).map(|(beat, _, _)| *beat);
        let event_del_button = ButtonView::new("DEL EVT")
            .action_id("tempo.event_delete")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &event_del_core else { return };
                if let Some(beat) = event_del_target {
                    let _ = core.remove_tempo_event(beat);
                }
            });
        let tempo_card = View::Card(vec![
            View::HStack(vec![
                View::Text("TEMPO".to_owned()),
                tempo_down.into(),
                tempo_up.into(),
                tempo_tap.into(),
                signature_button.into(),
                event_prev.into(),
                View::Caption(event_summary),
                event_next.into(),
                event_ramp.into(),
                event_add.into(),
                event_del_button.into(),
            ]),
            View::Caption(format!("BPM {:.1}", self.tempo.get())),
        ]);

        tempo_card
    }
}
