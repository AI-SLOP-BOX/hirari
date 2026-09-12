use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_track_following_moves_notes_to_nearest_chord_tones() {
        let mut harmony = HarmonicOrchestrator::new();
        harmony.add_chord(0, 60, vec![0, 4, 7], "C");
        let mut notes = vec![(0, 61u8), (120, 65u8), (999, 67u8)];
        assert_eq!(harmony.follow_chord_track(&mut notes), 2);
        assert_eq!(notes.iter().map(|(_, pitch)| *pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
        assert!(harmony.audit_harmonic());
    }

    #[test]
    fn validated_chord_track_updates_are_atomic_and_replace_by_tick() {
        let mut harmony = HarmonicOrchestrator::new();
        assert!(!harmony.try_add_chord(0, 60, Vec::new(), ""));
        assert!(harmony.try_add_chord(0, 60, vec![0, 4, 7], " C "));
        assert!(harmony.try_add_chord(0, 60, vec![0, 3, 7], "Cm"));
        assert_eq!(harmony.chord_progression.len(), 1);
        assert_eq!(harmony.chord_progression[0].name, "Cm");
        assert!(harmony.remove_chord_at(0));
        assert!(harmony.chord_progression.is_empty());
    }

    #[test]
    fn scale_assistant_quantizes_parts_for_extended_modes() {
        let mut harmony = HarmonicOrchestrator::new();
        harmony.set_root(60);
        harmony.set_scale(super::ScaleType::Pentatonic);
        let mut notes = [60, 61, 62, 63, 64, 65, 67];
        assert_eq!(harmony.quantize_notes(&mut notes), 3);
        assert!(notes.iter().all(|note| [60, 62, 64, 67, 69].contains(note) || *note < 60));
    }

    #[test]
    fn chord_voicing_preserves_ascending_order() {
        let mut harmony = HarmonicOrchestrator::new();
        harmony.add_chord(0, 60, vec![0, 4, 7], "C");
        let mut notes = [(0, 61u8), (0, 62u8), (0, 63u8)];
        assert_eq!(harmony.voice_chord_track(&mut notes), 3);
        assert!(notes[0].1 <= notes[1].1 && notes[1].1 <= notes[2].1);
        assert_eq!(notes.iter().map(|(_, pitch)| *pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
    }

    #[test]
    fn recognizes_inversion_and_tension_from_midi() {
        let chord = ProChordTrack::recognize_midi_chord(240, &[52, 55, 58, 60, 62], true, true).unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.quality, ChordQuality::Dominant7);
        assert_eq!(chord.bass, Some(4));
        assert_eq!(chord.tensions, vec![14]);
    }

    #[test]
    fn converts_chord_track_to_timed_editable_midi() {
        let mut track = ProChordTrack::default();
        assert!(track.upsert(ProChordEvent { tick: 0, root: 0, quality: ChordQuality::Major,
            tensions: vec![14], bass: None }));
        assert!(track.upsert(ProChordEvent { tick: 960, root: 7, quality: ChordQuality::Dominant7,
            tensions: Vec::new(), bass: Some(11) }));
        let notes = track.chords_to_midi(4, 96, 480).unwrap();
        assert!(notes.iter().filter(|note| note.tick == 0).all(|note| note.duration == 960));
        assert!(notes.iter().any(|note| note.tick == 0 && note.pitch == 74));
        assert!(notes.iter().any(|note| note.tick == 960 && note.pitch == 59));
        assert_eq!(ProChordTrack::from_json(&track.to_json().unwrap()).unwrap(), track);
    }

    #[test]
    fn rejects_unrecognizable_or_invalid_chords() {
        assert!(ProChordTrack::recognize_midi_chord(0, &[60, 64], true, true).is_none());
        let mut track = ProChordTrack::default();
        assert!(!track.upsert(ProChordEvent { tick: 0, root: 12, quality: ChordQuality::Major,
            tensions: Vec::new(), bass: None }));
    }

    #[test]
    fn chord_pads_assign_unique_track_chords_in_timeline_order() {
        let mut track = ProChordTrack::default();
        for (tick, root, quality) in [(0, 0, ChordQuality::Major), (480, 7, ChordQuality::Major),
            (960, 0, ChordQuality::Major), (1440, 9, ChordQuality::Minor)] {
            assert!(track.upsert(ProChordEvent { tick, root, quality, tensions: Vec::new(), bass: None }));
        }
        let mut rack = ChordPadRack::default();
        assert!(rack.assign_from_chord_track(&track));
        assert_eq!(rack.pads.len(), 3);
        assert_eq!(rack.pads.iter().map(|pad| pad.chord.root).collect::<Vec<_>>(), vec![0, 7, 9]);
        assert_eq!(ChordPadRack::from_json(&rack.to_json().unwrap()).unwrap(), rack);
    }

    #[test]
    fn chord_pad_remote_trigger_supports_latch_and_note_offs() {
        let mut track = ProChordTrack::default();
        assert!(track.upsert(ProChordEvent { tick: 0, root: 0, quality: ChordQuality::Major,
            tensions: Vec::new(), bass: None }));
        let mut rack = ChordPadRack::default();
        rack.latch = true;
        assert!(rack.assign_from_chord_track(&track));
        let on = rack.trigger_remote(36, 100, true).unwrap();
        assert_eq!(on.note_ons, vec![60, 64, 67]);
        assert!(rack.trigger_remote(36, 0, false).is_none());
        let off = rack.trigger_remote(36, 100, true).unwrap();
        assert_eq!(off.note_offs, vec![60, 64, 67]);
        assert!(off.note_ons.is_empty());
        assert!(rack.audit());
    }

    #[test]
    fn adaptive_voicing_limits_register_jump_and_locked_pad_rejects_edits() {
        let mut track = ProChordTrack::default();
        assert!(track.upsert(ProChordEvent { tick: 0, root: 11, quality: ChordQuality::Major,
            tensions: Vec::new(), bass: None }));
        assert!(track.upsert(ProChordEvent { tick: 480, root: 0, quality: ChordQuality::Major,
            tensions: vec![14], bass: Some(4) }));
        let mut rack = ChordPadRack::default();
        assert!(rack.assign_from_chord_track(&track));
        let first = rack.trigger_remote(36, 90, true).unwrap();
        let second = rack.trigger_remote(37, 90, true).unwrap();
        let first_center = first.note_ons.iter().map(|note| i16::from(*note)).sum::<i16>() / first.note_ons.len() as i16;
        let second_center = second.note_ons.iter().map(|note| i16::from(*note)).sum::<i16>() / second.note_ons.len() as i16;
        assert!((first_center - second_center).abs() <= 7);
        assert!(second.note_ons.contains(&52));
        assert!(rack.set_pad_lock(2, true));
        assert!(!rack.set_pad_voicing(2, 1));
        assert_eq!(rack.stop_all(), second.note_ons);
        assert!(rack.audit());
    }

    #[test]
    fn section_player_distributes_bottom_to_top_and_mutes_selected_voice() {
        let mut player = ChordSectionPlayer::default();
        player.section_keys = vec![48, 50, 52];
        player.force_single_sections = 1;
        player.muted_sections.insert(2);
        let sections = player.distribute(&[60, 64, 67, 71, 74]).unwrap();
        assert_eq!(sections[0], vec![60]);
        assert!(sections[1].is_empty());
        assert_eq!(sections[2], vec![67, 71]);
        assert_eq!(player.notes_for_section(&[60, 64, 67, 71, 74], 3).unwrap(), vec![67, 71]);
        assert!(player.audit());
    }

    #[test]
    fn subsection_assignment_transposes_its_section_and_enforces_five_key_limit() {
        let mut player = ChordSectionPlayer::default();
        player.section_keys = vec![48, 50, 52];
        player.subsection_keys = vec![72, 74];
        player.subsections = vec![
            SubsectionAssignment { section: 1, semitone_offset: -12 },
            SubsectionAssignment { section: 3, semitone_offset: 12 },
        ];
        assert_eq!(player.notes_for_subsection(&[60, 64, 67], 1).unwrap(), vec![48]);
        assert_eq!(player.notes_for_subsection(&[60, 64, 67], 2).unwrap(), vec![79]);
        assert_eq!(ChordSectionPlayer::from_json(&player.to_json().unwrap()).unwrap(), player);

        player.section_keys = vec![1, 2, 3, 4, 5, 6];
        assert!(!player.audit());
    }

    #[test]
    fn section_player_rejects_conflicting_remote_keys_and_out_of_range_transpose() {
        let mut player = ChordSectionPlayer::default();
        player.subsection_keys = vec![48];
        player.subsections = vec![SubsectionAssignment { section: 1, semitone_offset: 0 }];
        assert!(!player.audit());
        player.subsection_keys = vec![80];
        player.subsections[0].semitone_offset = 48;
        assert!(player.audit());
        assert_eq!(player.notes_for_subsection(&[100], 1),
            Err("subsection transposition exceeds MIDI range".to_owned()));
    }

    fn pattern(source: PatternVelocitySource) -> ChordPatternPlayer {
        ChordPatternPlayer { name: "Piano 8ths".into(), length_ticks: 480, voices: 3,
            velocity_source: source, steps: vec![
                ChordPatternStep { tick: 0, duration: 120, voice: 0, octave_offset: 0, velocity: 70 },
                ChordPatternStep { tick: 120, duration: 120, voice: 1, octave_offset: 0, velocity: 80 },
                ChordPatternStep { tick: 240, duration: 120, voice: 2, octave_offset: 0, velocity: 90 },
                ChordPatternStep { tick: 360, duration: 120, voice: 1, octave_offset: 1, velocity: 100 },
            ] }
    }

    #[test]
    fn pattern_player_maps_three_voice_loop_to_pad_chord_and_repeats() {
        let player = pattern(PatternVelocitySource::Pattern);
        let notes = player.render(&[60, 64, 67, 71], 960, 2, 127).unwrap();
        assert_eq!(notes.len(), 8);
        assert_eq!((notes[0].tick, notes[0].pitch, notes[0].velocity), (960, 60, 70));
        assert_eq!((notes[3].tick, notes[3].pitch), (1320, 76));
        assert_eq!(notes[4].tick, 1440);
        assert_eq!(player.progress(600), Some(0.25));
        assert_eq!(ChordPatternPlayer::from_json(&player.to_json().unwrap()).unwrap(), player);
    }

    #[test]
    fn pattern_player_can_take_velocity_from_trigger_keyboard() {
        let player = pattern(PatternVelocitySource::MidiKeyboard);
        let notes = player.render(&[55, 59, 62], 0, 1, 111).unwrap();
        assert!(notes.iter().all(|note| note.velocity == 111));
    }

    #[test]
    fn pattern_import_requires_three_to_five_voices_and_valid_midi_range() {
        let mut player = pattern(PatternVelocitySource::Pattern);
        player.voices = 2;
        assert!(!player.audit());
        player.voices = 3;
        assert!(player.render(&[60, 64], 0, 1, 100).is_err());
        player.steps[0].octave_offset = 8;
        assert!(player.render(&[60, 64, 120], 0, 1, 100).is_err());
        player.steps[0].octave_offset = 0;
        player.steps[0].duration = 481;
        assert!(!player.audit());
    }
}
