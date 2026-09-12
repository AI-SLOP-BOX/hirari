#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "aura-project-test-{}-{}.{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ))
    }

    #[test]
    fn layout_round_trips_all_project_state() {
        let layout = r#"[{"id":7,"name":"Keys","type":"Instrument","volume":0.8,"pan":-0.2,"mute":false,"solo":true,"record_armed":true,"phase_invert":true,"volume_automation":[{"time":0.0,"value":0.5,"curve":0.0},{"time":1.0,"value":0.8,"curve":0.1}],"pan_automation":[{"time":0.0,"value":0.5,"curve":0.0}],"plugin_types":[0],"plugin_bypass":[true],"plugin_parameter_values":[[0.25]],"plugin_state_hex":["00ff10"],"regions":[{"id":3,"name":"Take","start":128,"len":2048,"source_offset":256,"base_source_offset":0,"base_length":4096,"muted":false,"path":"audio/take.wav","clip_gain":0.75,"fade_in_samples":8,"fade_out_samples":16,"reverse":true,"warp_ratio":1.25,"pitch_semitones":-3.0,"loop_count":3}]}]"#;
        let document = ProjectDocument::from_layout_json("Demo", 128.0, 48_000.0, layout).unwrap();
        assert_eq!(document.tracks[0].plugin_types, vec![0]);
        assert_eq!(
            document.tracks[0].plugin_states,
            vec![vec![0x00, 0xff, 0x10]]
        );
        assert_eq!(document.tracks[0].plugin_bypasses, vec![true]);
        assert_eq!(document.tracks[0].plugin_parameter_values, vec![vec![0.25]]);
        assert_eq!(document.plugin_instances.len(), 1);
        assert_eq!(document.plugin_instances[0].track_id, 7);
        assert_eq!(document.plugin_instances[0].slot_index, 0);
        assert_eq!(document.plugin_instances[0].plugin_id, "builtin:0");
        assert_eq!(document.plugin_instances[0].capability, "builtin");
        assert!(document.plugin_instances[0].bypassed);
        assert_eq!(document.plugin_instances[0].parameter_values, vec![0.25]);
        assert!(document.tracks[0].record_armed);
        assert!(document.tracks[0].phase_invert);
        assert_eq!(document.tracks[0].volume_automation.len(), 2);
        assert_eq!(document.tracks[0].pan_automation.len(), 1);
        assert!(document.regions[0].reverse);
        assert_eq!(document.regions[0].source_offset, 256);
        assert_eq!(document.regions[0].base_length, 4096);
        let path = temp_path("aura");
        document.save_atomic(path.to_str().unwrap()).unwrap();
        let loaded = ProjectDocument::load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded, document);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn track_rename_and_remove_preserve_document_invariants() {
        let mut document = ProjectDocument::from_layout_json("Ops", 120.0, 48_000.0, "[]").unwrap();
        let first = document.add_track("Audio", "Audio").unwrap();
        let second = document.add_track("Bus", "Bus").unwrap();
        document.set_track_name(first, "Lead Vocal").unwrap();
        assert_eq!(document.tracks.iter().find(|t| t.id == first).unwrap().name, "Lead Vocal");
        document.remove_track(first).unwrap();
        assert!(document.tracks.iter().all(|t| t.id != first));
        assert_eq!(document.tracks.len(), 1);
        assert!(document.tracks.iter().any(|t| t.id == second));
        assert!(document.validate().is_ok());
    }

    #[test]
    fn routing_edges_round_trip_with_project_layout() {
        let layout = r#"[
            {"id":7,"name":"Source","type":"Audio","regions":[],
             "feedback_routes":[{"source_id":7,"destination_id":8,"gain":0.5}]},
            {"id":8,"name":"Target","type":"Audio","regions":[],
             "sidechain_routes":[{"source_id":7,"destination_id":8,"plugin_index":1,"tap_point":0}]}
        ]"#;
        let document = ProjectDocument::from_layout_json("Routing", 120.0, 48_000.0, layout).unwrap();
        assert_eq!(document.feedback_routes.len(), 1);
        assert_eq!(document.feedback_routes[0].gain, 0.5);
        assert_eq!(document.sidechain_routes.len(), 1);
        let path = temp_path("routing");
        document.save_atomic(path.to_str().unwrap()).unwrap();
        let loaded = ProjectDocument::load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.feedback_routes, document.feedback_routes);
        assert_eq!(loaded.sidechain_routes, document.sidechain_routes);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn audio_routes_round_trip_with_canonical_project_document() {
        let layout = r#"[
            {"id":21,"name":"Source","type":"Audio","regions":[]},
            {"id":22,"name":"Bus","type":"Bus","regions":[]}
        ]"#;
        let mut document =
            ProjectDocument::from_layout_json("Audio Routes", 120.0, 48_000.0, layout).unwrap();
        document.audio_routes.push(AudioRouteContract {
            source_id: 21,
            destination_id: 22,
            gain: 0.75,
        });
        document.validate().unwrap();
        let path = temp_path("audio-routes");
        document.save_atomic(path.to_str().unwrap()).unwrap();
        let loaded = ProjectDocument::load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.audio_routes, document.audio_routes);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn comping_contract_rejects_unknown_take_and_overlapping_segments() {
        let mut document = ProjectDocument::from_layout_json("Comp", 120.0, 48_000.0, "[]")
            .expect("empty project should be valid");
        document.comp_takes.push(CompTakeContract {
            id: 7,
            name: "Vocal take".into(),
            start_sample: 0,
            end_sample: 2_000,
        });
        document.comp_segments = vec![
            CompSegmentContract {
                take_id: 7,
                start_sample: 0,
                length_samples: 1_000,
                crossfade_samples: 32,
            },
            CompSegmentContract {
                take_id: 8,
                start_sample: 1_000,
                length_samples: 1_000,
                crossfade_samples: 32,
            },
        ];
        assert!(document.validate().is_err());

        document.comp_segments[1].take_id = 7;
        document.comp_segments[1].start_sample = 900;
        assert!(document.validate().is_err());
    }

    #[test]
    fn region_pitch_and_warp_are_bounded() {
        let base = r#"[{"id":1,"name":"Audio","regions":[{"id":1,"path":"take.wav","start":0,"len":64,"warp_ratio":2.5,"pitch_semitones":0.0}]}]"#;
        assert!(ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, base).is_err());

        let valid = r#"[{"id":1,"name":"Audio","regions":[{"id":1,"path":"take.wav","start":0,"len":64,"warp_ratio":0.5,"pitch_semitones":24.0}]}]"#;
        let document = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, valid).unwrap();
        assert_eq!(document.regions[0].warp_ratio, 0.5);
        assert_eq!(document.regions[0].pitch_semitones, 24.0);
        assert_eq!(document.regions[0].loop_count, 1);

        let looped = r#"[{"id":1,"name":"Audio","regions":[{"id":1,"path":"take.wav","start":0,"len":64,"loop_count":1024}]}]"#;
        let document = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, looped).unwrap();
        assert_eq!(document.regions[0].loop_count, 1024);

        let invalid_loop = r#"[{"id":1,"name":"Audio","regions":[{"id":1,"path":"take.wav","start":0,"len":64,"loop_count":1025}]}]"#;
        assert!(ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, invalid_loop).is_err());
    }

    #[test]
    fn sandbox_plugin_metadata_round_trips_without_losing_state() {
        let layout = r#"[{"id":1,"name":"Vocal","plugin_types":[4294967295],"plugin_state_hex":["00a1ff"],"sandbox_plugin_paths":["/tmp/test.clap"],"sandbox_plugin_state_hex":["00a1ff"],"regions":[]}]"#;
        let document =
            ProjectDocument::from_layout_json("Sandbox", 120.0, 48_000.0, layout).unwrap();
        assert_eq!(document.tracks[0].plugin_types, vec![u32::MAX]);
        assert_eq!(
            document.tracks[0].sandbox_plugin_paths,
            vec!["/tmp/test.clap"]
        );
        assert_eq!(
            document.tracks[0].sandbox_plugin_states,
            vec![vec![0x00, 0xa1, 0xff]]
        );
        assert_eq!(document.plugin_instances.len(), 1);
        assert_eq!(document.plugin_instances[0].track_id, 1);
        assert_eq!(document.plugin_instances[0].format, PluginFormat::Clap);
        assert_eq!(document.plugin_instances[0].capability, "sandbox");
        let path = temp_path("json");
        document.save_atomic(path.to_str().unwrap()).unwrap();
        let loaded = ProjectDocument::load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded, document);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unknown_plugin_state_versions_fail_closed() {
        let layout = r#"[{"id":1,"name":"Vocal","plugin_types":[0],"plugin_state_hex":["00a1ff"],"plugin_state_versions":[2],"regions":[]}]"#;
        let error = ProjectDocument::from_layout_json("Versioned", 120.0, 48_000.0, layout)
            .expect_err("unknown plugin state versions must not be silently restored");
        assert!(error
            .to_string()
            .contains("unsupported plugin state version"));

        let legacy_layout = r#"[{"id":1,"name":"Vocal","plugin_types":[0],"plugin_state_hex":["00a1ff"],"regions":[]}]"#;
        let document = ProjectDocument::from_layout_json("Legacy", 120.0, 48_000.0, legacy_layout)
            .expect("legacy state without a version uses the current version");
        assert_eq!(
            document.tracks[0].plugin_state_versions,
            vec![PLUGIN_STATE_SCHEMA_VERSION]
        );

        let version_zero = r#"[{"id":1,"name":"Legacy","plugin_types":[0],"plugin_state_hex":["00a1ff"],"plugin_state_versions":[0],"regions":[]}]"#;
        let migrated = ProjectDocument::from_layout_json("Migrated", 120.0, 48_000.0, version_zero)
            .expect("version zero raw state should migrate to current metadata");
        assert_eq!(
            migrated.tracks[0].plugin_state_versions,
            vec![PLUGIN_STATE_SCHEMA_VERSION]
        );
    }

    #[test]
    fn malformed_or_inconsistent_projects_are_rejected() {
        assert!(ProjectDocument::from_layout_json("Demo", 128.0, 48_000.0, "not-json").is_err());
        let mut document = ProjectDocument {
            schema_version: PROJECT_SCHEMA_VERSION,
            contract_version: PROJECT_CONTRACT_VERSION,
            project_id: new_project_id(),
            metadata: ProjectMetadata {
                name: "Demo".into(),
                version: PROJECT_SCHEMA_VERSION,
                bpm: 128.0,
                tracks_count: 1,
                key_root: 0,
                scale_type: 0,
            },
            sample_rate: 48_000.0,
            master_gain: 1.0,
            cycle_start_sample: 0,
            cycle_end_sample: 0,
            cycle_enabled: false,
            metronome_enabled: false,
            aux_track_ids: Vec::new(),
            comp_takes: Vec::new(),
            comp_segments: Vec::new(),
            tracks: vec![ProjectTrack {
                id: 1,
                name: "Track".into(),
                track_type: "Audio".into(),
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                record_armed: false,
                phase_invert: false,
                track_delay_samples: 0,
                volume_automation: Vec::new(),
                pan_automation: Vec::new(),
                track_delay_automation: Vec::new(),
                plugin_types: Vec::new(),
                plugin_bypasses: Vec::new(),
                plugin_parameter_values: Vec::new(),
                plugin_states: Vec::new(),
                plugin_gui_states: Vec::new(),
                plugin_state_versions: Vec::new(),
                sandbox_plugin_paths: Vec::new(),
                sandbox_plugin_states: Vec::new(),
                sandbox_plugin_state_versions: Vec::new(),
            }],
            regions: Vec::new(),
            plugin_instances: Vec::new(),
            midi_learn_mappings: Vec::new(),
            midi_notes: Vec::new(),
            chord_track: Vec::new(),
            midi_events: Vec::new(),
            tempo_events: Vec::new(),
            time_signature_events: Vec::new(),
            macro_mappings: Vec::new(),
            warp_markers: Vec::new(),
            render_targets: Vec::new(),
            freeze_artifacts: Vec::new(),
            sidechain_routes: Vec::new(),
            feedback_routes: Vec::new(),
            audio_routes: Vec::new(),
            openutau_vocals: Vec::new(),
            track_stacks: Vec::new(),
            markers: Vec::new(),
            vca_groups: Vec::new(),
            hardware_inserts: Vec::new(),
            control_room: None,
        };
        document.metadata.tracks_count = 2;
        assert!(document.validate().is_err());
    }

    #[test]
    fn embedded_control_room_state_is_validated_with_project_document() {
        let mut document = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, "[]")
            .expect("empty layout should be valid");
        let mut state = crate::control_room::ControlRoomState::default();
        state.monitor_outputs.clear();
        document.control_room = Some(state);
        assert!(document.validate().is_err());
    }

    #[test]
    fn truncated_project_layout_prefixes_never_hydrate_successfully() {
        let valid = r#"[{"id":1,"name":"Track","track_type":"Audio","volume":1.0,"pan":0.0,"regions":[{"id":2,"path":"a.wav","start":0,"len":64}]}]"#;
        for end in 0..valid.len() {
            let prefix = &valid[..end];
            assert!(
                ProjectDocument::from_layout_json("Truncated", 120.0, 48_000.0, prefix).is_err(),
                "truncated prefix at {end} must be rejected"
            );
        }
    }

    #[test]
    fn mutated_project_layout_corpus_never_panics_or_accepts_invalid_state() {
        let valid = br#"[{"id":1,"name":"Track","track_type":"Audio","volume":1.0,"pan":0.0,"regions":[{"id":2,"path":"a.wav","start":0,"len":64}]}]"#;
        for index in 0..valid.len() {
            let mut mutated = valid.to_vec();
            mutated[index] ^= 0xff;
            let result = std::panic::catch_unwind(|| {
                ProjectDocument::from_layout_json(
                    "Mutated",
                    120.0,
                    48_000.0,
                    std::str::from_utf8(&mutated).unwrap_or("<invalid utf8>"),
                )
            });
            assert!(result.is_ok(), "mutation at {index} must not panic");
            if let Ok(Ok(document)) = result {
                assert!(
                    document.validate().is_ok(),
                    "accepted mutation at {index} must validate"
                );
            }
        }

        let unknown_fields = r#"[{"id":1,"name":"Track","track_type":"Audio","volume":1.0,"pan":0.0,"future_field":{"nested":true},"regions":[]}]"#;
        assert!(
            ProjectDocument::from_layout_json("Unknown", 120.0, 48_000.0, unknown_fields).is_ok()
        );
    }

    #[test]
    fn region_ids_are_unique_across_tracks() {
        let layout = r#"[
            {"id":1,"name":"A","regions":[{"id":7,"path":"a.wav","start":0,"len":64}]},
            {"id":2,"name":"B","regions":[{"id":7,"path":"b.wav","start":0,"len":64}]}
        ]"#;
        assert!(ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, layout).is_err());
    }

    #[test]
    fn unsupported_schema_is_rejected_on_load() {
        let path = temp_path("json");
        std::fs::write(&path, br#"{"schema_version":99}"#).unwrap();
        let result = ProjectDocument::load(path.to_str().unwrap());
        assert!(result.is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_native_fields_are_rejected_before_hydration() {
        let mut document = ProjectDocument {
            schema_version: PROJECT_SCHEMA_VERSION,
            contract_version: PROJECT_CONTRACT_VERSION,
            project_id: new_project_id(),
            metadata: ProjectMetadata {
                name: "Demo".into(),
                version: PROJECT_SCHEMA_VERSION,
                bpm: 128.0,
                tracks_count: 1,
                key_root: 0,
                scale_type: 0,
            },
            sample_rate: 48_000.0,
            master_gain: 1.0,
            cycle_start_sample: 0,
            cycle_end_sample: 0,
            cycle_enabled: false,
            metronome_enabled: false,
            aux_track_ids: Vec::new(),
            comp_takes: Vec::new(),
            comp_segments: Vec::new(),
            tracks: vec![ProjectTrack {
                id: 1,
                name: "Track".into(),
                track_type: "Unknown".into(),
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                record_armed: false,
                phase_invert: false,
                track_delay_samples: 0,
                volume_automation: Vec::new(),
                pan_automation: Vec::new(),
                track_delay_automation: Vec::new(),
                plugin_types: Vec::new(),
                plugin_bypasses: Vec::new(),
                plugin_parameter_values: Vec::new(),
                plugin_states: Vec::new(),
                plugin_gui_states: Vec::new(),
                plugin_state_versions: Vec::new(),
                sandbox_plugin_paths: Vec::new(),
                sandbox_plugin_states: Vec::new(),
                sandbox_plugin_state_versions: Vec::new(),
            }],
            regions: Vec::new(),
            plugin_instances: Vec::new(),
            midi_learn_mappings: Vec::new(),
            midi_notes: Vec::new(),
            chord_track: Vec::new(),
            midi_events: Vec::new(),
            tempo_events: Vec::new(),
            time_signature_events: Vec::new(),
            macro_mappings: Vec::new(),
            warp_markers: Vec::new(),
            render_targets: Vec::new(),
            freeze_artifacts: Vec::new(),
            sidechain_routes: Vec::new(),
            feedback_routes: Vec::new(),
            audio_routes: Vec::new(),
            openutau_vocals: Vec::new(),
            track_stacks: Vec::new(),
            markers: Vec::new(),
            vca_groups: Vec::new(),
            hardware_inserts: Vec::new(),
            control_room: None,
        };
        assert!(document.validate().is_err());

        document.tracks[0].track_type = "Audio".into();
        document.tracks[0].pan = 2.0;
        assert!(document.validate().is_err());

        document.tracks[0].pan = 0.0;
        document.sample_rate = 48_000.5;
        assert!(document.validate().is_err());
    }

    #[test]
    fn native_track_type_mapping_is_explicit() {
        assert_eq!(ProjectDocument::native_track_type_code("Audio").unwrap(), 0);
        assert_eq!(ProjectDocument::native_track_type_code("Midi").unwrap(), 1);
        assert_eq!(
            ProjectDocument::native_track_type_code("Instrument").unwrap(),
            2
        );
        assert_eq!(ProjectDocument::native_track_type_code("Bus").unwrap(), 3);
        assert_eq!(ProjectDocument::native_track_type_code("Aux").unwrap(), 3);
        assert_eq!(ProjectDocument::native_track_type_code("Vocal").unwrap(), 4);
        assert!(ProjectDocument::native_track_type_code("audio").is_err());
    }

    #[test]
    fn project_identity_is_stable_across_serialization() {
        let document = ProjectDocument::from_layout_json("Identity", 120.0, 48_000.0, "[]").unwrap();
        assert!(Uuid::parse_str(&document.project_id).is_ok());
        let bytes = serde_json::to_vec(&document).unwrap();
        let restored: ProjectDocument = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.project_id, document.project_id);
        let other = ProjectDocument::from_layout_json("Identity", 120.0, 48_000.0, "[]").unwrap();
        assert_ne!(other.project_id, document.project_id);
    }

    #[test]
    fn freeze_artifact_metadata_round_trips_with_project() {
        let mut document = ProjectDocument::from_layout_json(
            "Freeze", 120.0, 48_000.0,
            r#"[{"id":1,"name":"Synth","track_type":"Instrument","volume":1.0,"pan":0.0,"regions":[]}]"#,
        ).unwrap();
        document.freeze_artifacts.push(FreezeArtifactContract {
            track_id: 1,
            project_generation: 4,
            audio_generation: 8,
            total_samples: 48_000,
            sample_rate: 48_000,
            path: "freeze/track-1.wav".into(),
            content_checksum: 123,
        });
        document.validate().unwrap();
        let restored: ProjectDocument = serde_json::from_slice(
            &serde_json::to_vec(&document).unwrap(),
        ).unwrap();
        assert_eq!(restored.freeze_artifacts, document.freeze_artifacts);
    }

    #[test]
    fn freeze_artifact_must_reference_an_existing_track() {
        let mut document = ProjectDocument::from_layout_json("Freeze", 120.0, 48_000.0, "[]").unwrap();
        document.freeze_artifacts.push(FreezeArtifactContract {
            track_id: 9,
            project_generation: 1,
            audio_generation: 1,
            total_samples: 1,
            sample_rate: 48_000,
            path: "freeze.wav".into(),
            content_checksum: 1,
        });
        assert!(document.validate().is_err());
    }

    #[test]
    fn aggregate_project_limits_reject_resource_exhaustion_inputs() {
        let mut document = ProjectDocument::from_layout_json("Limits", 120.0, 48_000.0, "[]").unwrap();
        document.tracks = (1..=(MAX_PROJECT_TRACKS as u32 + 1))
            .map(|id| ProjectTrack {
                id,
                name: format!("Track {id}"),
                track_type: "Audio".into(),
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                record_armed: false,
                phase_invert: false,
                track_delay_samples: 0,
                volume_automation: Vec::new(),
                pan_automation: Vec::new(),
                track_delay_automation: Vec::new(),
                plugin_types: Vec::new(),
                plugin_bypasses: Vec::new(),
                plugin_parameter_values: Vec::new(),
                plugin_states: Vec::new(),
                plugin_gui_states: Vec::new(),
                plugin_state_versions: Vec::new(),
                sandbox_plugin_paths: Vec::new(),
                sandbox_plugin_states: Vec::new(),
                sandbox_plugin_state_versions: Vec::new(),
            })
            .collect();
        document.metadata.tracks_count = document.tracks.len() as u32;
        assert!(document.validate().is_err());

    }

    #[test]
    fn scheduled_midi_notes_roundtrip_and_validate_against_tracks() {
        let mut document = ProjectDocument::from_layout_json(
            "MIDI", 120.0, 48_000.0,
            r#"[{"id":1,"name":"Keys","track_type":"Midi","volume":1.0,"pan":0.0,"regions":[]}]"#,
        ).unwrap();
        document.midi_notes.push(MidiNoteContract {
            track_id: 1, pitch: 60, velocity: 100, start_sample: 48_000,
            length_samples: 24_000,
            lyric: "la".into(),
            phoneme: "la".into(), pitch_curve_cents: vec![0, 14, -8],
            vibrato_depth_cents: 28, portamento_samples: 1200,
            probability: 100, repeat_count: 1,
        });
        document.midi_events = vec![
            crate::midi::MIDIEvent {
                beat: 1.5,
                channel: 0,
                kind: crate::midi::MIDIEventKind::ControlChange {
                    controller: 74,
                    value: 96,
                },
            },
            crate::midi::MIDIEvent {
                beat: 2.0,
                channel: 0,
                kind: crate::midi::MIDIEventKind::SysEx {
                    data: vec![0x7d, 0x01, 0x02],
                },
            },
            crate::midi::MIDIEvent {
                beat: 2.5,
                channel: 1,
                kind: crate::midi::MIDIEventKind::Midi2ChannelVoice {
                    status: 0x9,
                    index: 60,
                    value: 0x8000_0000,
                },
            },
        ];
        document.validate().unwrap();
        let restored: ProjectDocument = serde_json::from_slice(
            &serde_json::to_vec(&document).unwrap(),
        ).unwrap();
        assert_eq!(restored.midi_notes, document.midi_notes);
        assert_eq!(restored.midi_events, document.midi_events);
        document.midi_notes[0].portamento_samples = 24_001;
        assert!(document.validate().is_err());
        document.midi_notes[0].portamento_samples = 1_200;
        document.midi_notes[0].track_id = 99;
        assert!(document.validate().is_err());
    }


    #[test]
    fn reproducibility_manifest_is_deterministic_and_tracks_content_changes() {
        let mut document = ProjectDocument::from_layout_json(
            "Manifest", 120.0, 48_000.0,
            r#"[{"id":1,"name":"Vocal","track_type":"Audio","volume":1.0,"pan":0.0,"regions":[]}]"#,
        ).unwrap();
        let first = document.reproducibility_manifest().unwrap();
        let round_tripped: ProjectDocument = serde_json::from_slice(
            &serde_json::to_vec(&document).unwrap(),
        ).unwrap();
        let second = round_tripped.reproducibility_manifest().unwrap();
        assert_eq!(first["snapshot_sha256"], second["snapshot_sha256"]);
        assert_eq!(first["asset_reference_sha256"], second["asset_reference_sha256"]);
        document.master_gain = 0.75;
        let changed = document.reproducibility_manifest().unwrap();
        assert_ne!(first["snapshot_sha256"], changed["snapshot_sha256"]);
        assert_eq!(first["asset_reference_sha256"], changed["asset_reference_sha256"]);
    }
}
