impl AuraCore {
    pub fn save_project(&self, path: &str) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        if path.trim().is_empty() {
            return false;
        }
        // Native mutations use their own engine lock and may arrive while
        // sidecars are being serialized. Reject a stale publication instead
        // of letting an older snapshot overwrite a newer edit.
        let save_generation = self.project_generation();
        let had_primary = std::path::Path::new(path).is_file();
        let sidecars = [
            Self::comping_sidecar_path(path),
            Self::midi_sidecar_path(path),
        ];
        let mut sidecar_backups = Vec::with_capacity(sidecars.len());
        for sidecar in &sidecars {
            let backup = std::path::PathBuf::from(format!("{}.bak.1", sidecar.display()));
            let existed = sidecar.is_file();
            if existed && !copy_file_atomic_replace(sidecar, &backup) {
                return false;
            }
            sidecar_backups.push((backup, existed));
        }
        // The native serializer predates the Rust persistence layer. Preserve
        // the current primary before delegating to it so the regular Save
        // button gets the same recovery guarantee as save_project_v2.
        if crate::persistence::PersistenceOrchestrator::new(10)
            .rotate_existing_backup(path)
            .is_err()
        {
            return false;
        }
        let saved = self.engine.as_ref().is_some_and(|e| e.save_project(path))
            && self.save_comping_sidecar(path)
            && self.save_midi_sidecar(path);
        let verified = saved
            && self.project_generation() == save_generation
            && std::fs::metadata(path)
                .map(|meta| meta.is_file() && meta.len() > 0)
                .unwrap_or(false)
            && std::fs::read_to_string(Self::comping_sidecar_path(path))
                .ok()
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
                .is_some()
            && std::fs::read_to_string(Self::midi_sidecar_path(path))
                .ok()
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
                .is_some();
        if verified {
            return true;
        }

