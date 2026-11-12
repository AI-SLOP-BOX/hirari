//! Tests split out of midi_file.rs.

    use super::write_standard_midi;
    use crate::project_contracts::MidiNoteContract;

    fn note(track_id: u32, start_sample: u64) -> MidiNoteContract {
        MidiNoteContract {
            region_id: 0,
            midi_channel: 0,
            articulation: 0,
            track_id,
            pitch: 60,
            velocity: 100,
            start_sample,
            length_samples: 24_000,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            vibrato_rate_millihz: 5_000,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        }
    }

    #[test]
    fn writes_format_one_midi_with_tempo_and_track_chunks() {
        let path = std::env::temp_dir().join(format!("hirari-smf-{}.mid", std::process::id()));
        let mut first = note(1, 0);
        first.velocity = 37;
        first.lyric = "la".into();
        let count = write_standard_midi(&path, &[first, note(2, 48_000)], 48_000.0, 120.0)
            .expect("SMF export must succeed");
        let data = std::fs::read(&path).expect("SMF must be readable");
        assert_eq!(count, 2);
        assert_eq!(&data[..4], b"MThd");
        assert_eq!(&data[10..12], &[0, 3]);
        assert!(data.windows(4).filter(|chunk| *chunk == b"MTrk").count() == 3);
        // midi_channel 0 encodes as status 0x90 (0x90 | channel); the previous
        // expectation of 0x91 described a channel that these notes do not use.
        assert!(
            data.windows(3).any(|chunk| chunk == [0x90, 60, 37]),
            "expected note-on ch0 pitch 60 velocity 37"
        );
        assert!(
            data.windows(3).any(|chunk| chunk == [0x80, 60, 0]),
            "expected the matching note-off ch0"
        );
        assert!(data
            .windows(5)
            .any(|chunk| chunk == [0xff, 0x05, 2, b'l', b'a']));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_invalid_note_without_publishing_a_file() {
        let path =
            std::env::temp_dir().join(format!("hirari-smf-invalid-{}.mid", std::process::id()));
        let mut invalid = note(1, 0);
        invalid.velocity = 0;
        assert!(write_standard_midi(&path, &[invalid], 48_000.0, 120.0).is_err());
        assert!(!path.exists());
    }
