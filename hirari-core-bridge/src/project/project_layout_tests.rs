//! Tests split out of project_layout.rs.
use super::*;

    use crate::region_processing_history::HirariEventStep;

    #[test]
    fn escapes_exactly_the_bytes_the_cpp_writer_escaped() {
        let mut writer = LayoutWriter {
            out: Vec::new(),
            cancel: None,
            sidechain_routes: &[],
            feedback_routes: &[],
        };
        writer.text(b"a\"b\\c\nd\re\tf\x01g");
        assert_eq!(writer.out, b"a\\\"b\\\\c\\nd\\re\\tf?g");
    }

    fn layout_string(value: &str) -> HirariLayoutString {
        HirariLayoutString {
            data: value.as_ptr(),
            len: value.len(),
        }
    }

    fn write_json(snapshot: &HirariProjectLayoutSnapshot) -> Option<String> {
        let mut data = std::ptr::null_mut();
        let mut size = 0usize;
        let ok = unsafe {
            hirari_project_layout_write_json(
                snapshot,
                None,
                std::ptr::null(),
                &mut data,
                &mut size,
            )
        };
        if !ok {
            assert!(data.is_null());
            return None;
        }
        let text = String::from_utf8(unsafe { std::slice::from_raw_parts(data, size) }.to_vec())
            .expect("layout JSON is UTF-8");
        unsafe { hirari_project_layout_json_free(data, size) };
        Some(text)
    }

    fn empty_track(id: u32, name: &str) -> HirariLayoutTrack {
        HirariLayoutTrack {
            id,
            name: layout_string(name),
            volume: 1.0,
            frozen_sample_rate: 44_100,
            ..HirariLayoutTrack::default()
        }
    }

    struct Stores {
        notes: *mut HirariRegionVariAudioStore,
        history: *mut HirariRegionProcessingHistoryStore,
        frozen: *mut HirariVariAudioFrozen,
    }

    impl Stores {
        fn new() -> Self {
            Stores {
                notes: unsafe { crate::vari_audio::hirari_region_vari_audio_store_new() },
                history: unsafe {
                    crate::region_processing_history::hirari_region_processing_history_new()
                },
                frozen: std::ptr::null_mut(),
            }
        }
    }

    impl Drop for Stores {
        fn drop(&mut self) {
            unsafe {
                if !self.frozen.is_null() {
                    crate::vari_audio::hirari_region_vari_audio_frozen_free(self.frozen);
                }
                crate::vari_audio::hirari_region_vari_audio_store_clear(self.notes);
                crate::region_processing_history::hirari_region_processing_history_clear(
                    self.history,
                );
            }
        }
    }

    #[test]
    fn serializes_the_full_layout_with_the_legacy_key_order_and_number_format() {
        let stores = Stores::new();
        let segment = HirariAudioNoteSegment {
            start_seconds: 1.25,
            end_seconds: 2.5,
            detected_pitch_cents: 6900.0,
            pitch_offset_cents: 10.0,
            formant_offset_cents: -20.0,
        };
        let anchor = HirariAudioNoteAnchor {
            position_seconds: 1.5,
            pitch_cents: 6900.0,
            formant_cents: 0.0,
        };
        assert!(unsafe {
            crate::vari_audio::hirari_region_vari_audio_store_upsert(
                stores.notes,
                segment,
                &anchor,
                1,
            )
        });
        let operation = b"gain";
        let step = HirariEventStep {
            id: 1,
            operation: operation.as_ptr(),
            operation_len: operation.len(),
            parameter: 0.5,
            enabled: true,
        };
        assert!(unsafe {
            crate::region_processing_history::hirari_region_processing_history_append(
                stores.history,
                step,
            )
        });

        let track_name = "Lead \"A\"";
        let expression_map = "{}";
        let volume_automation = [HirariLayoutAutomationPoint {
            time: 0.5,
            value: 0.25,
            curve: 0.0,
        }];
        let plugin_types = [1u32, 2];
        let plugin_bypass = [0u8, 1];
        let first_parameters = [0.5f32, 1.0 / 3.0];
        let second_parameters: [f32; 0] = [];
        let plugin_parameters = [
            HirariLayoutParameterBlock {
                values: first_parameters.as_ptr(),
                count: first_parameters.len(),
            },
            HirariLayoutParameterBlock {
                values: second_parameters.as_ptr(),
                count: 0,
            },
        ];
        let first_state = [0x00u8, 0xab];
        let plugin_states = [HirariLayoutByteBlock {
            bytes: first_state.as_ptr(),
            count: first_state.len(),
        }];
        let sidechain_routes = [HirariLayoutSidechainRoute {
            source_id: 9,
            destination_id: 5,
            plugin_index: 1,
            tap_point: 2,
        }];
        let feedback_routes = [HirariLayoutFeedbackRoute {
            source_id: 5,
            destination_id: 7,
            gain: 0.25,
        }];

        let region_name = "Vox";
        let region_path = "/tmp/missing.wav";
        let warp_markers = [HirariWarpMarker {
            source_sample: 0,
            timeline_sample: 0,
            transient: 1,
        }];
        let range_edits = [NativeRangeEdit {
            start: 0,
            end: 10,
            gain: 0.5,
            fade_in: 1,
            fade_out: 2,
        }];
        let regions = [HirariLayoutRegion {
            id: 100,
            name: layout_string(region_name),
            state: HirariRegionState {
                start: 1000,
                len: 2000,
                base_start: 1000,
                base_length: 2000,
                source_offset: 50,
                base_source_offset: 50,
                source_length: 2200,
                fade_in_samples: 10,
                fade_out_samples: 20,
                warp_ratio: 1.0,
                source_sample_rate: 48_000.0,
                timeline_sample_rate: 48_000.0,
                clip_gain: 0.75,
                pitch_semitones: 0.0,
                loop_count: 1,
                sync_group: 3,
                muted: false,
                reverse: false,
                locked: false,
                pitch_preserve_warp: false,
            },
            source_sample_rate: 48_000,
            path: layout_string(region_path),
            missing: true,
            audio_loaded: false,
            warp_markers: warp_markers.as_ptr(),
            warp_marker_count: warp_markers.len(),
            range_edits: range_edits.as_ptr(),
            range_edit_count: range_edits.len(),
            note_store: stores.notes,
            note_frozen: std::ptr::null(),
            processing_store: stores.history,
        }];

        let tracks = [HirariLayoutTrack {
            id: 5,
            name: layout_string(track_name),
            track_type: 1,
            volume: 0.5,
            pan: -0.25,
            muted: false,
            solo: true,
            record_armed: false,
            phase_inverted: true,
            track_delay_samples: 128,
            frozen: false,
            frozen_total_samples: 0,
            frozen_sample_rate: 44_100,
            frozen_path: layout_string(""),
            volume_automation: volume_automation.as_ptr(),
            volume_automation_count: volume_automation.len(),
            pan_automation: std::ptr::null(),
            pan_automation_count: 0,
            track_delay_automation: std::ptr::null(),
            track_delay_automation_count: 0,
            expression_map_json: layout_string(expression_map),
            expression_map_pro_json: layout_string(expression_map),
            plugin_automation: std::ptr::null(),
            plugin_automation_count: 0,
            plugin_types: plugin_types.as_ptr(),
            plugin_type_count: plugin_types.len(),
            plugin_bypass: plugin_bypass.as_ptr(),
            plugin_parameters: plugin_parameters.as_ptr(),
            plugin_states: plugin_states.as_ptr(),
            plugin_state_count: plugin_states.len(),
            sandbox_plugin_paths: std::ptr::null(),
            sandbox_plugin_path_count: 0,
            sandbox_plugin_states: std::ptr::null(),
            sandbox_plugin_state_count: 0,
            regions: regions.as_ptr(),
            region_count: regions.len(),
            ..Default::default()
        }];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            sidechain_routes: sidechain_routes.as_ptr(),
            sidechain_route_count: sidechain_routes.len(),
            feedback_routes: feedback_routes.as_ptr(),
            feedback_route_count: feedback_routes.len(),
            timeline_sample_rate: 48_000.0,
        };

        let json = write_json(&snapshot).expect("layout serializes");

        let marker_hash = fnv1a(&0.0f32.to_ne_bytes()).to_string();
        let region = concat!(
            r#"{"id":100,"name":"Vox","start":1000,"len":2000,"source_length":2200,"#,
            r#""source_sample_rate":48000,"source_offset":50,"base_start":1000,"#,
            r#""base_source_offset":50,"base_length":2000,"muted":false,"#,
            r#""path":"/tmp/missing.wav","missing":true,"audio_loaded":false,"#,
            r#""clip_gain":0.750000,"fade_in_samples":10,"fade_out_samples":20,"#,
            r#""reverse":false,"warp_ratio":1.000000,"pitch_preserve_warp":false,"#,
            r#""pitch_semitones":0.000000,"loop_count":1,"sync_group":3,"#,
            r#""warp_markers":[{"source_sample":0,"timeline_sample":0,"transient":true}],"#,
            r#""range_edits":[{"start":0,"end":10,"gain":0.500000,"fade_in":1,"fade_out":2}],"#,
            r#""audio_note_segments":[{"start":1.250000,"end":2.500000,"#,
            r#""detected_pitch_cents":6900.000000,"pitch_offset_cents":10.000000,"#,
            r#""formant_offset_cents":-20.000000,"anchors":[{"position":1.500000,"#,
            r#""pitch_cents":6900.000000,"formant_cents":0.000000}]}],"#,
            r#""processing_history":[{"id":1,"operation":"gain","parameter":0.500000,"enabled":true}]}"#,
        );
        let expected = format!(
            concat!(
                r#"[{{"id":5,"name":"Lead \"A\"","type":"Midi","volume":0.500000,"#,
                r#""pan":-0.250000,"mute":false,"solo":true,"record_armed":false,"#,
                r#""phase_invert":true,"track_delay_samples":128,"frozen":false,"#,
                r#""frozen_total_samples":0,"frozen_sample_rate":44100,"frozen_path":"","#,
                r#""volume_automation":[{{"time":0.500000,"value":0.250000,"curve":0.000000}}],"#,
                r#""pan_automation":[],"track_delay_automation":[],"#,
                r#""expression_map":{{}},"expression_map_pro":{{}},"plugin_automation":[],"#,
                r#""plugin_types":[1,2],"plugin_bypass":[false,true],"#,
                r#""plugin_parameter_values":[[0.500000,0.333333],[]],"#,
                r#""plugin_state_sizes":[2,4],"plugin_state_hashes":[{},{}],"#,
                r#""plugin_state_hex":["00ab","00000000"],"sandbox_plugin_paths":[],"#,
                r#""sandbox_plugin_state_hex":[],"#,
                r#""sidechain_routes":[{{"source_id":9,"destination_id":5,"#,
                r#""plugin_index":1,"tap_point":2}}],"#,
                r#""feedback_routes":[{{"source_id":5,"destination_id":7,"gain":0.250000}}],"#,
                r#""regions":[{}]}}]"#,
            ),
            fnv1a(&first_state),
            marker_hash,
            region,
        );
        assert_eq!(json, expected);
    }

    #[test]
    fn skips_the_empty_bootstrap_track() {
        let bootstrap_name = "Audio 1";
        let other_name = "Audio 2";
        let tracks = [
            empty_track(0, bootstrap_name),
            empty_track(1, other_name),
        ];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        let json = write_json(&snapshot).expect("layout serializes");
        assert!(json.starts_with(r#"[{"id":1,"name":"Audio 2","#));
        assert!(!json.contains("Audio 1"));
    }

    #[test]
    fn keeps_the_bootstrap_track_when_it_is_the_only_track() {
        let bootstrap_name = "Audio 1";
        let tracks = [empty_track(0, bootstrap_name)];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        let json = write_json(&snapshot).expect("layout serializes");
        assert!(json.starts_with(r#"[{"id":0,"name":"Audio 1","#));
    }

    #[test]
    fn cancellation_aborts_the_layout() {
        let other_name = "Audio 2";
        let tracks = [empty_track(1, other_name)];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        unsafe extern "C" fn cancel(_: *const c_void) -> bool {
            true
        }
        let mut data = std::ptr::null_mut();
        let mut size = 0usize;
        assert!(!unsafe {
            hirari_project_layout_write_json(
                &snapshot,
                Some(cancel),
                std::ptr::null(),
                &mut data,
                &mut size,
            )
        });
        assert!(data.is_null());
    }

    #[test]
    fn non_finite_numbers_fall_back_to_the_legacy_placeholders() {
        let other_name = "Audio 2";
        let plugin_types = [1u32];
        let plugin_bypass = [0u8];
        let parameters = [f32::NAN];
        let blocks = [HirariLayoutParameterBlock {
            values: parameters.as_ptr(),
            count: parameters.len(),
        }];
        let tracks = [HirariLayoutTrack {
            volume: f32::NAN,
            plugin_types: plugin_types.as_ptr(),
            plugin_type_count: plugin_types.len(),
            plugin_bypass: plugin_bypass.as_ptr(),
            plugin_parameters: blocks.as_ptr(),
            ..empty_track(1, other_name)
        }];
        let snapshot = HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        };
        let json = write_json(&snapshot).expect("layout serializes");
        assert!(json.contains(r#""volume":0,"#));
        assert!(json.contains(r#""plugin_parameter_values":[[0.0]]"#));
    }

    #[test]
    fn published_note_projection_matches_the_store_materialization() {
        let stores = Stores::new();
        let segment = HirariAudioNoteSegment {
            start_seconds: 1.0,
            end_seconds: 2.0,
            detected_pitch_cents: 4400.0,
            pitch_offset_cents: 0.0,
            formant_offset_cents: 0.0,
        };
        assert!(unsafe {
            crate::vari_audio::hirari_region_vari_audio_store_upsert(
                stores.notes,
                segment,
                std::ptr::null(),
                0,
            )
        });
        let frozen = unsafe { crate::vari_audio::hirari_region_vari_audio_freeze(stores.notes) };
        assert!(!frozen.is_null());

        let region_name = "Vox";
        let region_path = "";
        let mut region = HirariLayoutRegion {
            id: 100,
            name: layout_string(region_name),
            path: layout_string(region_path),
            source_sample_rate: 48_000,
            note_store: stores.notes,
            note_frozen: std::ptr::null(),
            processing_store: stores.history,
            ..HirariLayoutRegion::default()
        };
        let other_name = "Audio 2";
        let mut tracks = [HirariLayoutTrack {
            regions: &region,
            region_count: 1,
            ..empty_track(1, other_name)
        }];
        let from_store = write_json(&HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        })
        .expect("layout serializes");
        region.note_frozen = frozen;
        tracks[0].regions = &region;
        let from_frozen = write_json(&HirariProjectLayoutSnapshot {
            tracks: tracks.as_ptr(),
            track_count: tracks.len(),
            ..HirariProjectLayoutSnapshot::default()
        })
        .expect("layout serializes");
        assert_eq!(from_store, from_frozen);
        assert!(from_frozen.contains(
            r#""audio_note_segments":[{"start":1.000000,"end":2.000000,"#,
        ));
    }
