//! Tests split out of native_project.rs.

    use super::*;
    use std::io::Write;

    #[test]
    fn inspects_version_28_header_and_crc() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&120.0f64.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        let checksum = crc32(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        let path = std::env::temp_dir().join(format!(
            "hirari-native-inspect-{}.hirari",
            std::process::id()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let result = inspect(&path).unwrap();
        assert_eq!(result.format, "hirari-native-binary");
        assert_eq!(result.version, 28);
        assert_eq!(result.sample_rate, 48_000);
        assert_eq!(result.track_count, Some(3));
        assert!(result.checksum_valid);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_torn_native_container() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&120.0f64.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let path = std::env::temp_dir().join(format!(
            "hirari-native-inspect-corrupt-{}.hirari",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let error = inspect(&path).unwrap_err().to_string();
        assert!(error.contains("checksum") || error.contains("trailer"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_native_project_sample_rate_below_audio_floor() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&120.0f64.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let checksum = crc32(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        let path = std::env::temp_dir().join(format!(
            "hirari-native-inspect-invalid-rate-{}.hirari",
            std::process::id()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert!(inspect(&path).is_err());
        let _ = std::fs::remove_file(path);
    }

    fn native_project_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hirari-native-contract-{}-{}.hirari",
            std::process::id(),
            suffix
        ))
    }

    // The retired C++ contract exercised these rejections through
    // `ProjectSerializer`. They belong to the Rust codec now, so assert the
    // same behavior at the entrypoints the engine actually calls.
    #[test]
    fn rejects_duplicate_tracks_orphan_regions_corruption_and_oversize() {
        use crate::ffi::{NativeRegionState, NativeTrackState};

        let track = NativeTrackState {
            id: 7,
            volume: 1.0,
            ..Default::default()
        };
        let region = NativeRegionState {
            id: 9001,
            trackId: 7,
            samplePosition: 0,
            sampleLength: 128,
            filePath: "contract.wav".into(),
            warpRatio: 1.0,
            loopCount: 1,
            ..Default::default()
        };
        let state = NativeProjectState {
            version: 28,
            sampleRate: 48_000,
            bpm: 120.0,
            tracks: vec![track.clone()],
            regions: vec![region],
            ..Default::default()
        };

        let path = native_project_path("roundtrip");
        let path_text = path.to_string_lossy().into_owned();
        assert!(native_project_save(&path_text, &state));
        let restored = native_project_load(&path_text);
        assert!(restored.valid);
        assert_eq!(restored.regions.len(), 1);
        assert_eq!(restored.regions[0].id, 9001);

        let mut duplicate = state.clone();
        duplicate.tracks = vec![track.clone(), track];
        assert!(native_project_save(&path_text, &duplicate));
        assert!(
            !native_project_load(&path_text).valid,
            "a container with duplicate track ids must be rejected on load"
        );

        let mut orphan = state.clone();
        orphan.regions[0].trackId = 999;
        assert!(native_project_save(&path_text, &orphan));
        assert!(
            !native_project_load(&path_text).valid,
            "a region whose track is absent must be rejected on load"
        );

        assert!(native_project_save(&path_text, &state));
        let mut bytes = std::fs::read(&path).expect("saved container is readable");
        bytes[12] ^= 0x5a;
        std::fs::write(&path, &bytes).expect("corrupted container is writable");
        assert!(
            !native_project_load(&path_text).valid,
            "a checksum mismatch must be rejected on load"
        );

        let oversized = native_project_path("oversized");
        let oversized_text = oversized.to_string_lossy().into_owned();
        let file = std::fs::File::create(&oversized).expect("oversized container is creatable");
        file.set_len(MAX_PROJECT_BYTES as u64 + 1)
            .expect("sparse container is sized");
        drop(file);
        assert!(
            !native_project_load(&oversized_text).valid,
            "an oversized container must be rejected before reading"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&oversized);
    }

    fn v42_document() -> NativeProjectState {
        use crate::ffi::{NativeRegionState, NativeRouteState, NativeSidechainState, NativeTrackState, };
        let audio = |id: u32| NativeTrackState {
            id,
            trackType: 0,
            volume: 1.0,
            ..Default::default()
        };
        let mut destination = audio(8);
        destination.pluginData = vec![0, 0, 0, 0];
        NativeProjectState {
            version: 42,
            sampleRate: 48_000,
            bpm: 120.0,
            tracks: vec![
                audio(7),
                destination,
                NativeTrackState {
                    id: 9,
                    trackType: 3,
                    volume: 1.0,
                    ..Default::default()
                },
            ],
            regions: vec![NativeRegionState {
                id: 9001,
                trackId: 7,
                samplePosition: 0,
                sampleLength: 128,
                sourceLength: 128,
                sourceSampleRate: 48_000,
                baseLength: 128,
                filePath: "contract.wav".into(),
                warpRatio: 1.0,
                loopCount: 1,
                ..Default::default()
            }],
            sidechains: vec![NativeSidechainState {
                sourceId: 7,
                destinationId: 8,
                pluginIndex: 0,
                tapPoint: 0,
            }],
            routes: vec![
                NativeRouteState {
                    sourceId: 7,
                    destinationId: 9,
                    gain: 1.0,
                    send: true,
                    preFader: false,
                },
                NativeRouteState {
                    sourceId: 8,
                    destinationId: 7,
                    gain: 1.0,
                    send: false,
                    preFader: false,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn rejects_cross_section_inconsistencies_at_load() {
        let state = v42_document();
        let path = native_project_path("cross-validation");
        let path_text = path.to_string_lossy().into_owned();
        assert!(native_project_save(&path_text, &state));
        let restored = native_project_load(&path_text);
        assert!(restored.valid, "the baseline document must hydrate");
        assert_eq!(restored.routes.len(), 2);
        assert_eq!(restored.sidechains.len(), 1);

        let assert_rejected = |candidate: NativeProjectState, what: &str| {
            assert!(native_project_save(&path_text, &candidate));
            assert!(
                !native_project_load(&path_text).valid,
                "a document with {what} must be rejected on load"
            );
        };

        let mut sentinel = v42_document();
        sentinel.regions.clear();
        sentinel.sidechains.clear();
        sentinel.routes.clear();
        let second_track = sentinel.tracks[1].clone();
        sentinel.tracks = vec![
            crate::ffi::NativeTrackState {
                id: u32::MAX,
                ..Default::default()
            },
            second_track,
        ];
        assert_rejected(sentinel, "the invalid track id sentinel");

        let mut odd_plugin_data = v42_document();
        odd_plugin_data.tracks[1].pluginData.push(0);
        assert_rejected(odd_plugin_data, "a partial plugin id word");

        let mut excess_states = v42_document();
        excess_states.tracks[1].pluginStates = vec![crate::ffi::NativeBlob::default(); 2];
        excess_states.tracks[1].pluginGuiStates = vec![crate::ffi::NativeBlob::default(); 2];
        assert_rejected(excess_states, "more plugin states than slots");

        let mut missing_sandbox_path = v42_document();
        missing_sandbox_path.tracks[1].pluginData = vec![0xff, 0xff, 0xff, 0xff];
        assert_rejected(missing_sandbox_path, "a sandbox marker without a path");

        let mut unknown_route = v42_document();
        unknown_route.routes[1].destinationId = 999;
        assert_rejected(unknown_route, "a route to an absent track");

        let mut out_of_range_node = v42_document();
        out_of_range_node.routes[1].destinationId = 128;
        assert_rejected(out_of_range_node, "a route node at the graph limit");

        let mut send_to_audio = v42_document();
        send_to_audio.routes[0].sourceId = 8;
        send_to_audio.routes[0].destinationId = 7;
        assert_rejected(send_to_audio, "a send routed to a non-bus track");

        let mut unknown_sidechain = v42_document();
        unknown_sidechain.sidechains[0].sourceId = 999;
        assert_rejected(unknown_sidechain, "a sidechain from an absent track");

        let mut cyclic_sidechain = v42_document();
        cyclic_sidechain.sidechains.push(crate::ffi::NativeSidechainState {
            sourceId: 8,
            destinationId: 7,
            pluginIndex: 0,
            tapPoint: 0,
        });
        assert_rejected(cyclic_sidechain, "a cyclic sidechain topology");

        let _ = std::fs::remove_file(&path);
    }
