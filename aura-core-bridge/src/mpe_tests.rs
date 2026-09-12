use super::*;

#[test]
fn note_expression_round_trip_preserves_release_curves() {
    let mut note = NoteExpressionNote { id: 7, pitch: 60, start_sample: 100, length_samples: 1_000,
        release_samples: 200, curves: std::collections::BTreeMap::new() };
    assert!(note.upsert_point(NoteExpressionParameter::Pressure,
        NoteExpressionPoint { offset_samples: 1_100, value: 0.75 }));
    let json = note.to_json().unwrap();
    assert_eq!(NoteExpressionNote::from_json(&json).unwrap(), note);
    assert!(NoteExpressionNote::from_json(&json.replace("1100", "999999")).is_err());
}

fn note(id: u64, start: u64, length: u64) -> NoteExpressionNote {
    NoteExpressionNote { id, pitch: 60, start_sample: start, length_samples: length,
        release_samples: 0, curves: std::collections::BTreeMap::new() }
}

#[test]
fn overdubs_and_interpolates_per_note_expression() {
    let mut note = note(1, 100, 100);
    assert!(note.overdub(NoteExpressionParameter::Pressure, &[
        NoteExpressionPoint { offset_samples: 0, value: 0.0 },
        NoteExpressionPoint { offset_samples: 100, value: 1.0 },
    ]));
    assert!((note.evaluate(NoteExpressionParameter::Pressure, 50).unwrap() - 0.5).abs() < 0.001);
    assert!(note.validate());
}

#[test]
fn converts_cc_to_all_overlapping_notes_and_creates_release_tail() {
    let mut notes = vec![note(1, 0, 100), note(2, 50, 50)];
    let mappings = std::collections::BTreeMap::from([(74, NoteExpressionParameter::Timbre)]);
    let events = vec![MidiControllerPoint { sample: 75, controller: 74, value: 127 },
        MidiControllerPoint { sample: 120, controller: 74, value: 64 }];
    let result = convert_controllers_to_note_expression(&mut notes, &events, &mappings, 40);
    assert_eq!(result.converted_points, 2);
    assert!(result.remaining.is_empty());
    assert!(notes.iter().all(|note| note.curves[&NoteExpressionParameter::Timbre].len() == 2));
    assert!(notes.iter().all(|note| note.release_samples == 20));
}

#[test]
fn paste_scales_timing_and_trim_removes_release_data() {
    let mut source = note(1, 0, 100);
    source.set_release_length(100);
    assert!(source.overdub(NoteExpressionParameter::Pitch, &[
        NoteExpressionPoint { offset_samples: 0, value: -1.0 },
        NoteExpressionPoint { offset_samples: 200, value: 1.0 },
    ]));
    let mut target = note(2, 0, 50); target.set_release_length(50);
    assert!(target.paste_scaled(&source, NoteExpressionParameter::Pitch, NoteExpressionParameter::Timbre));
    assert_eq!(target.curves[&NoteExpressionParameter::Timbre][1].offset_samples, 100);
    target.trim_to_note_length();
    assert_eq!(target.curves[&NoteExpressionParameter::Timbre].len(), 1);
}

#[test]
fn repeats_selected_expression_section_atomically() {
    let mut note = note(1, 0, 400);
    assert!(note.overdub(NoteExpressionParameter::Pressure, &[
        NoteExpressionPoint { offset_samples: 0, value: 0.0 },
        NoteExpressionPoint { offset_samples: 50, value: 1.0 },
    ]));
    assert!(note.repeat_section(NoteExpressionParameter::Pressure, 0, 100, 2));
    let offsets: Vec<_> = note.curves[&NoteExpressionParameter::Pressure].iter().map(|point| point.offset_samples).collect();
    assert_eq!(offsets, vec![0, 50, 100, 150, 200, 250]);
}

#[test]
fn setup_converts_pitch_aftertouch_cc_and_pitch_specific_poly_pressure() {
    let mut notes = vec![note(1, 100, 100), NoteExpressionNote { id: 2, pitch: 64, ..note(2, 100, 100) }];
    let channels = std::collections::BTreeMap::from([(1, 1), (2, 1)]);
    let setup = NoteExpressionMidiSetup::cubase_default();
    let events = [
        IncomingExpressionEvent { sample: 120, channel: 1, kind: IncomingExpressionKind::ControlChange { controller: 74, value: 127 } },
        IncomingExpressionEvent { sample: 130, channel: 1, kind: IncomingExpressionKind::PitchBend { value: 4096 } },
        IncomingExpressionEvent { sample: 140, channel: 1, kind: IncomingExpressionKind::Aftertouch { value: 96 } },
        IncomingExpressionEvent { sample: 150, channel: 1, kind: IncomingExpressionKind::PolyPressure { note: 64, value: 100 } },
    ];
    let result = convert_midi_expression_with_setup(&mut notes, &channels, &events, &setup);
    assert_eq!(result.converted_points, 4);
    assert!(result.remaining.is_empty());
    assert!(notes[0].curves.contains_key(&NoteExpressionParameter::Controller(74)));
    assert!(notes[0].curves.contains_key(&NoteExpressionParameter::Pitch));
    assert_eq!(notes[0].curves[&NoteExpressionParameter::Pressure].len(), 1);
    assert_eq!(notes[1].curves[&NoteExpressionParameter::Pressure].len(), 2);
}

#[test]
fn controller_catch_uses_nearest_upcoming_note_and_leaves_disabled_cc() {
    let mut notes = vec![note(1, 100, 50), note(2, 140, 50)];
    let channels = std::collections::BTreeMap::from([(1, 2), (2, 2)]);
    let mut setup = NoteExpressionMidiSetup::cubase_default();
    setup.controller_catch_samples = 50;
    let events = [
        IncomingExpressionEvent { sample: 90, channel: 2, kind: IncomingExpressionKind::ControlChange { controller: 74, value: 64 } },
        IncomingExpressionEvent { sample: 110, channel: 2, kind: IncomingExpressionKind::ControlChange { controller: 1, value: 127 } },
    ];
    let result = convert_midi_expression_with_setup(&mut notes, &channels, &events, &setup);
    assert_eq!(result.converted_points, 1);
    assert_eq!(result.remaining, vec![events[1]]);
    assert_eq!(notes[0].curves[&NoteExpressionParameter::Controller(74)][0].offset_samples, 0);
    assert!(!notes[1].curves.contains_key(&NoteExpressionParameter::Controller(74)));
}

#[test]
fn channel_specific_expression_does_not_leak_to_other_mpe_voice() {
    let mut notes = vec![note(1, 0, 100), note(2, 0, 100)];
    let channels = std::collections::BTreeMap::from([(1, 2), (2, 3)]);
    let event = IncomingExpressionEvent { sample: 50, channel: 3,
        kind: IncomingExpressionKind::PitchBend { value: -8192 } };
    let result = convert_midi_expression_with_setup(&mut notes, &channels, &[event], &NoteExpressionMidiSetup::cubase_default());
    assert_eq!(result.converted_points, 1);
    assert!(!notes[0].curves.contains_key(&NoteExpressionParameter::Pitch));
    assert_eq!(notes[1].curves[&NoteExpressionParameter::Pitch][0].value, -1.0);
}
