use super::*;
use super::{content_hash, fingerprint_asset_file, unique_nonce, validate_commit_id, validate_ref_name};

mod tests {
    use super::*;
    use crate::project::ProjectDocument;
    use crate::project_contracts::OpenUtauVocalContract;

    #[test]
    fn commit_branch_checkout_and_load_are_content_addressed() {
        let root = std::env::temp_dir().join(format!("aura-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectHistoryStore::open(&root).unwrap();
        let project = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, "[]").unwrap();
        let first = store.commit(&project, "initial").unwrap();
        let branch = store.create_branch("experiment", None).unwrap();
        assert_eq!(branch.commit_id, first.commit_id);
        assert_eq!(store.checkout("experiment").unwrap().branch, "experiment");
        assert_eq!(store.load_commit(&first.commit_id).unwrap(), project);
        let _tag = store.tag("v1", None).unwrap();
        assert_eq!(
            store.status().unwrap().snapshot_hash,
            Some(first.snapshot_hash)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_projects_keep_history_next_to_the_file() {
        let parent = std::env::temp_dir().join(format!("aura-history-file-{}", unique_nonce()));
        std::fs::create_dir_all(&parent).unwrap();
        let project_file = parent.join("Song.aura");
        std::fs::write(&project_file, b"placeholder").unwrap();
        let store = ProjectHistoryStore::open(&project_file).unwrap();
        assert!(parent.join(".aura/history").is_dir());
        assert!(!project_file.join(".aura/history").exists());
        let _ = std::fs::remove_dir_all(parent);
        drop(store);
    }

    #[test]
    fn commit_file_reads_working_tree_inside_history_transaction() {
        let root =
            std::env::temp_dir().join(format!("aura-history-commit-file-{}", unique_nonce()));
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let project = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, "[]").unwrap();
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let commit = store.commit_file(&project_file, "initial").unwrap();
        assert_eq!(store.load_commit(&commit.commit_id).unwrap(), project);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extension_bearing_missing_paths_require_explicit_project_mode() {
        let root = std::env::temp_dir().join(format!("aura-history-ambiguous-{}", unique_nonce()));
        let missing = root.join("Song.aura");
        assert!(ProjectHistoryStore::open(&missing).is_err());
        let store = ProjectHistoryStore::open_project_directory(&missing).unwrap();
        assert!(missing.join(".aura/history").is_dir());
        let _ = std::fs::remove_dir_all(root);
        drop(store);
    }

    #[test]
    fn commit_and_ref_names_are_strictly_validated() {
        assert!(validate_commit_id(&"a".repeat(64)).is_ok());
        assert!(validate_commit_id("short").is_err());
        assert!(validate_commit_id(&"g".repeat(64)).is_err());
        assert!(validate_ref_name("main-2").is_ok());
        assert!(validate_ref_name("../escape").is_err());
        assert!(validate_ref_name("bad name").is_err());
    }

    #[test]
    fn asset_fingerprint_hashes_large_files_without_changing_content_identity() {
        let path = std::env::temp_dir().join(format!("aura-history-asset-{}", unique_nonce()));
        let bytes = vec![0x5au8; 1024 * 1024 + 17];
        std::fs::write(&path, &bytes).unwrap();
        let (size, hash, _, _, _) = fingerprint_asset_file(&path).unwrap();
        assert_eq!(size, bytes.len() as u64);
        assert_eq!(hash, content_hash(&bytes));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn openutau_source_and_rendered_assets_participate_in_commit_identity() {
        let root = std::env::temp_dir().join(format!("aura-history-openutau-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let rendered = root.join("voice.wav");
        std::fs::write(&source, b"note C4").unwrap();
        std::fs::write(&rendered, b"render-a").unwrap();

        let store = ProjectHistoryStore::open_project_directory(&root).unwrap();
        let mut project =
            ProjectDocument::from_layout_json("Vocal", 120.0, 48_000.0, "[]").unwrap();
        project.openutau_vocals.push(OpenUtauVocalContract {
            source_path: source.to_string_lossy().into_owned(),
            rendered_audio_path: rendered.to_string_lossy().into_owned(),
            singer: "test-singer".into(),
            source_generation: 1,
            source_hash: String::new(),
            rendered_audio_hash: String::new(),
            rendered_audio_bytes: 0,
            rendered_sample_rate: None,
            rendered_channels: None,
            rendered_frames: None,
            source_note_count: 1,
            source_singers: vec!["test-singer".into()],
            tuning: Default::default(),
        });
        let project_file = root.join("Song.aura");
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        let first = store.commit(&project, "vocal-a").unwrap();

        let verified = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(verified["ok"], true);
        assert_eq!(verified["asset_manifest_matches_head"], true);

        std::fs::write(&source, b"note D4").unwrap();
        let changed_asset = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(changed_asset["ok"], false);
        assert_eq!(changed_asset["working_tree_matches_head"], true);
        assert_eq!(changed_asset["asset_manifest_matches_head"], false);
        let second = store.commit(&project, "vocal-b").unwrap();
        assert_ne!(first.asset_manifest_hash, second.asset_manifest_hash);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn status_for_branch_does_not_publish_a_checkout() {
        let root = std::env::temp_dir().join(format!("aura-history-preview-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectHistoryStore::open(&root).unwrap();
        let project = ProjectDocument::from_layout_json("Preview", 120.0, 48_000.0, "[]").unwrap();
        let initial = store.commit(&project, "initial").unwrap();
        store.create_branch("experiment", None).unwrap();

        let preview = store.status_for_branch("experiment").unwrap();
        assert_eq!(preview.branch, "experiment");
        assert_eq!(preview.head, Some(initial.commit_id));
        assert_eq!(store.status().unwrap().branch, "main");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn verify_working_tree_detects_unsaved_or_wrong_head_state() {
        let root = std::env::temp_dir().join(format!("aura-history-verify-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let project = ProjectDocument::from_layout_json("Verify", 120.0, 48_000.0, "[]").unwrap();
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        store.commit(&project, "initial").unwrap();

        let verified = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(verified["ok"], true);
        assert_eq!(verified["working_tree_matches_head"], true);

        let mut unsaved = project.clone();
        unsaved.metadata.name = "Unsaved edit".into();
        unsaved.save_atomic(project_file.to_str().unwrap()).unwrap();
        let diverged = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(diverged["ok"], false);
        assert_eq!(diverged["identity_matches"], true);
        assert_eq!(diverged["working_tree_matches_head"], false);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn diff_reports_entity_level_track_and_plugin_changes() {
        let root = std::env::temp_dir().join(format!("aura-history-diff-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let layout = r#"[{"id":7,"name":"Vocal","plugin_types":[0],"plugin_state_hex":["00"],"regions":[]}]"#;
        let first = ProjectDocument::from_layout_json("Diff", 120.0, 48_000.0, layout).unwrap();
        let first_commit = store.commit(&first, "initial").unwrap();

        let mut second = first.clone();
        second.tracks[0].name = "Lead Vocal".to_owned();
        second.tracks[0].volume = 0.75;
        second.tracks[0].track_delay_samples = 2400;
        second.plugin_instances[0].bypassed = true;
        second
            .freeze_artifacts
            .push(crate::project_contracts::FreezeArtifactContract {
                track_id: 7,
                project_generation: 2,
                audio_generation: 3,
                total_samples: 48_000,
                sample_rate: 48_000,
                path: "freeze/vocal.wav".to_owned(),
                content_checksum: 1,
            });
        let second_commit = store.commit(&second, "vocal edit").unwrap();
        let diff = store
            .diff_commits(&first_commit.commit_id, &second_commit.commit_id)
            .unwrap();

        assert!(diff.changed_sections.contains(&"tracks".to_owned()));
        assert!(diff
            .changed_sections
            .contains(&"plugin_instances".to_owned()));
        assert!(diff
            .changed_sections
            .contains(&"freeze_artifacts".to_owned()));
        assert!(diff.changes.iter().any(|change| {
            change.section == "tracks"
                && change.entity_id.as_deref() == Some("7")
                && change.operation == "changed"
                && change.fields.iter().any(|field| {
                    field.field == "track_delay_samples"
                        && field.before == Some(serde_json::json!(0))
                        && field.after == Some(serde_json::json!(2400))
                })
        }));
        assert!(diff.changes.iter().any(|change| {
            change.section == "plugin_instances"
                && change.entity_id.as_deref() == Some("track:7:slot:0")
                && change.operation == "changed"
        }));
        assert!(diff.changes.iter().any(|change| {
            change.section == "freeze_artifacts"
                && change.entity_id.as_deref() == Some("7")
                && change.operation == "added"
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn checkout_and_restore_publishes_head_only_after_project_restore() {
        let root = std::env::temp_dir().join(format!("aura-history-restore-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let initial = ProjectDocument::from_layout_json("Initial", 120.0, 48_000.0, "[]").unwrap();
        initial.save_atomic(project_file.to_str().unwrap()).unwrap();
        let first = store.commit(&initial, "initial").unwrap();

        let mut changed = initial.clone();
        changed.metadata.name = "Changed".into();
        changed.metadata.bpm = 128.0;
        changed.save_atomic(project_file.to_str().unwrap()).unwrap();
        let second = store.commit(&changed, "changed").unwrap();
        assert_ne!(first.commit_id, second.commit_id);

        store.checkout_and_restore(&project_file, "main").unwrap();
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            changed
        );

        store
            .create_branch("initial-state", Some(&first.commit_id))
            .unwrap();
        let status = store
            .checkout_and_restore(&project_file, "initial-state")
            .unwrap();
        assert_eq!(status.branch, "initial-state");
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            initial
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_rejects_a_different_project_at_the_same_path() {
        let root = std::env::temp_dir().join(format!("aura-history-identity-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let project = ProjectDocument::from_layout_json("Original", 120.0, 48_000.0, "[]").unwrap();
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        store.commit(&project, "initial").unwrap();

        let replacement =
            ProjectDocument::from_layout_json("Replacement", 120.0, 48_000.0, "[]").unwrap();
        replacement
            .save_atomic(project_file.to_str().unwrap())
            .unwrap();
        let before = std::fs::read(&project_file).unwrap();
        assert!(store.checkout_and_restore(&project_file, "main").is_err());
        assert_eq!(std::fs::read(&project_file).unwrap(), before);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revert_and_cherry_pick_publish_new_commits_with_matching_working_tree() {
        let root = std::env::temp_dir().join(format!("aura-history-rewrite-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();

        let initial = ProjectDocument::from_layout_json("Initial", 120.0, 48_000.0, "[]").unwrap();
        initial.save_atomic(project_file.to_str().unwrap()).unwrap();
        let first = store.commit(&initial, "initial").unwrap();
        let mut changed = initial.clone();
        changed.metadata.name = "Changed".into();
        changed.metadata.bpm = 128.0;
        changed.openutau_vocals.push(OpenUtauVocalContract {
            source_path: "voice.ustx".into(),
            rendered_audio_path: "voice.wav".into(),
            singer: "Teto".into(),
            source_generation: 1,
            source_hash: "source".into(),
            rendered_audio_hash: "rendered".into(),
            rendered_audio_bytes: 4,
            rendered_sample_rate: Some(48_000),
            rendered_channels: Some(2),
            rendered_frames: Some(2),
            source_note_count: 1,
            source_singers: vec!["Teto".into()],
            tuning: Default::default(),
        });
        changed.chord_track.push(crate::harmonic::ChordEvent {
            tick: 0,
            root: 60,
            intervals: vec![0, 4, 7],
            name: "C".into(),
        });
        changed.save_atomic(project_file.to_str().unwrap()).unwrap();
        let second = store.commit(&changed, "changed").unwrap();

        let reverted = store
            .revert_and_restore(&project_file, &first.commit_id)
            .unwrap();
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            initial
        );
        assert_eq!(
            store.status().unwrap().head,
            Some(reverted.commit_id.clone())
        );
        assert_eq!(reverted.parent, Some(second.commit_id.clone()));

        let merged = store
            .cherry_pick_and_restore(&project_file, &reverted.commit_id, &["metadata".into()])
            .unwrap();
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            initial
        );
        assert_eq!(store.status().unwrap().head, Some(merged.commit_id));

        let openutau_only = store
            .cherry_pick(&initial, &second.commit_id, &["openutau_vocals".into()])
            .unwrap();
        assert_eq!(openutau_only.openutau_vocals, changed.openutau_vocals);
        let chord_only = store
            .cherry_pick(&initial, &second.commit_id, &["chord_track".into()])
            .unwrap();
        assert_eq!(chord_only.chord_track, changed.chord_track);

        let _ = std::fs::remove_dir_all(root);
    }
}
