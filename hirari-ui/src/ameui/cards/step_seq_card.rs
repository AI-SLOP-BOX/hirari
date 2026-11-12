//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn step_seq_card_view(&self) -> View {
        // STEP SEQ card (step sequencer): editing of the selected MIDI/
        // Instrument track's notes is the ground truth; the card stores
        // lane pitches, resolution and feel per track through the bridge's
        // step_sequencer_patterns_json API. Unscoped (region_id == 0)
        // patterns bind to the track itself.
        let step_seq_card = {
            let target = self.tracks.get().into_iter().find(|track| {
                track.id == self.piano_track.get()
                    && matches!(track.kind.as_str(), "Midi" | "MIDI" | "Instrument")
            });
            if let Some(target) = target {
                let track_id = target.id;
                let track_name = target.name.clone();
                let patterns = self.step_seq_patterns.get();
                let pattern = patterns
                    .iter()
                    .find(|pattern| pattern.track_id == track_id && pattern.region_id == 0)
                    .cloned()
                    .unwrap_or_else(
                        || hirari_core_bridge::project::StepSequencerPatternContract {
                            track_id,
                            region_id: 0,
                            lane_pitches: [36, 38, 42, 46, 40, 45],
                            resolution: 16,
                            feel: 0,
                        },
                    );
                let step_beats = {
                    let mult = match pattern.feel {
                        1 => 2.0 / 3.0,
                        2 => 1.5,
                        _ => 1.0,
                    };
                    4.0 / f64::from(pattern.resolution) * mult
                };
                let lane_pitches = pattern.lane_pitches;
                let resolution = pattern.resolution;
                let feel = pattern.feel;
                let page = self.step_page.get().clamp(1, 8);
                let first_step = ((page - 1) * 16) as usize;
                let mut active = std::collections::HashSet::<(u8, usize)>::new();
                for note in self.piano_notes.get().iter() {
                    if note.track_id != track_id || note.region_id != 0 {
                        continue;
                    }
                    let Ok(step) =
                        usize::try_from(((note.start_beat / step_beats as f32).round()) as i64)
                    else {
                        continue;
                    };
                    if (note.start_beat - step as f32 * step_beats as f32).abs() > 0.01 {
                        continue;
                    }
                    if step >= first_step && step < first_step + 16 {
                        active.insert((note.pitch, step - first_step));
                    }
                }
                // Resolution cycler 4/8/16/32/64.
                let res_state = self.step_seq_patterns.clone();
                let res_core = self.core.clone();
                let resolution_button = ButtonView::new(format!("1/{resolution}"))
                    .action_id("step_seq.resolution_cycle")
                    .primary()
                    .on_press(move |_| {
                        const SET: [u8; 5] = [4, 8, 16, 32, 64];
                        let current = res_state
                            .get()
                            .iter()
                            .find(|p| p.track_id == track_id && p.region_id == 0)
                            .map(|p| p.resolution)
                            .unwrap_or(16);
                        let next = SET
                            .iter()
                            .find(|res| **res > current)
                            .copied()
                            .unwrap_or(SET[0]);
                        let mut patterns = res_state.get();
                        match patterns
                            .iter()
                            .position(|p| p.track_id == track_id && p.region_id == 0)
                        {
                            Some(index) => patterns[index].resolution = next,
                            None => patterns.push(
                                hirari_core_bridge::project::StepSequencerPatternContract {
                                    track_id,
                                    region_id: 0,
                                    lane_pitches: [36, 38, 42, 46, 40, 45],
                                    resolution: next,
                                    feel: 0,
                                },
                            ),
                        }
                        if let Some(core) = &res_core {
                            let _ = core.set_step_sequencer_patterns_json(
                                &serde_json::to_string(&patterns).unwrap_or_default(),
                            );
                        }
                        res_state.set(patterns);
                    });
                // Feel cycler straight/triplet/dotted.
                let feel_state = self.step_seq_patterns.clone();
                let feel_core = self.core.clone();
                let feel_label = match feel {
                    1 => "FEEL TRIPLET",
                    2 => "FEEL DOTTED",
                    _ => "FEEL STRAIGHT",
                };
                let feel_button = ButtonView::new(feel_label)
                    .action_id("step_seq.feel_cycle")
                    .secondary()
                    .on_press(move |_| {
                        let next = (feel + 1) % 3;
                        let mut patterns = feel_state.get();
                        match patterns
                            .iter()
                            .position(|p| p.track_id == track_id && p.region_id == 0)
                        {
                            Some(index) => patterns[index].feel = next,
                            None => patterns.push(
                                hirari_core_bridge::project::StepSequencerPatternContract {
                                    track_id,
                                    region_id: 0,
                                    lane_pitches: [36, 38, 42, 46, 40, 45],
                                    resolution: 16,
                                    feel: next,
                                },
                            ),
                        }
                        if let Some(core) = &feel_core {
                            let _ = core.set_step_sequencer_patterns_json(
                                &serde_json::to_string(&patterns).unwrap_or_default(),
                            );
                        }
                        feel_state.set(patterns);
                    });
                // Page cycler.
                let page_back_state = self.step_page.clone();
                let page_back = ButtonView::new("‹")
                    .action_id("step_seq.page_back")
                    .secondary()
                    .on_press(move |_| {
                        page_back_state.set(page_back_state.get().saturating_sub(1).max(1));
                    });
                let page_fwd_state = self.step_page.clone();
                let page_forward = ButtonView::new("›")
                    .action_id("step_seq.page_forward")
                    .secondary()
                    .on_press(move |_| {
                        page_fwd_state.set((page_fwd_state.get() + 1).min(8));
                    });
                // Lane rows with 16 toggle cells each.
                let velocity = self.step_velocity.get() as u8;
                let probability = self.step_probability.get() as u8;
                let gate = self.step_gate.get();
                let mut lane_rows: Vec<View> = Vec::with_capacity(6);
                for lane in 0..6usize {
                    let pitch = lane_pitches[lane];
                    let lane_pitch_state = self.step_seq_patterns.clone();
                    let lane_pitch_core = self.core.clone();
                    let lane_minus = ButtonView::new("−")
                        .action_id(format!("step_seq.lane_minus.{lane}"))
                        .secondary()
                        .on_press(move |_| {
                            let mut patterns = lane_pitch_state.get();
                            match patterns
                                .iter()
                                .position(|p| p.track_id == track_id && p.region_id == 0)
                            {
                                Some(index) => {
                                    if patterns[index].lane_pitches[lane] > 0
                                        && !patterns[index]
                                            .lane_pitches
                                            .contains(&(patterns[index].lane_pitches[lane] - 1))
                                    {
                                        patterns[index].lane_pitches[lane] -= 1;
                                    }
                                }
                                None => {
                                    let mut contract =
                                        hirari_core_bridge::project::StepSequencerPatternContract {
                                            track_id,
                                            region_id: 0,
                                            lane_pitches: [36, 38, 42, 46, 40, 45],
                                            resolution: 16,
                                            feel: 0,
                                        };
                                    if contract.lane_pitches[lane] > 0
                                        && !contract
                                            .lane_pitches
                                            .contains(&(contract.lane_pitches[lane] - 1))
                                    {
                                        contract.lane_pitches[lane] -= 1;
                                    }
                                    patterns.push(contract);
                                }
                            }
                            if let Some(core) = &lane_pitch_core {
                                let _ = core.set_step_sequencer_patterns_json(
                                    &serde_json::to_string(&patterns).unwrap_or_default(),
                                );
                            }
                            lane_pitch_state.set(patterns);
                        });
                    let lane_pitch_state = self.step_seq_patterns.clone();
                    let lane_pitch_core = self.core.clone();
                    let lane_plus = ButtonView::new("+")
                        .action_id(format!("step_seq.lane_plus.{lane}"))
                        .secondary()
                        .on_press(move |_| {
                            let mut patterns = lane_pitch_state.get();
                            match patterns
                                .iter()
                                .position(|p| p.track_id == track_id && p.region_id == 0)
                            {
                                Some(index) => {
                                    if patterns[index].lane_pitches[lane] < 127
                                        && !patterns[index]
                                            .lane_pitches
                                            .contains(&(patterns[index].lane_pitches[lane] + 1))
                                    {
                                        patterns[index].lane_pitches[lane] += 1;
                                    }
                                }
                                None => {
                                    let mut contract =
                                        hirari_core_bridge::project::StepSequencerPatternContract {
                                            track_id,
                                            region_id: 0,
                                            lane_pitches: [36, 38, 42, 46, 40, 45],
                                            resolution: 16,
                                            feel: 0,
                                        };
                                    if contract.lane_pitches[lane] < 127
                                        && !contract
                                            .lane_pitches
                                            .contains(&(contract.lane_pitches[lane] + 1))
                                    {
                                        contract.lane_pitches[lane] += 1;
                                    }
                                    patterns.push(contract);
                                }
                            }
                            if let Some(core) = &lane_pitch_core {
                                let _ = core.set_step_sequencer_patterns_json(
                                    &serde_json::to_string(&patterns).unwrap_or_default(),
                                );
                            }
                            lane_pitch_state.set(patterns);
                        });
                    let mut cells: Vec<View> = Vec::with_capacity(16);
                    for step in 0..16usize {
                        let is_active = active.contains(&(pitch, step));
                        let cell_core = self.core.clone();
                        let cell_notes = self.piano_notes.clone();
                        let cell_track_id = track_id;
                        let cell_pitch = pitch;
                        let cell_first = first_step;
                        let ship_step_beats = step_beats;
                        let velocity = velocity;
                        let probability = probability;
                        let gate = gate;
                        let cell_button = ButtonView::new(if is_active { "×" } else { "·" })
                            .action_id(format!("step_seq.cell.{lane}.{step}"))
                            .secondary()
                            .on_press(move |_| {
                                let step_abs = cell_first + step;
                                let start_beat = step_abs as f32 * ship_step_beats as f32;
                                let length_beats =
                                    (ship_step_beats as f32 * (gate / 100.0)).max(0.015625);
                                let mut notes: Vec<PianoNote> = cell_notes.get();
                                if let Some(index) = notes.iter().position(|note| {
                                    note.track_id == cell_track_id
                                        && note.region_id == 0
                                        && note.pitch == cell_pitch
                                        && (note.start_beat - start_beat).abs() < 0.01
                                }) {
                                    notes.remove(index);
                                } else {
                                    notes.push(PianoNote {
                                        track_id: cell_track_id,
                                        region_id: 0,
                                        pitch: cell_pitch,
                                        midi_channel: 0,
                                        articulation: 0,
                                        velocity: velocity.clamp(1, 127),
                                        start_beat,
                                        length_beats,
                                        lyric: String::new(),
                                        phoneme: String::new(),
                                        pitch_curve_cents: Vec::new(),
                                        vibrato_depth_cents: 0,
                                        vibrato_rate_millihz: 5_000,
                                        portamento_samples: 0,
                                        probability,
                                        repeat_count: 1,
                                    });
                                }
                                if let Some(core) = &cell_core {
                                    replace_notes(core, &cell_notes, notes);
                                }
                            });
                        cells.push(cell_button.into());
                    }
                    let mut lane_row = vec![
                        View::Caption(format!("LANE {} · MIDI {pitch}", lane + 1)),
                        lane_minus.into(),
                        lane_plus.into(),
                    ];
                    lane_row.extend(cells);
                    lane_rows.push(View::HStack(lane_row));
                }
                // Gate/Velocity/Probability sliders for new notes.
                let gate_state = self.step_gate.clone();
                let gate_slider = SliderView::new(self.step_gate.get(), 5.0, 400.0)
                    .action_id("step_seq.gate")
                    .on_change(move |v| gate_state.set(v.clamp(5.0, 400.0)));
                let velocity_state = self.step_velocity.clone();
                let velocity_slider = SliderView::new(self.step_velocity.get(), 1.0, 127.0)
                    .action_id("step_seq.velocity")
                    .on_change(move |v| velocity_state.set(v.clamp(1.0, 127.0)));
                let prob_state = self.step_probability.clone();
                let prob_slider = SliderView::new(self.step_probability.get(), 0.0, 100.0)
                    .action_id("step_seq.probability")
                    .on_change(move |v| prob_state.set(v.clamp(0.0, 100.0)));
                let velocity_pct = format!("{}", self.step_velocity.get() as u8);
                let probability_pct = format!("{}%", self.step_probability.get() as u8);
                let gate_pct = format!("{}%", self.step_gate.get() as u8);
                let mut rows: Vec<View> = Vec::with_capacity(lane_rows.len() + 5);
                rows.push(View::HStack(vec![
                    View::Text(format!("STEP SEQ · {track_name}")),
                    resolution_button.into(),
                    feel_button.into(),
                    page_back.into(),
                    View::Caption(format!("PAGE {page}/8")),
                    page_forward.into(),
                ]));
                rows.extend(lane_rows);
                rows.push(View::HStack(vec![
                    View::Caption("GATE".to_owned()),
                    gate_slider.into(),
                    View::Caption(gate_pct),
                    View::Caption("VELOCITY".to_owned()),
                    velocity_slider.into(),
                    View::Caption(velocity_pct),
                    View::Caption("PROB".to_owned()),
                    prob_slider.into(),
                    View::Caption(probability_pct),
                ]));
                rows.push(View::Caption(
                    "Tap a cell to toggle that step's note on the selected track (Undoable)."
                        .to_owned(),
                ));
                View::Card(vec![View::VStack(rows)])
            } else {
                View::Card(vec![
                    View::Text("STEP SEQ".to_owned()),
                    View::Caption(
                        "Select a MIDI or Instrument track in the track list to edit its step pattern."
                            .to_owned(),
                    ),
                ])
            }
        };

        step_seq_card
    }
}
