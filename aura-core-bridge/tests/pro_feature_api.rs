use aura_core_bridge::advanced_export_engine::*;
use aura_core_bridge::control_room::ControlRoomConsole;
use aura_core_bridge::delivery::{audit_loudness, LoudnessMeasurements, LoudnessRequirements};
use aura_core_bridge::expression_map::*;
use aura_core_bridge::hardware::*;
use aura_core_bridge::midi_logical_editor::*;
use aura_core_bridge::mpe::*;
use aura_core_bridge::pdc_manager::*;
use aura_core_bridge::plugin_catalog::PluginCollectionManager;
use aura_core_bridge::scale_assistant::{Scale, ScaleAssistantEngine};
use aura_core_bridge::sidechain_manager::*;
use aura_core_bridge::sync_transport::*;
use aura_core_bridge::vca::VcaConsole;

#[test]
fn music_expression_features_are_reachable_from_the_public_crate_api() {
    let mut scale = ScaleAssistantEngine::new();
    scale.set_root(2);
    scale.set_scale(Scale::Dorian);
    assert_eq!(
        ScaleAssistantEngine::from_json(&scale.to_json().unwrap()).unwrap(),
        scale
    );

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
            remote_trigger: None,
        }],
        sound_slots: vec![SoundSlot {
            id: "legato".into(),
            name: "Legato".into(),
            articulation_ids: vec!["legato".into()],
            outputs: vec![MidiOutput::KeySwitch {
                note: 24,
                velocity: 100,
                length_ticks: 120,
            }],
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
        }],
        default_slot: Some("legato".into()),
        remote_trigger_mode: RemoteTriggerMode::KeySwitch,
        latch_remote_triggers: false,
    };
    assert!(map.render(&["legato".into()], 60, 100, 0).is_some());
    assert_eq!(
        ExpressionMapPro::from_json(&map.to_json().unwrap()).unwrap(),
        map
    );

    let mut note = NoteExpressionNote {
        id: 1,
        pitch: 60,
        start_sample: 0,
        length_samples: 480,
        release_samples: 120,
        curves: std::collections::BTreeMap::new(),
    };
    assert!(note.upsert_point(
        NoteExpressionParameter::Pressure,
        NoteExpressionPoint {
            offset_samples: 240,
            value: 0.5
        }
    ));
    assert_eq!(
        NoteExpressionNote::from_json(&note.to_json().unwrap()).unwrap(),
        note
    );

    let preset = LogicalEditorPresetPro {
        name: "Raise selected velocities".into(),
        filter: FilterExpression::Condition(FilterCondition {
            target: FilterTarget::Selected,
            operator: FilterOperator::Equal,
            value1: 1,
            value2: 0,
        }),
        function: LogicalFunction::Transform,
        actions: vec![LogicalAction {
            target: ActionTarget::SecondaryValue,
            operation: ActionOperation::Add,
            parameter1: 10.0,
            parameter2: 0.0,
        }],
        cursor_position: 0,
        loop_range: None,
        random_seed: 1,
    };
    let mut events = vec![LogicalMidiEvent {
        id: 1,
        kind: LogicalEventKind::Note,
        channel: 0,
        position: 0,
        length: 120,
        main_value: 60,
        secondary_value: 90,
        selected: true,
        muted: false,
        note_expression: vec![],
    }];
    assert_eq!(preset.apply(&mut events).unwrap().changed, 1);
    assert_eq!(events[0].secondary_value, 100);
}

#[test]
fn studio_delivery_features_are_reachable_from_the_public_crate_api() {
    assert!(VcaConsole::from_json(&VcaConsole::default().to_json().unwrap()).is_ok());
    assert!(
        ControlRoomConsole::from_json(&ControlRoomConsole::default().to_json().unwrap()).is_ok()
    );
    assert!(ConstrainDelayCompensation::from_json(
        &ConstrainDelayCompensation::default().to_json().unwrap()
    )
    .is_ok());

    let mut sidechains = SidechainOrchestrator::new();
    assert!(sidechains
        .register_link(
            1,
            SidechainLinkRust {
                source_track_id: 1,
                dest_track_id: 2,
                plugin_idx: 0,
                input_bus: 0,
                enabled: true,
                level: 0.75,
                tap_point: SidechainTapPointRust::PostFader
            }
        )
        .is_ok());
    assert_eq!(sidechains.sources_for_input(2, 0, 0), vec![(1, 1, 0.75)]);

    let mut collections = PluginCollectionManager::new(["compressor", "synth"]);
    let favorites = collections.create("Favorites", false).unwrap();
    assert!(collections.add_plugin(&favorites, "compressor", &["Dynamics".into()]));
    assert!(collections.activate(&favorites));

    let mut remote = MidiRemoteEngine::new(1, 8).unwrap();
    assert!(remote.upsert_page(MappingPage {
        id: 1,
        name: "Transport".into(),
        factory: false,
        scope: MappingScope::Project,
        mappings: vec![RemoteMapping {
            control_id: 1,
            target: "play".into(),
            value_mode: RemoteValueMode::Toggle,
            input_mode: RemoteInputMode::Absolute,
            focus_mode: RemoteFocusMode::Fixed,
            minimum: 0.0,
            maximum: 1.0,
            inverted: false,
            transmit_feedback: true,
            bank_slot: None
        }]
    }));
    assert!(MidiRemoteEngine::from_json(&remote.to_json().unwrap()).is_ok());

    let sync = SyncProjectSettings {
        source: SyncSource::MidiPort(1),
        protocol: SyncProtocol::MidiClock,
        enabled: true,
        timecode: TimecodeLockPreferences::default(),
        midi_clock: MidiClockPreferences {
            follows_project_position: true,
            always_send_start: false,
            send_clock_in_stop: false,
        },
        midi_destinations: vec![MidiClockDestination {
            port_id: 1,
            enabled: true,
        }],
        mmc_device_id: 0x7f,
    };
    assert!(SyncProjectSettings::from_json(&sync.to_json().unwrap()).is_ok());

    let channels = [AvailableExportChannel {
        id: 1,
        name: "Main".into(),
        kind: ExportChannelKind::Output,
        channels: 2,
        selected: true,
        requires_realtime: false,
    }];
    let request = ExportRequestPro {
        project_name: "Song".into(),
        channel_ids: vec![1],
        range: ExportRangeSelection::Locators {
            start_sample: 0,
            end_sample: 48_000,
        },
        codec: CodecRust::Wav,
        sample_rate: 48_000,
        bit_depth: 24,
        effects: ExportEffectsMode::MasterGroupsAndSends,
        channel_mode: ExportChannelMode::Interleaved,
        naming: ExportNamingScheme {
            parts: vec![NamingPart::Project, NamingPart::Channel],
            separator: "_".into(),
        },
        realtime: false,
        deactivate_external_midi: true,
        existing_file_policy: ExistingFilePolicy::Error,
    };
    let plan = plan_export(&request, &channels, &[]).unwrap();
    let mut queue = ExportQueuePro::default();
    queue.enqueue(plan).unwrap();
    assert!(ExportQueuePro::from_json(&queue.to_json().unwrap()).is_ok());

    let report = audit_loudness(
        LoudnessMeasurements {
            integrated_lufs: -23.0,
            max_short_term_lufs: -18.0,
            max_momentary_lufs: -16.0,
            loudness_range_lu: 10.0,
            max_true_peak_dbtp: -1.2,
        },
        LoudnessRequirements::ebu_r128(),
    );
    assert!(report.passed);
}
