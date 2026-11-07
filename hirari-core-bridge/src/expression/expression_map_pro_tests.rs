#[cfg(test)]
mod pro_tests {
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
    fn imports_vst3_preset_key_switches_as_a_complete_expression_map() {
        let switches = vec![
            VstKeySwitchInfo {
                name: " Sustain ".into(),
                note: 24,
                velocity: 100,
                length_ticks: 120,
            },
            VstKeySwitchInfo {
                name: "Pizzicato".into(),
                note: 25,
                velocity: 127,
                length_ticks: 60,
            },
        ];
        let map = ExpressionMapPro::from_vst_key_switches(" Iconica VX ", &switches).unwrap();
        assert_eq!(map.name, "Iconica VX");
        assert_eq!(map.default_slot.as_deref(), Some("vst-keyswitch-0"));
        assert_eq!(map.articulations.len(), 2);
        assert_eq!(map.sound_slots.len(), 2);
        assert_eq!(map.articulations[0].name, "Sustain");
        assert_eq!(
            map.articulations[1].remote_trigger,
            Some(RemoteTrigger::Key { note: 25 })
        );
        assert_eq!(
            map.sound_slots[1].outputs,
            vec![MidiOutput::KeySwitch {
                note: 25,
                velocity: 127,
                length_ticks: 60
            }]
        );
        assert_eq!(
            map.render(&["vst-keyswitch-1".into()], 60, 100, 0)
                .unwrap()
                .slot_id,
            "vst-keyswitch-1"
        );
        assert!(map.validate());
    }

    #[test]
    fn vst_key_switch_import_rejects_ambiguous_or_malformed_metadata() {
        let valid = VstKeySwitchInfo {
            name: "Sustain".into(),
            note: 24,
            velocity: 100,
            length_ticks: 120,
        };
        assert!(ExpressionMapPro::from_vst_key_switches("", &[valid.clone()]).is_err());
        let duplicate_note = VstKeySwitchInfo {
            name: "Pizz".into(),
            ..valid.clone()
        };
        assert!(ExpressionMapPro::from_vst_key_switches(
            "Strings",
            &[valid.clone(), duplicate_note]
        )
        .is_err());
        let duplicate_name = VstKeySwitchInfo {
            note: 25,
            ..valid.clone()
        };
        assert!(ExpressionMapPro::from_vst_key_switches(
            "Strings",
            &[valid.clone(), duplicate_name]
        )
        .is_err());
        let invalid_velocity = VstKeySwitchInfo {
            velocity: 0,
            ..valid
        };
        assert!(ExpressionMapPro::from_vst_key_switches("Strings", &[invalid_velocity]).is_err());
    }

