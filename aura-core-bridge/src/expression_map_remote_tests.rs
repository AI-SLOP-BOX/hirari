#[cfg(test)]
mod remote_and_dynamics_tests {
    use super::*;

    fn articulation(id: &str, role: ArticulationRole, group: u8) -> ProArticulation {
        ProArticulation {
            id: id.into(),
            name: id.into(),
            role,
            group,
            playback_technique: id.into(),
            alias_for: None,
            fallback: None,
            remote_trigger: None,
        }
    }
    fn slot(id: &str, articulations: &[&str]) -> SoundSlot {
        SoundSlot {
            id: id.into(),
            name: id.into(),
            articulation_ids: articulations.iter().map(|id| (*id).into()).collect(),
            outputs: vec![],
            off_outputs: vec![],
            channel: None,
            transpose: 0,
            velocity_scale: 1.0,
            pitch_range: None,
            velocity_range: None,
            add_on: false,
            note_length_ticks: None,
            attack_compensation_ticks: 0,
            separation_ticks: 0,
        }
    }

    #[test]
    fn program_change_remote_mode_persists_without_note_off_semantics() {
        let mut sustain = articulation("sustain", ArticulationRole::Direction, 1);
        sustain.remote_trigger = Some(RemoteTrigger::Program { program: 5 });
        let map = ExpressionMapPro {
            name: "Brass".into(),
            articulations: vec![sustain],
            sound_slots: vec![slot("sustain", &["sustain"])],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::ProgramChange,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Program { program: 5 }, true));
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Program { program: 5 }, false));
        assert_eq!(runtime.active_directions(), vec!["sustain"]);
        assert!(!runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 5 }, true));
    }

    #[test]
    fn remote_mode_mismatch_and_duplicate_triggers_are_rejected() {
        let mut first = articulation("one", ArticulationRole::Direction, 1);
        first.remote_trigger = Some(RemoteTrigger::Key { note: 20 });
        let mut second = articulation("two", ArticulationRole::Direction, 2);
        second.remote_trigger = Some(RemoteTrigger::Key { note: 20 });
        let mut map = ExpressionMapPro {
            name: "Invalid".into(),
            articulations: vec![first, second],
            sound_slots: vec![slot("one", &["one"]), slot("two", &["two"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: true,
        };
        assert!(!map.validate());
        map.articulations[1].remote_trigger = Some(RemoteTrigger::Key { note: 21 });
        assert!(map.validate());
        map.remote_trigger_mode = RemoteTriggerMode::ProgramChange;
        assert!(!map.validate());
    }

    #[test]
    fn rejects_conflicting_articulations_from_the_same_group() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("arco", ArticulationRole::Direction, 1),
                articulation("pizz", ArticulationRole::Direction, 1),
            ],
            sound_slots: vec![slot("arco", &["arco"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        assert!(map.resolve_slot(&["arco".into(), "pizz".into()]).is_none());
    }

    #[test]
    fn dynamics_mapping_changes_velocity_and_sends_volume_and_custom_cc() {
        let mut dynamics = DynamicsMap::initialized(DynamicRange::PpppToFfff);
        dynamics.volume_output = DynamicVolumeOutput::ExpressionCc11;
        dynamics.send_controller = Some(1);
        let rendered = dynamics.apply(DynamicSymbol::Ff, 100).unwrap();
        assert!(rendered.velocity > 100);
        assert_eq!(rendered.midi_outputs.len(), 2);
        assert!(matches!(
            rendered.midi_outputs[0],
            MidiOutput::ControlChange { controller: 11, .. }
        ));
        assert!(matches!(
            rendered.midi_outputs[1],
            MidiOutput::ControlChange { controller: 1, .. }
        ));
        assert!(rendered.vst3_volume.is_none());
        assert!(dynamics.validate());
    }

    #[test]
    fn compact_dynamic_range_ignores_extreme_symbols() {
        let dynamics = DynamicsMap::initialized(DynamicRange::PpToFf);
        let rendered = dynamics.apply(DynamicSymbol::Pppp, 80).unwrap();
        assert_eq!(rendered.velocity, 80);
        assert!(rendered.midi_outputs.is_empty());
    }

    #[test]
    fn vst3_dynamic_volume_is_normalized_without_emitting_midi_volume() {
        let mut dynamics = DynamicsMap::initialized(DynamicRange::PpppToFfff);
        dynamics.change_velocities = false;
        dynamics.volume_output = DynamicVolumeOutput::Vst3Volume;
        let rendered = dynamics.apply(DynamicSymbol::Mf, 96).unwrap();
        assert_eq!(rendered.velocity, 96);
        assert!(rendered.midi_outputs.is_empty());
        assert!(rendered
            .vst3_volume
            .is_some_and(|value| (0.0..=1.0).contains(&value)));
    }
}
