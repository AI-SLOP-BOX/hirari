use super::sanitize_notes;
use crate::slint_ui::ZNote;

#[test]
fn sanitize_notes_removes_invalid_positions_and_orders_notes() {
    let mut notes = vec![
        ZNote {
            pitch: 140,
            start_beat: 4.0,
            length_beats: 0.0,
            velocity: 200,
            articulation: 0,
            selected: false,
            lyric: "".into(),
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 0,
        },
        ZNote {
            pitch: 60,
            start_beat: -2.0,
            length_beats: 1.0,
            velocity: 100,
            articulation: 0,
            selected: false,
            lyric: "".into(),
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 0,
        },
        ZNote {
            pitch: 64,
            start_beat: f32::NAN,
            length_beats: 1.0,
            velocity: 100,
            articulation: 0,
            selected: false,
            lyric: "".into(),
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 0,
        },
    ];
    sanitize_notes(&mut notes);
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].start_beat, 0.0);
    assert_eq!(notes[0].pitch, 60);
    assert_eq!(notes[0].length_beats, 1.0);
    assert_eq!(notes[1].pitch, 127);
    assert_eq!(notes[1].length_beats, 0.0625);
    assert_eq!(notes[1].velocity, 127);
}

#[test]
fn sanitize_notes_preserves_valid_selection_and_articulation() {
    let mut notes = vec![ZNote {
        pitch: 36,
        start_beat: 2.0,
        length_beats: 0.5,
        velocity: 80,
        articulation: 3,
        selected: true,
        lyric: "ka".into(),
        vibrato_amount: 0.0,
        vibrato_rate_millihz: 0,
    }];
    sanitize_notes(&mut notes);
    assert_eq!(notes[0].articulation, 3);
    assert!(notes[0].selected);
}
