#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_checksum_uses_sha256_and_reads_legacy_fnv() {
        let data = b"aura-persistence-checksum";
        let modern = PersistenceOrchestrator::calculate_checksum(data);
        let legacy = PersistenceOrchestrator::legacy_fnv1a_checksum(data);
        assert_ne!(modern, legacy);
        let persistence = PersistenceOrchestrator::new(1);
        assert!(persistence.audit_persistence(data, modern));
        assert!(persistence.audit_persistence(data, legacy));
        assert!(!persistence.audit_persistence(b"mutated", modern));
    }

    #[test]
    fn atomic_save_replaces_file_without_leaving_temp_file() {
        let path = std::env::temp_dir().join(format!(
            "aura-persistence-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(2);
        let info = persistence
            .atomic_save(&path_string, br#"{"version":1}"#)
            .unwrap();

        assert!(path.is_file());
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"version":1}"#);
        assert!(persistence.audit_persistence(br#"{"version":1}"#, info.checksum));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn concurrent_save_lock_preserves_existing_project() {
        let path = std::env::temp_dir().join(format!(
            "aura-save-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"stable").unwrap();
        let lock_path = PathBuf::from(format!("{}.save.lock", path.display()));
        File::create(&lock_path).unwrap();
        let mut persistence = PersistenceOrchestrator::new(2);
        assert!(persistence
            .atomic_save(path.to_str().unwrap(), b"new")
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"stable");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path);
    }

    #[test]
    fn save_lock_drop_does_not_remove_a_replaced_owner_lock() {
        let path = std::env::temp_dir().join(format!(
            "aura-replaced-lock-{}-{}.lock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "pid=1 token=original\n").unwrap();
        let guard = SaveLock {
            path: path.clone(),
            owner_token: "original".to_owned(),
        };
        // Simulate stale-owner recovery publishing a new lock before the
        // original guard is dropped.
        std::fs::write(&path, "pid=2 token=replacement\n").unwrap();
        drop(guard);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "pid=2 token=replacement\n"
        );
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn malformed_save_lock_is_never_reclaimed_optimistically() {
        let path = std::env::temp_dir().join(format!(
            "aura-malformed-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"stable").unwrap();
        let lock_path = PathBuf::from(format!("{}.save.lock", path.display()));
        // A partially-written or foreign lock must remain a hard conflict.
        std::fs::write(
            &lock_path,
            "pid=2147483647 token=not-a-valid-owner-record\n",
        )
        .unwrap();

        let mut persistence = PersistenceOrchestrator::new(1);
        assert!(persistence
            .atomic_save(path.to_str().unwrap(), b"new")
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"stable");
        assert!(lock_path.exists());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path);
    }

    #[cfg(unix)]
    #[test]
    fn dead_save_lock_is_reclaimed_without_overwriting_project() {
        let path = std::env::temp_dir().join(format!(
            "aura-stale-lock-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"stable").unwrap();
        let lock_path = PathBuf::from(format!("{}.save.lock", path.display()));
        std::fs::write(
            &lock_path,
            "pid=2147483647 created_unix_seconds=0 token=dead-face-1\n",
        )
        .unwrap();

        let mut persistence = PersistenceOrchestrator::new(1);
        persistence
            .atomic_save(path.to_str().unwrap(), b"updated")
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"updated");
        assert!(!lock_path.exists());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(lock_path);
    }

    #[test]
    fn save_project_uses_atomic_serialization() {
        let path = std::env::temp_dir().join(format!(
            "aura-project-{}-{}.aura",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let metadata = ProjectMetadata {
            name: "Preview".into(),
            version: 1,
            bpm: 120.0,
            tracks_count: 2,
            key_root: 0,
            scale_type: 0,
        };
        SovereignPersistence::save_project(path.to_str().unwrap(), &metadata).unwrap();
        let loaded: ProjectMetadata =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded.name, "Preview");
        assert_eq!(loaded.tracks_count, 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_project_json_is_rejected_without_panicking() {
        let path = std::env::temp_dir().join(format!(
            "aura-malformed-project-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for length in 0..2048usize {
            let mut state = 0x243f_6a88_u32 ^ length as u32;
            let mut bytes = vec![0u8; length];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            std::fs::write(&path, &bytes).unwrap();
            let result: Result<ProjectMetadata> =
                SovereignPersistence::load_json(path.to_str().unwrap());
            assert!(
                result.is_err(),
                "random bytes parsed as project JSON at {length}"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn atomic_save_rotates_real_recovery_generations() {
        let path = std::env::temp_dir().join(format!(
            "aura-rotation-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(2);
        persistence.atomic_save(&path_string, b"one").unwrap();
        persistence.atomic_save(&path_string, b"two").unwrap();
        persistence.atomic_save(&path_string, b"three").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"three");
        let candidates = PersistenceOrchestrator::recovery_candidates(&path_string).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(std::fs::read(&candidates[0].path).unwrap(), b"two");
        assert_eq!(std::fs::read(&candidates[1].path).unwrap(), b"one");

        let _ = std::fs::remove_file(&path);
        for candidate in candidates {
            let _ = std::fs::remove_file(candidate.path);
        }
    }

    #[test]
    fn repeated_atomic_save_load_keeps_latest_generation_and_no_temp_files() {
        let path = std::env::temp_dir().join(format!(
            "aura-persistence-soak-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(4);
        for generation in 1..=256u32 {
            let payload = format!(r#"{{"generation":{generation},"tracks":[]}}"#);
            persistence
                .atomic_save(&path_string, payload.as_bytes())
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), payload.as_bytes());
        }
        let candidates = PersistenceOrchestrator::recovery_candidates(&path_string).unwrap();
        assert_eq!(candidates.len(), 4);
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let temp_prefix = format!("{}.tmp-", path.file_name().unwrap().to_string_lossy());
        for entry in std::fs::read_dir(parent).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                !name.starts_with(&temp_prefix),
                "temporary save leaked: {name}"
            );
        }
        for candidate in candidates {
            let _ = std::fs::remove_file(candidate.path);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn recovery_candidate_checksum_matches_persistence_audit_checksum() {
        let path = std::env::temp_dir().join(format!(
            "aura-checksum-{}-{}.aura",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut persistence = PersistenceOrchestrator::new(1);
        persistence.atomic_save(&path_string, b"first").unwrap();
        persistence.atomic_save(&path_string, b"second").unwrap();

        let candidates = PersistenceOrchestrator::recovery_candidates(&path_string).unwrap();
        let candidate = candidates.first().expect("one recovery generation");
        let bytes = std::fs::read(&candidate.path).unwrap();
        assert_eq!(
            candidate.checksum,
            PersistenceOrchestrator::calculate_checksum(&bytes)
        );
        assert!(persistence.audit_persistence(&bytes, candidate.checksum));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(candidate.path.clone());
    }

    #[test]
    fn failed_atomic_save_does_not_replace_an_existing_directory_target() {
        let root = std::env::temp_dir().join(format!(
            "aura-atomic-failure-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("project.aura");
        std::fs::create_dir_all(&target).unwrap();

        let mut persistence = PersistenceOrchestrator::new(2);
        assert!(persistence
            .atomic_save(target.to_str().unwrap(), b"new project")
            .is_err());
        assert!(target.is_dir());
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_save_reclaims_stale_reserved_temporary_files() {
        let root = std::env::temp_dir().join(format!(
            "aura-stale-temp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("project.aura");
        let stale = root.join(format!(
            // Use a deliberately non-live owner. A current PID is not stale
            // and must now be preserved by cleanup_stale_save_temps().
            "project.aura.tmp-{}-{}-deadbeef",
            2_000_000_000u32,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let foreign = root.join("project.aura.tmp-legacy-format");
        let live_owner_temp = root.join(format!(
            "project.aura.tmp-{}-{}-liveowner",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&stale, b"partial").unwrap();
        std::fs::write(&foreign, b"leave this file alone").unwrap();
        std::fs::write(&live_owner_temp, b"active writer temp").unwrap();

        let mut persistence = PersistenceOrchestrator::new(1);
        persistence
            .atomic_save(path.to_str().unwrap(), b"complete")
            .unwrap();
        assert!(!stale.exists());
        assert!(foreign.exists());
        assert!(live_owner_temp.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");

        let _ = std::fs::remove_file(live_owner_temp);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_save_supports_unicode_project_paths() {
        let root = std::env::temp_dir().join(format!(
            "aura-保存-🎵-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("プロジェクト.aura");
        let mut persistence = PersistenceOrchestrator::new(1);
        persistence
            .atomic_save(path.to_str().unwrap(), br#"{"tracks":[]}"#)
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"tracks":[]}"#);

        let _ = std::fs::remove_dir_all(root);
    }
}
