//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn piano_selector_view(&self) -> View {
        let tracks = self.tracks.get();
        // Piano roll (read-only for now): track picker + note canvas.
        let piano_track = self.piano_track.get();
        let mut piano_buttons: Vec<View> = vec![View::Text("PIANO ROLL".to_owned())];
        for track in &tracks {
            let selector_track = self.piano_track.clone();
            let track_id = track.id;
            let button = ButtonView::new(format!("T{}", track.id))
                .action_id(format!("piano.track.{}", track.id))
                .on_press(move |_| selector_track.set(track_id));
            let button = if track_id == piano_track {
                button.primary()
            } else {
                button.secondary()
            };
            piano_buttons.push(button.into());
        }
        let delete_core = self.core.clone();
        let delete_notes = self.piano_notes.clone();
        let delete_selected = self.selected_note.clone();
        let delete_button = ButtonView::new("DELETE")
            .action_id("piano.delete_note")
            .secondary()
            .on_press(move |_| {
                if delete_core.is_none() {
                    return;
                }
                let Some(index) = delete_selected.get() else {
                    return;
                };
                let mut notes = delete_notes.get();
                if index < notes.len() {
                    notes.remove(index);
                    if let Some(core) = &delete_core {
                        replace_notes(core, &delete_notes, notes);
                    }
                }
                delete_selected.set(None);
            });
        piano_buttons.push(delete_button.into());
        let drum_state = self.drum_mode.clone();
        let drum_on = self.drum_mode.get();
        let drum_button = ButtonView::new(if drum_on { "PIANO" } else { "DRUM" })
            .action_id("piano.drum_mode")
            .secondary()
            .on_press(move |_| {
                drum_state.set(!drum_on);
            });
        piano_buttons.push(drum_button.into());
        let quantize_core = self.core.clone();
        let quantize_notes = self.piano_notes.clone();
        let quantize_track = self.piano_track.clone();
        let quantize_snap = self.arrange_snap_division.clone();
        let quantize_locked = self.arrange_locked.clone();
        let quantize_button = ButtonView::new("QUANTIZE")
            .action_id("piano.quantize")
            .secondary()
            .on_press(move |_| {
                if quantize_locked.get() {
                    return;
                }
                let Some(core) = &quantize_core else { return };
                let step = 4.0 / quantize_snap.get().max(1.0);
                let track_id = quantize_track.get();
                let mut notes = quantize_notes.get();
                let mut changed = false;
                for note in notes.iter_mut().filter(|note| note.track_id == track_id) {
                    let snapped = (note.start_beat / step).round() * step;
                    if snapped != note.start_beat {
                        note.start_beat = snapped.max(0.0);
                        changed = true;
                    }
                }
                if changed {
                    replace_notes(core, &quantize_notes, notes);
                }
            });
        piano_buttons.push(quantize_button.into());
        for (label, delta) in [("VEL-", -1), ("VEL+", 1)] {
            let velocity_core = self.core.clone();
            let velocity_notes = self.piano_notes.clone();
            let velocity_selected = self.selected_note.clone();
            let velocity_locked = self.arrange_locked.clone();
            let button = ButtonView::new(label)
                .action_id(format!("piano.velocity.{label}"))
                .secondary()
                .on_press(move |_| {
                    if velocity_locked.get() {
                        return;
                    }
                    let Some(core) = &velocity_core else { return };
                    let Some(index) = velocity_selected.get() else {
                        return;
                    };
                    let mut notes = velocity_notes.get();
                    let Some(note) = notes.get_mut(index) else {
                        return;
                    };
                    note.velocity = (i32::from(note.velocity) + delta).clamp(1, 127) as u8;
                    replace_notes(core, &velocity_notes, notes);
                });
            piano_buttons.push(button.into());
        }
        // Chord controls (piano roll): root/quality/length/inversion
        // steppers plus the insert action at the playhead.
        let chord_root_state = self.chord_root.clone();
        let root_down_core = self.chord_root.clone();
        let root_down = ButtonView::new("ROOT-")
            .action_id("piano.chord_root_down")
            .secondary()
            .on_press(move |_| {
                root_down_core.set(root_down_core.get().saturating_sub(1));
            });
        let root_up_core = self.chord_root.clone();
        let root_up = ButtonView::new("ROOT+")
            .action_id("piano.chord_root_up")
            .secondary()
            .on_press(move |_| {
                root_up_core.set((root_up_core.get() + 1).min(127));
            });
        let quality_state = self.chord_quality.clone();
        let quality_label = match self.chord_quality.get() {
            1 => "MIN",
            2 => "7TH",
            3 => "SUS",
            _ => "MAJ",
        };
        let quality_button = ButtonView::new(quality_label)
            .action_id("piano.chord_quality")
            .secondary()
            .on_press(move |_| {
                quality_state.set((quality_state.get() + 1) % 4);
            });
        let length_state = self.chord_length.clone();
        let length_label = format!("LEN{}", self.chord_length.get() as u32);
        let length_button = ButtonView::new(length_label)
            .action_id("piano.chord_length")
            .secondary()
            .on_press(move |_| {
                let next = match length_state.get() {
                    1.0 => 2.0,
                    2.0 => 4.0,
                    _ => 1.0,
                };
                length_state.set(next);
            });
        let inversion_state = self.chord_inversion.clone();
        let inversion_button = ButtonView::new(format!("INV{}", self.chord_inversion.get()))
            .action_id("piano.chord_inversion")
            .secondary()
            .on_press(move |_| {
                inversion_state.set((inversion_state.get() + 1) % 3);
            });
        let chord_core = self.core.clone();
        let chord_notes = self.piano_notes.clone();
        let chord_track = self.piano_track.clone();
        let chord_root = self.chord_root.clone();
        let chord_quality_state = self.chord_quality.clone();
        let chord_length_state = self.chord_length.clone();
        let chord_inversion_state = self.chord_inversion.clone();
        let chord_locked = self.arrange_locked.clone();
        let playhead_state = self.playhead_beats.clone();
        let chord_button = ButtonView::new("CHORD")
            .action_id("piano.chord_insert")
            .secondary()
            .on_press(move |_| {
                if chord_locked.get() {
                    return;
                }
                let Some(core) = &chord_core else { return };
                let root = chord_root.get();
                let quality = chord_quality_state.get();
                let length = chord_length_state.get();
                let inversion = chord_inversion_state.get();
                if !(0..=127).contains(&root)
                    || !(0.0625..=64.0).contains(&length)
                    || quality > 3
                    || inversion > 2
                {
                    return;
                }
                let base_intervals: &[i32] = match quality {
                    0 => &[0, 4, 7],
                    1 => &[0, 3, 7],
                    2 => &[0, 4, 7, 10],
                    _ => &[0, 5, 7],
                };
                let mut intervals = base_intervals.to_vec();
                let rotation = (inversion as usize).min(intervals.len().saturating_sub(1));
                for _ in 0..rotation {
                    let first = intervals.remove(0) + 12;
                    intervals.push(first);
                }
                let track_id = chord_track.get();
                let beat = playhead_state.get().max(0.0) as f32;
                let mut notes = chord_notes.get();
                for interval in &intervals {
                    let pitch = root as i32 + *interval;
                    if !(0..=127).contains(&pitch) {
                        continue;
                    }
                    let pitch = pitch as u8;
                    if notes.iter().any(|note| {
                        note.track_id == track_id
                            && note.pitch == pitch
                            && (note.start_beat - beat).abs() < 0.01
                    }) {
                        continue;
                    }
                    notes.push(PianoNote {
                        track_id,
                        region_id: 0,
                        pitch,
                        midi_channel: 0,
                        articulation: 0,
                        velocity: 100,
                        start_beat: beat,
                        length_beats: length,
                        lyric: String::new(),
                        phoneme: String::new(),
                        pitch_curve_cents: Vec::new(),
                        vibrato_depth_cents: 0,
                        vibrato_rate_millihz: 5_000,
                        portamento_samples: 0,
                        probability: 100,
                        repeat_count: 1,
                    });
                }
                // Also record a chord-track event so progression-aware
                // tooling (harmonic stapler, follow-mode voicing) sees the
                // same harmony as the notes the user just inserted.
                const TICKS_PER_BEAT: u64 = 960;
                const PITCH_NAMES: [&str; 12] = [
                    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
                ];
                let suffix = match quality {
                    0 => "",
                    1 => "m",
                    2 => "7",
                    _ => "sus4",
                };
                let name = format!("{}{}", PITCH_NAMES[(root % 12) as usize], suffix);
                let tick = (f64::from(beat) * TICKS_PER_BEAT as f64) as u64;
                let intervals_u8: Vec<u8> =
                    intervals.iter().map(|i| (*i).clamp(0, 127) as u8).collect();
                let _ = core.add_chord_event(tick, root.clamp(0, 127) as u8, intervals_u8, &name);
                replace_notes(core, &chord_notes, notes);
            });
        let clear_core = self.core.clone();
        let clear_notes = self.piano_notes.clone();
        let clear_track = self.piano_track.clone();
        let clear_locked = self.arrange_locked.clone();
        let clear_button = ButtonView::new("DELETE ALL")
            .action_id("piano.delete_all")
            .secondary()
            .on_press(move |_| {
                if clear_locked.get() {
                    return;
                }
                let Some(core) = &clear_core else { return };
                let track_id = clear_track.get();
                let notes: Vec<PianoNote> = clear_notes
                    .get()
                    .into_iter()
                    .filter(|note| note.track_id != track_id)
                    .collect();
                replace_notes(core, &clear_notes, notes);
            });
        piano_buttons.extend([
            root_down.into(),
            View::Text(format!("ROOT{}", chord_root_state.get())),
            root_up.into(),
            quality_button.into(),
            length_button.into(),
            inversion_button.into(),
            chord_button.into(),
            clear_button.into(),
        ]);
        // Progression insert: four chords
        // from the preset tables at the playhead, each a chord-length
        // apart, with the engine chord-symbol events.
        let preset_state = self.progression_preset.clone();
        let preset_button = ButtonView::new(format!("P{}", self.progression_preset.get() + 1))
            .action_id("piano.progression_preset")
            .secondary()
            .on_press(move |_| {
                preset_state.set((preset_state.get() + 1) % 3);
            });
        let prog_core = self.core.clone();
        let prog_notes = self.piano_notes.clone();
        let prog_track = self.piano_track.clone();
        let prog_root = self.chord_root.clone();
        let prog_length = self.chord_length.clone();
        let prog_inversion = self.chord_inversion.clone();
        let prog_preset = self.progression_preset.clone();
        let prog_locked = self.arrange_locked.clone();
        let prog_playhead = self.playhead_beats.clone();
        let prog_button = ButtonView::new("PROG")
            .action_id("piano.progression_insert")
            .secondary()
            .on_press(move |_| {
                if prog_locked.get() {
                    return;
                }
                let Some(core) = &prog_core else { return };
                let root = prog_root.get() as i32;
                let length = prog_length.get();
                let inversion = prog_inversion.get();
                let preset = prog_preset.get();
                if !(0..=127).contains(&root)
                    || !(0.0625..=64.0).contains(&length)
                    || preset > 2
                    || inversion > 2
                {
                    return;
                }
                let progressions: [[i32; 4]; 3] = [[0, 7, 9, 5], [9, 5, 0, 7], [0, 9, 5, 7]];
                let qualities: [[i32; 4]; 3] = [[0, 0, 1, 0], [1, 0, 0, 0], [0, 1, 0, 0]];
                let track_id = prog_track.get();
                let beat = prog_playhead.get().max(0.0) as f32;
                let mut notes = prog_notes.get();
                for chord_index in 0..4 {
                    let chord_root = root + progressions[preset as usize][chord_index];
                    let chord_quality = qualities[preset as usize][chord_index];
                    let base_intervals: &[i32] = if chord_quality == 0 {
                        &[0, 4, 7]
                    } else {
                        &[0, 3, 7]
                    };
                    let mut intervals = base_intervals.to_vec();
                    let rotation = (inversion as usize).min(intervals.len().saturating_sub(1));
                    for _ in 0..rotation {
                        let first = intervals.remove(0) + 12;
                        intervals.push(first);
                    }
                    let chord_beat = beat + chord_index as f32 * length;
                    for interval in &intervals {
                        let pitch = chord_root + *interval;
                        if !(0..=127).contains(&pitch) {
                            continue;
                        }
                        let pitch = pitch as u8;
                        if notes.iter().any(|note| {
                            note.track_id == track_id
                                && note.pitch == pitch
                                && (note.start_beat - chord_beat).abs() < 0.01
                        }) {
                            continue;
                        }
                        notes.push(PianoNote {
                            track_id,
                            region_id: 0,
                            pitch,
                            midi_channel: 0,
                            articulation: 0,
                            velocity: 100,
                            start_beat: chord_beat,
                            length_beats: length,
                            lyric: String::new(),
                            phoneme: String::new(),
                            pitch_curve_cents: Vec::new(),
                            vibrato_depth_cents: 0,
                            vibrato_rate_millihz: 5_000,
                            portamento_samples: 0,
                            probability: 100,
                            repeat_count: 1,
                        });
                    }
                    if (0..=127).contains(&chord_root) {
                        let _ = core.add_chord_event(
                            (f64::from(chord_beat.max(0.0)) * 480.0).round() as u64,
                            chord_root as u8,
                            intervals
                                .iter()
                                .map(|interval| (*interval).clamp(0, 127) as u8)
                                .collect(),
                            if chord_quality == 0 { "major" } else { "minor" },
                        );
                    }
                }
                replace_notes(core, &prog_notes, notes);
            });
        piano_buttons.push(preset_button.into());
        piano_buttons.push(prog_button.into());
        // Articulation map: MAP cycles the
        // articulation id; KS steps the keyswitch pitch (off, 60, down).
        let map_state = self.map_articulation_id.clone();
        let map_button = ButtonView::new(format!("MAP{}", self.map_articulation_id.get()))
            .action_id("piano.map_cycle")
            .secondary()
            .on_press(move |_| {
                let next = map_state.get() + 1;
                map_state.set(if next > 255 { 1 } else { next });
            });
        piano_buttons.push(map_button.into());
        let ks_core = self.core.clone();
        let ks_tracks = self.tracks.clone();
        let ks_track = self.piano_track.clone();
        let ks_map_id = self.map_articulation_id.clone();
        let ks_locked = self.arrange_locked.clone();
        let ks_current = tracks
            .iter()
            .find(|track| track.id == piano_track)
            .and_then(|track| {
                track
                    .expression_map
                    .iter()
                    .find(|entry| entry.articulation_id as u32 == self.map_articulation_id.get())
                    .and_then(|entry| entry.keyswitch_pitch)
            });
        let ks_label = match ks_current {
            Some(pitch) => format!("KS{pitch}"),
            None => "KS OFF".to_owned(),
        };
        let ks_button = ButtonView::new(ks_label)
            .action_id("piano.map_keyswitch")
            .secondary()
            .on_press(move |_| {
                if ks_locked.get() {
                    return;
                }
                let Some(core) = &ks_core else { return };
                let track_id = ks_track.get();
                let articulation_id = ks_map_id.get();
                if !(1..=255).contains(&articulation_id) {
                    return;
                }
                let tracks = ks_tracks.get();
                let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
                    return;
                };
                let mut entries = track.expression_map.clone();
                let current = entries
                    .iter()
                    .find(|entry| entry.articulation_id as u32 == articulation_id)
                    .and_then(|entry| entry.keyswitch_pitch);
                let pitch: Option<u8> = match current {
                    None => Some(60),
                    Some(0) => None,
                    Some(pitch) => Some(pitch - 1),
                };
                let id = articulation_id as u8;
                if let Some(entry) = entries.iter_mut().find(|entry| entry.articulation_id == id) {
                    entry.outputs.retain(|output| {
                        !matches!(
                            output,
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                ..
                            }
                        )
                    });
                    if let Some(pitch) = pitch {
                        entry.outputs.insert(
                            0,
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                note: pitch,
                                velocity: 127,
                                length_ticks: 0,
                            },
                        );
                        entry.keyswitch_pitch = Some(pitch);
                    } else {
                        entry.keyswitch_pitch = None;
                    }
                } else if let Some(pitch) = pitch {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name: format!("Articulation {id}"),
                        keyswitch_pitch: Some(pitch),
                        channel: 0,
                        group: 0,
                        outputs: vec![
                            hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch {
                                note: pitch,
                                velocity: 127,
                                length_ticks: 0,
                            },
                        ],
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                }
                entries.sort_by_key(|entry| entry.articulation_id);
                let accepted =
                    core.set_midi_expression_map(track_id, entries, articulation_id as u8);
                if accepted {
                    ks_tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                }
            });
        piano_buttons.push(ks_button.into());
        // Articulation name field for the MAP id.
        let name_core = self.core.clone();
        let name_tracks = self.tracks.clone();
        let name_track = self.piano_track.clone();
        let name_map_id = self.map_articulation_id.clone();
        let name_locked = self.arrange_locked.clone();
        let current_name = tracks
            .iter()
            .find(|track| track.id == piano_track)
            .and_then(|track| {
                track
                    .expression_map
                    .iter()
                    .find(|entry| entry.articulation_id as u32 == self.map_articulation_id.get())
                    .map(|entry| entry.name.clone())
            })
            .unwrap_or_default();
        let name_text = Rc::new(RefCell::new(current_name.clone()));
        let name_change_text = name_text.clone();
        let name_submit_text = name_text.clone();
        let name_field = TextFieldView::new(current_name, "Articulation name")
            .change_action_id("piano.map_name")
            .submit_action_id("piano.map_name")
            .on_change(move |value| {
                *name_change_text.borrow_mut() = value;
            })
            .on_submit(move || {
                if name_locked.get() {
                    return;
                }
                let Some(core) = &name_core else { return };
                let track_id = name_track.get();
                let articulation_id = name_map_id.get();
                if !(1..=255).contains(&articulation_id) {
                    return;
                }
                let name = name_submit_text.borrow().clone();
                if name.len() > 128 || name.contains('\0') {
                    return;
                }
                let tracks = name_tracks.get();
                let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
                    return;
                };
                let mut entries = track.expression_map.clone();
                let id = articulation_id as u8;
                if let Some(entry) = entries.iter_mut().find(|entry| entry.articulation_id == id) {
                    entry.name = name;
                } else {
                    entries.push(hirari_core_bridge::project_contracts::ExpressionMapEntry {
                        articulation_id: id,
                        name,
                        keyswitch_pitch: None,
                        channel: 0,
                        group: 0,
                        outputs: Vec::new(),
                        off_outputs: Vec::new(),
                        transition_off_outputs: Vec::new(),
                    });
                }
                entries.sort_by_key(|entry| entry.articulation_id);
                if core.set_midi_expression_map(track_id, entries, articulation_id as u8) {
                    name_tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                }
            });
        piano_buttons.push(name_field.into());
        // Lyric editor for the selected note; commits on submit, like the
        // Lyric field. The text buffer is shared between the change
        // and submit closures so Enter commits the typed value.
        let lyric_notes = self.piano_notes.clone();
        let lyric_selected = self.selected_note.clone();
        let lyric_core = self.core.clone();
        let lyric_locked = self.arrange_locked.clone();
        let lyric_text = Rc::new(RefCell::new(
            self.selected_note
                .get()
                .and_then(|index| {
                    self.piano_notes
                        .get()
                        .get(index)
                        .map(|note| note.lyric.clone())
                })
                .unwrap_or_default(),
        ));
        let lyric_change_text = lyric_text.clone();
        let lyric_submit_text = lyric_text.clone();
        let lyric_field = TextFieldView::new(lyric_text.borrow().clone(), "lyric (Enter to apply)")
            .change_action_id("piano.lyric")
            .submit_action_id("piano.lyric")
            .on_change(move |value| {
                *lyric_change_text.borrow_mut() = value;
            })
            .on_submit(move || {
                if lyric_locked.get() {
                    return;
                }
                let Some(core) = &lyric_core else { return };
                let Some(index) = lyric_selected.get() else {
                    return;
                };
                let mut notes = lyric_notes.get();
                let Some(note) = notes.get_mut(index) else {
                    return;
                };
                let lyric = lyric_submit_text.borrow().clone();
                if lyric.len() > 1_024 || lyric.contains('\0') {
                    return;
                }
                note.lyric = lyric;
                replace_notes(core, &lyric_notes, notes);
            });
        piano_buttons.push(lyric_field.into());
        // Articulation stepper for the selected note.
        for (label, delta) in [("ART-", -1), ("ART+", 1)] {
            let art_core = self.core.clone();
            let art_notes = self.piano_notes.clone();
            let art_selected = self.selected_note.clone();
            let art_locked = self.arrange_locked.clone();
            let button = ButtonView::new(label)
                .action_id(format!("piano.articulation.{label}"))
                .secondary()
                .on_press(move |_| {
                    if art_locked.get() {
                        return;
                    }
                    let Some(core) = &art_core else { return };
                    let Some(index) = art_selected.get() else {
                        return;
                    };
                    let mut notes = art_notes.get();
                    let Some(note) = notes.get_mut(index) else {
                        return;
                    };
                    note.articulation = (i32::from(note.articulation) + delta).clamp(0, 127) as u8;
                    replace_notes(core, &art_notes, notes);
                });
            piano_buttons.push(button.into());
        }
        let piano_selector = View::Card(vec![View::HStack(piano_buttons)]);

        piano_selector
    }
}