        // A native writer or either sidecar can fail after the primary has
        // already been replaced. Restore the previous generation so a failed
        // save is observationally atomic for both disk and the live engine.
        if had_primary {
            let backup = format!("{path}.bak.1");
            if copy_file_atomic_replace(std::path::Path::new(&backup), std::path::Path::new(path)) {
                let _ = self
                    .engine
                    .as_ref()
                    .is_some_and(|e| e.load_project(&backup));
            }
        } else {
            let _ = std::fs::remove_file(path);
        }
        for (backup, existed) in sidecar_backups {
            let sidecar = backup
                .to_string_lossy()
                .strip_suffix(".bak.1")
                .map(std::path::PathBuf::from);
            if let Some(sidecar) = sidecar {
                if existed {
                    let _ = copy_file_atomic_replace(&backup, &sidecar);
                } else {
                    let _ = std::fs::remove_file(sidecar);
                }
            }
        }
        false
    }

    /// Split an audio region at both sides of each sufficiently long silent run.
    /// Boundaries are applied from right to left so the original region remains a
    /// stable left-hand target while native IDs are allocated for the right side.
    pub fn split_region_at_silence(
        &self,
        tid: u32,
        rid: u32,
        samples: &[f32],
        threshold: f32,
        min_length: usize,
    ) -> u32 {
        if tid == 0
            || rid == 0
            || !threshold.is_finite()
            || !(0.0..=1.0).contains(&threshold)
            || min_length == 0
        {
            return 0;
        }
        let Ok(layout) = serde_json::from_str::<serde_json::Value>(&self.get_project_layout_json())
        else {
            return 0;
        };
        let Some(region) = layout.as_array().and_then(|tracks| {
            tracks.iter().find_map(|track| {
                track
                    .get("regions")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|regions| {
                        regions.iter().find(|region| {
                            region.get("id").and_then(serde_json::Value::as_u64) == Some(rid as u64)
                        })
                    })
            })
        }) else {
            return 0;
        };
        let region_start = region
            .get("start")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let region_length = region
            .get("length")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(samples.len() as u64);
        let mut boundaries =
            crate::silence_detector::detect_silence(samples, threshold, min_length)
                .into_iter()
                .flat_map(|range| {
                    let start = range.start as u64;
                    let end = start.saturating_add(range.length as u64);
                    [start, end]
                })
                .filter(|offset| *offset > 0 && *offset < region_length)
                .map(|offset| region_start.saturating_add(offset))
                .collect::<Vec<_>>();
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut applied = 0;
        for sample in boundaries.into_iter().rev() {
            if self.split_region(tid, rid, self.samples_to_beats(sample)) {
                applied += 1;
            }
        }
        applied
    }

    /// Splits a region and applies a symmetric non-destructive crossfade at
    /// the new boundary by resolving the native-created right region ID.
    pub fn split_region_with_auto_crossfade(
        &self,
        tid: u32,
        rid: u32,
        beat: f64,
        ratio: f32,
    ) -> bool {
        if tid == 0
            || rid == 0
            || !beat.is_finite()
            || beat <= 0.0
            || !ratio.is_finite()
            || !(0.0..=1.0).contains(&ratio)
        {
            return false;
        }
        let split_samples = self.beats_to_samples(beat);
        if !self.split_region(tid, rid, beat) {
            return false;
        }
        let Ok(layout) = serde_json::from_str::<serde_json::Value>(&self.get_project_layout_json())
        else {
            return false;
        };
        let Some(track) = layout.as_array().and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(tid as u64)
            })
        }) else {
            return false;
        };
        let Some(regions) = track.get("regions").and_then(serde_json::Value::as_array) else {
            return false;
        };
        let right_id = regions
            .iter()
            .find(|region| {
                region.get("start").and_then(serde_json::Value::as_u64) == Some(split_samples)
            })
            .and_then(|region| region.get("id").and_then(serde_json::Value::as_u64))
            .unwrap_or(0) as u32;
        right_id != 0
            && self.set_region_fades(tid, right_id, ratio, 0.0)
            && self.set_region_fades(tid, rid, 0.0, ratio)
    }
    /// Structured counterpart to the legacy bool API. This keeps existing UI
    /// callers compatible while giving CLI/automation callers a stable error
    /// code instead of an information-losing false value.
    pub fn save_project_diagnostic_json(&self, path: &str) -> String {
        let result = if path.trim().is_empty() {
            Err(crate::bridge_error::BridgeError::new(
                "invalid_path",
                "project path must not be empty",
            ))
        } else if self.save_project(path) {
            Ok(serde_json::json!({
                "ok": true,
                "path": path,
                "generation": self.project_generation(),
            }))
        } else {
            Err(crate::bridge_error::BridgeError::new(
                "project_save_failed",
                "project or sidecar publication failed",
            )
            .retryable(true)
            .at_generation(self.project_generation()))
        };
        match result {
            Ok(value) => value.to_string(),
            Err(error) => serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"serialization_error\"}".to_owned()),
        }
    }
    pub fn load_project(&self, path: &str) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        if path.trim().is_empty()
            || !std::fs::metadata(path)
                .map(|meta| meta.is_file() && meta.len() > 0)
                .unwrap_or(false)
        {
            return false;
        }
        // Validate optional sidecars before mutating the native graph. Missing
        // sidecars remain backward-compatible and mean an empty collection;
        // malformed sidecars must never leave the project half-loaded.
        for sidecar in [
            Self::comping_sidecar_path(path),
            Self::midi_sidecar_path(path),
        ] {
            if !sidecar.is_file() {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&sidecar) else {
                return false;
            };
            if serde_json::from_str::<serde_json::Value>(&contents).is_err() {
                return false;
            }
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let rollback_path = std::env::temp_dir().join(format!(
            "aura-load-rollback-{}-{}.native",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        let rollback_path_text = rollback_path.to_string_lossy().into_owned();
        if !engine.save_project(&rollback_path_text) {
            return false;
        }
        let previous_comping = self.comping_snapshot_json();
        let previous_midi = self.midi_events_json();
        let loaded = engine.load_project(path)
            && self.load_comping_sidecar(path)
            && self.load_midi_sidecar(path);
        if loaded {
            if let Ok(mut history) = self.midi_lyric_history.lock() {
                history.clear();
            }
            let _ = std::fs::remove_file(&rollback_path);
            return true;
        }

        // A sidecar failure must not leave the native graph or either
        // auxiliary state store half-hydrated. Restore all three snapshots
        // before exposing the failed load to callers.
        let native_restored = engine.load_project(&rollback_path_text);
        let comping_restored = self.restore_comping_snapshot_json(&previous_comping);
        let midi_restored = self.set_midi_events_json(&previous_midi);
        let _ = std::fs::remove_file(&rollback_path);
        let _rollback_succeeded = native_restored && comping_restored && midi_restored;
        false
    }

    pub fn load_project_diagnostic_json(&self, path: &str) -> String {
        let result = if path.trim().is_empty() {
            Err(crate::bridge_error::BridgeError::new(
                "invalid_path",
                "project path must not be empty",
            ))
        } else if !std::path::Path::new(path).is_file() {
            Err(crate::bridge_error::BridgeError::new(
                "project_not_found",
                "project file does not exist",
            ))
        } else if self.load_project(path) {
            Ok(serde_json::json!({
                "ok": true,
                "path": path,
                "generation": self.project_generation(),
            }))
        } else {
            Err(crate::bridge_error::BridgeError::new(
                "project_load_failed",
                "project or sidecar hydration failed",
            )
            .retryable(false)
            .at_generation(self.project_generation()))
        };
        match result {
            Ok(value) => value.to_string(),
            Err(error) => serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"serialization_error\"}".to_owned()),
        }
    }

    /// Returns valid on-disk recovery generations as JSON so the UI can show
    /// recoverable snapshots without duplicating filesystem scanning logic.
    pub fn recovery_candidates_json(&self, path: &str) -> String {
        if path.trim().is_empty() {
            return "[]".to_owned();
        }
        crate::persistence::PersistenceOrchestrator::recovery_candidates(path)
            .ok()
            .and_then(|candidates| serde_json::to_string(&candidates).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    /// Loads one validated backup generation into the native engine. The
    /// generation is numeric, so callers cannot make this API escape the
    /// project's own `.bak.N` recovery set.
    pub fn restore_project_backup(&self, path: &str, generation: u32) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        let Ok(candidates) = crate::persistence::PersistenceOrchestrator::recovery_candidates(path)
        else {
            return false;
        };
        let Some(candidate) = candidates
            .into_iter()
            .find(|candidate| candidate.generation == generation)
        else {
            return false;
        };
        let current_path = std::path::Path::new(path);
        let rollback_path = std::path::PathBuf::from(format!("{path}.before-restore"));
        let rollback_ready =
            current_path.is_file() && std::fs::copy(current_path, &rollback_path).is_ok();
        let candidate_valid = std::fs::read(&candidate.path)
            .ok()
            .is_some_and(|bytes| bytes.len() >= 8 && bytes.starts_with(b"ARUA"));
        if !candidate_valid {
            return false;
        }
        let restored = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.load_project(candidate.path.to_string_lossy().as_ref()));
        if restored {
            return true;
        }
        if rollback_ready {
            let _ = self.engine.as_ref().is_some_and(|engine| {
                engine.load_project(rollback_path.to_string_lossy().as_ref())
            });
        }
        false
    }
}
