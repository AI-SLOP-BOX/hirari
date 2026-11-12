//! Tests split out of project_archive.rs.

    use super::*;
    use crate::persistence::ProjectMetadata;
    use crate::project::{ProjectDocument, ProjectRegion};

    fn project(path: &str) -> ProjectDocument {
        ProjectDocument {
            schema_version: 1,
            contract_version: 1,
            project_id: "p".into(),
            metadata: ProjectMetadata::default(),
            sample_rate: 48_000.0,
            master_gain: 1.0,
            cycle_start_sample: 0,
            cycle_end_sample: 0,
            cycle_enabled: false,
            metronome_enabled: false,
            tracks: vec![],
            aux_track_ids: vec![],
            regions: vec![ProjectRegion {
                id: 1,
                track_id: 1,
                name: "a".into(),
                path: path.into(),
                start: 0,
                length: 1,
                source_length: 1,
                source_sample_rate: 48_000,
                source_offset: 0,
                base_source_offset: 0,
                base_length: 1,
                muted: false,
                clip_gain: 1.0,
                fade_in_samples: 0,
                fade_out_samples: 0,
                warp_ratio: 1.0,
                pitch_preserve_warp: false,
                pitch_semitones: 0.0,
                reverse: false,
                loop_count: 1,
                sync_group: 0,
                processing_history: Vec::new(),
            }],
            plugin_instances: vec![],
            midi_learn_mappings: vec![],
            midi_notes: vec![],
            chord_track: vec![],
            midi_events: vec![],
            tempo_events: vec![],
            time_signature_events: vec![],
            macro_mappings: vec![],
            warp_markers: vec![],
            render_targets: vec![],
            freeze_artifacts: vec![],
            sidechain_routes: vec![],
            feedback_routes: vec![],
            audio_routes: vec![],
            audio_input_assignments: vec![],
            step_sequencer_patterns: vec![],
            openutau_vocals: vec![],
            comp_takes: vec![],
            comp_segments: vec![],
            track_stacks: vec![],
            markers: vec![],
            vca_groups: vec![],
            hardware_inserts: vec![],
            control_room: None,
        }
    }

    #[test]
    fn plans_missing_and_existing_dependencies() {
        let root = std::env::temp_dir().join(format!("hirari-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let manifest =
            ProjectArchive::plan(root.join("song.hirari"), &project("take.wav")).unwrap();
        assert!(manifest.validate());
        assert_eq!(manifest.entries.len(), 1);
        assert!(manifest.missing.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn creates_and_verifies_package() {
        let root = std::env::temp_dir().join(format!("hirari-package-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.hirari-package");
        let manifest = ProjectArchive::create(
            root.join("song.hirari"),
            b"{}",
            &project("take.wav"),
            &package,
        )
        .unwrap();
        assert_eq!(
            ProjectArchive::verify(&package).unwrap(),
            Vec::<String>::new()
        );
        assert!(manifest.validate());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gives_same_named_dependencies_unique_package_paths() {
        let root =
            std::env::temp_dir().join(format!("hirari-archive-collision-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::write(root.join("one/take.wav"), b"same audio").unwrap();
        fs::write(root.join("two/take.wav"), b"same audio").unwrap();
        let mut document = project("one/take.wav");
        document.regions.push(ProjectRegion {
            id: 2,
            path: "two/take.wav".into(),
            ..document.regions[0].clone()
        });

        let manifest = ProjectArchive::plan(root.join("song.hirari"), &document).unwrap();

        assert!(manifest.validate());
        assert_eq!(manifest.entries.len(), 2);
        assert_ne!(
            manifest.entries[0].package_path,
            manifest.entries[1].package_path
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_rejects_traversal_and_duplicate_missing_paths() {
        let mut manifest = ArchiveManifest {
            schema_version: ARCHIVE_SCHEMA_VERSION,
            project_file: "song.hirari".into(),
            entries: vec![ArchiveEntry {
                source: "take.wav".into(),
                package_path: "Media/../take.wav".into(),
                bytes: 1,
                sha256: "0".repeat(64),
                exists: true,
            }],
            missing: vec![],
            total_bytes: 1,
        };
        assert!(!manifest.validate());
        manifest.entries.clear();
        manifest.total_bytes = 0;
        manifest.missing = vec!["lost.wav".into(), "lost.wav".into()];
        assert!(!manifest.validate());
    }

    #[test]
    fn verify_reports_packaged_file_size_changes() {
        let root =
            std::env::temp_dir().join(format!("hirari-package-tamper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.hirari-package");
        let manifest = ProjectArchive::create(
            root.join("song.hirari"),
            b"{}",
            &project("take.wav"),
            &package,
        )
        .unwrap();
        fs::write(
            package.join(&manifest.entries[0].package_path),
            b"longer audio",
        )
        .unwrap();

        let errors = ProjectArchive::verify(&package).unwrap();

        assert_eq!(
            errors,
            vec![format!(
                "size mismatch: {}",
                manifest.entries[0].package_path
            )]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_manifest_is_rejected_before_allocation() {
        let root =
            std::env::temp_dir().join(format!("hirari-oversized-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let package = root.join("song.hirari-package");
        fs::create_dir_all(&package).unwrap();
        let file = File::create(package.join("manifest.json")).unwrap();
        file.set_len(MAX_MANIFEST_BYTES + 1).unwrap();

        let result = ProjectArchive::read_manifest(&package);

        assert!(result.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restores_verified_package_atomically() {
        let root = std::env::temp_dir().join(format!("hirari-restore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("take.wav"), b"audio").unwrap();
        let package = root.join("song.hirari-package");
        ProjectArchive::create(
            root.join("song.hirari"),
            b"{}",
            &project("take.wav"),
            &package,
        )
        .unwrap();
        let destination = root.join("restored");
        let restored = ProjectArchive::restore(&package, &destination).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(
            fs::read(destination.join("Media/take.wav")).unwrap(),
            b"audio"
        );
        let _ = fs::remove_dir_all(root);
    }
