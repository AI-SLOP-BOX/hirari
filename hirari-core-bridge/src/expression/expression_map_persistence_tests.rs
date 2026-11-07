#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn pro_expression_map_round_trips_only_valid_alias_graphs() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![ProArticulation {
                id: "legato".into(),
                name: "Legato".into(),
                role: ArticulationRole::Direction,
                group: 1,
                playback_technique: "legato".into(),
                alias_for: None,
                fallback: None,
                remote_trigger: Some(RemoteTrigger::Key { note: 24 }),
            }],
            sound_slots: vec![SoundSlot {
                id: "legato-slot".into(),
                name: "Legato".into(),
                articulation_ids: vec!["legato".into()],
                outputs: vec![MidiOutput::KeySwitch {
                    note: 24,
                    velocity: 100,
                    length_ticks: 120,
                }],
                off_outputs: Vec::new(),
                channel: None,
                transpose: 0,
                velocity_scale: 1.0,
                pitch_range: None,
                velocity_range: None,
                add_on: false,
                note_length_ticks: None,
                attack_compensation_ticks: 0,
                separation_ticks: 0,
            }],
            default_slot: Some("legato-slot".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: true,
        };
        let json = map.to_json().unwrap();
        assert_eq!(ExpressionMapPro::from_json(&json).unwrap(), map);
        let invalid = json.replace(
            "\"default_slot\":\"legato-slot\"",
            "\"default_slot\":\"missing-slot\"",
        );
        assert!(ExpressionMapPro::from_json(&invalid).is_err());
    }
}