    #[test]
    fn sound_slot_actions_preserve_a_valid_editable_map() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("sustain", ArticulationRole::Direction, 1),
                articulation("pizz", ArticulationRole::Direction, 1),
            ],
            sound_slots: vec![slot("sustain", &["sustain"]), slot("pizz", &["pizz"])],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };

        assert!(map.duplicate_sound_slot("pizz", "pizz-soft", "Pizzicato Soft"));
        assert_eq!(
            map.sound_slots
                .iter()
                .map(|slot| slot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["sustain", "pizz", "pizz-soft"]
        );
        assert!(map.rename_sound_slot("pizz-soft", "Pizzicato molto piano"));
        assert_eq!(map.slot("pizz-soft").unwrap().name, "Pizzicato molto piano");
        assert!(map.move_sound_slot("pizz-soft", SoundSlotMove::Up));
        assert_eq!(map.sound_slots[1].id, "pizz-soft");
        assert!(map.set_default_sound_slot("pizz"));
        assert_eq!(map.default_slot.as_deref(), Some("pizz"));
        assert_eq!(map.sound_slots[0].id, "pizz");
        assert!(map.remove_sound_slot("pizz"));
        assert_eq!(map.default_slot.as_deref(), Some("sustain"));
        assert!(map.validate());
    }

    #[test]
    fn invalid_sound_slot_actions_are_atomic() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![articulation("sustain", ArticulationRole::Direction, 1)],
            sound_slots: vec![slot("sustain", &["sustain"])],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let snapshot = map.clone();
        assert!(!map.duplicate_sound_slot("sustain", "sustain", "Duplicate ID"));
        assert_eq!(map, snapshot);
        assert!(!map.rename_sound_slot("sustain", "  "));
        assert_eq!(map, snapshot);
        assert!(!map.move_sound_slot("sustain", SoundSlotMove::Up));
        assert_eq!(map, snapshot);
        assert!(!map.remove_sound_slot("sustain"));
        assert_eq!(map, snapshot);
    }

    #[test]
    fn exact_sound_slot_renders_multiple_midi_outputs() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![SoundSlot {
                outputs: vec![
                    MidiOutput::KeySwitch {
                        note: 24,
                        velocity: 127,
                        length_ticks: 120,
                    },
                    MidiOutput::ControlChange {
                        controller: 1,
                        value: 96,
                    },
                ],
                channel: Some(3),
                transpose: 12,
                velocity_scale: 0.5,
                ..slot("pizz-accent", &["pizz", "accent"])
            }],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let rendered = map
            .render(&["pizz".into(), "accent".into()], 60, 100, 0)
            .unwrap();
        assert_eq!(
            (
                rendered.note,
                rendered.velocity,
                rendered.channel,
                rendered.outputs.len()
            ),
            (72, 50, 3, 2)
        );
        assert_eq!(rendered.slot_id, "pizz-accent");
        assert!(rendered.add_on_slot_ids.is_empty());
    }

    #[test]
    fn add_on_slots_layer_independent_switches_without_combination_slot() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("sordino", ArticulationRole::Direction, 2),
            ],
            sound_slots: vec![
                SoundSlot {
                    outputs: vec![MidiOutput::KeySwitch {
                        note: 24,
                        velocity: 127,
                        length_ticks: 120,
                    }],
                    ..slot("pizz", &["pizz"])
                },
                SoundSlot {
                    outputs: vec![MidiOutput::ControlChange {
                        controller: 15,
                        value: 127,
                    }],
                    add_on: true,
                    ..slot("sordino-addon", &["sordino"])
                },
            ],
            default_slot: Some("pizz".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        let rendered = map
            .render(&["pizz".into(), "sordino".into()], 60, 100, 0)
            .unwrap();
        assert_eq!(rendered.slot_id, "pizz");
        assert_eq!(rendered.add_on_slot_ids, vec!["sordino-addon"]);
        assert_eq!(
            rendered.outputs,
            vec![
                MidiOutput::KeySwitch {
                    note: 24,
                    velocity: 127,
                    length_ticks: 120
                },
                MidiOutput::ControlChange {
                    controller: 15,
                    value: 127
                },
            ]
        );
        assert!(map
            .render(&["pizz".into()], 60, 100, 0)
            .unwrap()
            .add_on_slot_ids
            .is_empty());
    }

    #[test]
    fn slot_transition_sends_off_before_on_and_exposes_timing_modifiers() {
        let mut legato_slot = slot("legato", &["legato"]);
        legato_slot.outputs = vec![MidiOutput::ControlChange {
            controller: 15,
            value: 127,
        }];
        legato_slot.off_outputs = vec![MidiOutput::ControlChange {
            controller: 15,
            value: 0,
        }];
        legato_slot.note_length_ticks = Some(360);
        legato_slot.attack_compensation_ticks = 48;
        legato_slot.separation_ticks = 12;
        let mut pizz_slot = slot("pizz", &["pizz"]);
        pizz_slot.outputs = vec![MidiOutput::KeySwitch {
            note: 24,
            velocity: 100,
            length_ticks: 30,
        }];
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("legato", ArticulationRole::Direction, 1),
                articulation("pizz", ArticulationRole::Direction, 1),
            ],
            sound_slots: vec![legato_slot, pizz_slot],
            default_slot: Some("legato".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.activate(&map, "legato"));
        let rendered = runtime.render_note(&map, 60, 100, 0).unwrap();
        let first = runtime.transition(&map, &rendered).unwrap();
        assert!(first.off_outputs.is_empty());
        assert_eq!(
            first.on_outputs,
            vec![MidiOutput::ControlChange {
                controller: 15,
                value: 127
            }]
        );
        assert_eq!(
            (
                first.note_start_offset_ticks,
                first.switch_offset_ticks,
                first.note_length_ticks
            ),
            (-48, -12, Some(360))
        );

        assert!(runtime.activate(&map, "pizz"));
        let rendered = runtime.render_note(&map, 62, 100, 0).unwrap();
        let second = runtime.transition(&map, &rendered).unwrap();
        assert_eq!(second.from_slot.as_deref(), Some("legato"));
        assert_eq!(
            second.off_outputs,
            vec![MidiOutput::ControlChange {
                controller: 15,
                value: 0
            }]
        );
        assert_eq!(
            second.on_outputs,
            vec![MidiOutput::KeySwitch {
                note: 24,
                velocity: 100,
                length_ticks: 30
            }]
        );
    }

    #[test]
    fn unchanged_slot_does_not_retransmit_on_events() {
        let mut sustain = slot("sustain", &["sustain"]);
        sustain.outputs = vec![MidiOutput::ProgramChange {
            bank_msb: None,
            bank_lsb: None,
            program: 3,
        }];
        let map = ExpressionMapPro {
            name: "Brass".into(),
            articulations: vec![articulation("sustain", ArticulationRole::Direction, 1)],
            sound_slots: vec![sustain],
            default_slot: Some("sustain".into()),
            remote_trigger_mode: RemoteTriggerMode::ProgramChange,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.activate(&map, "sustain"));
        let rendered = runtime.render_note(&map, 60, 100, 0).unwrap();
        assert_eq!(
            runtime
                .transition(&map, &rendered)
                .unwrap()
                .on_outputs
                .len(),
            1
        );
        let rendered = runtime.render_note(&map, 62, 100, 0).unwrap();
        let transition = runtime.transition(&map, &rendered).unwrap();
        assert!(transition.on_outputs.is_empty());
        assert!(transition.off_outputs.is_empty());
    }

    #[test]
    fn add_on_slot_cannot_be_the_only_or_default_base_slot() {
        let mut add_on = slot("addon", &["accent"]);
        add_on.add_on = true;
        let map = ExpressionMapPro {
            name: "Invalid".into(),
            articulations: vec![articulation("accent", ArticulationRole::Attribute, 1)],
            sound_slots: vec![add_on],
            default_slot: Some("addon".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(!map.validate());
    }

    #[test]
    fn closest_slot_prioritizes_the_most_important_group() {
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![slot("accent", &["accent"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.validate());
        assert_eq!(
            map.resolve_slot(&["pizz".into(), "accent".into()])
                .unwrap()
                .id,
            "pizz"
        );
    }

    #[test]
    fn moving_groups_changes_closest_match_priority_for_the_complete_group() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("arco", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![slot("accent", &["accent"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert_eq!(
            map.resolve_slot(&["pizz".into(), "accent".into()])
                .unwrap()
                .id,
            "pizz"
        );
        assert!(map.move_group(2, GroupMove::Up));
        assert_eq!(map.articulation("accent").unwrap().group, 1);
        assert_eq!(map.articulation("pizz").unwrap().group, 2);
        assert_eq!(map.articulation("arco").unwrap().group, 2);
        assert_eq!(
            map.resolve_slot(&["pizz".into(), "accent".into()])
                .unwrap()
                .id,
            "accent"
        );
        assert!(!map.move_group(1, GroupMove::Up));
    }

    #[test]
    fn remote_trigger_bulk_actions_are_ordered_atomic_and_mode_aware() {
        let mut map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("accent", ArticulationRole::Attribute, 2),
            ],
            sound_slots: vec![slot("accent", &["accent"]), slot("pizz", &["pizz"])],
            default_slot: None,
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        assert!(map.reassign_all_remote_triggers(24));
        assert_eq!(
            map.articulation("accent").unwrap().remote_trigger,
            Some(RemoteTrigger::Key { note: 24 })
        );
        assert_eq!(
            map.articulation("pizz").unwrap().remote_trigger,
            Some(RemoteTrigger::Key { note: 25 })
        );
        assert!(map.transpose_all_remote_triggers(12));
        assert_eq!(
            map.articulation("accent").unwrap().remote_trigger,
            Some(RemoteTrigger::Key { note: 36 })
        );
        let snapshot = map.clone();
        assert!(!map.transpose_all_remote_triggers(100));
        assert_eq!(map, snapshot);
        assert!(!map.reassign_all_remote_triggers(127));
        assert_eq!(map, snapshot);
        map.remove_all_remote_triggers();
        assert!(map
            .articulations
            .iter()
            .all(|item| item.remote_trigger.is_none()));

        map.remote_trigger_mode = RemoteTriggerMode::ProgramChange;
        assert!(map.reassign_all_remote_triggers(5));
        assert_eq!(
            map.articulation("accent").unwrap().remote_trigger,
            Some(RemoteTrigger::Program { program: 5 })
        );
    }

    #[test]
    fn directions_persist_and_attributes_apply_to_one_note() {
        let mut accent = articulation("accent", ArticulationRole::Attribute, 2);
        accent.remote_trigger = Some(RemoteTrigger::Key { note: 12 });
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![articulation("pizz", ArticulationRole::Direction, 1), accent],
            sound_slots: vec![
                slot("pizz", &["pizz"]),
                slot("pizz-accent", &["pizz", "accent"]),
            ],
            default_slot: Some("pizz".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.activate(&map, "pizz"));
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 12 }, true));
        assert_eq!(
            runtime.render_note(&map, 60, 100, 0).unwrap().slot_id,
            "pizz-accent"
        );
        assert_eq!(
            runtime.render_note(&map, 62, 100, 0).unwrap().slot_id,
            "pizz"
        );
        assert_eq!(runtime.active_directions(), vec!["pizz"]);
    }

    #[test]
    fn direction_reset_event_clears_one_group_or_all_groups() {
        let map = ExpressionMapPro {
            name: "Orchestra".into(),
            articulations: vec![
                articulation("pizz", ArticulationRole::Direction, 1),
                articulation("vibrato", ArticulationRole::Direction, 2),
            ],
            sound_slots: vec![
                slot("pizz", &["pizz"]),
                slot("vibrato", &["vibrato"]),
                slot("both", &["pizz", "vibrato"]),
            ],
            default_slot: Some("pizz".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.apply_lane_event(&map, &ExpressionLaneEvent::Articulation("pizz".into())));
        assert!(
            runtime.apply_lane_event(&map, &ExpressionLaneEvent::Articulation("vibrato".into()))
        );
        assert_eq!(
            runtime.render_note(&map, 60, 100, 0).unwrap().slot_id,
            "both"
        );
        assert!(runtime.apply_lane_event(&map, &ExpressionLaneEvent::ResetGroup(1)));
        assert_eq!(runtime.active_directions(), vec!["vibrato"]);
        assert!(runtime.apply_lane_event(&map, &ExpressionLaneEvent::ResetAllDirections));
        assert!(runtime.active_directions().is_empty());
        assert!(!runtime.apply_lane_event(&map, &ExpressionLaneEvent::ResetGroup(0)));
    }

    #[test]
    fn momentary_remote_direction_is_removed_on_key_release() {
        let mut legato = articulation("legato", ArticulationRole::Direction, 1);
        legato.remote_trigger = Some(RemoteTrigger::Key { note: 24 });
        let map = ExpressionMapPro {
            name: "Strings".into(),
            articulations: vec![legato],
            sound_slots: vec![slot("legato", &["legato"])],
            default_slot: Some("legato".into()),
            remote_trigger_mode: RemoteTriggerMode::KeySwitch,
            latch_remote_triggers: false,
        };
        let mut runtime = ExpressionMapRuntime::default();
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 24 }, true));
        assert_eq!(runtime.active_directions(), vec!["legato"]);
        assert!(runtime.trigger_remote(&map, &RemoteTrigger::Key { note: 24 }, false));
        assert!(runtime.active_directions().is_empty());
    }

}
