#[path = "hirari_core_project_loading_tracks.rs"]
mod tracks_;
#[path = "hirari_core_project_loading_regions.rs"]
mod regions_;

impl HirariCore {
    pub fn load_project_v2(&self, path: &str) -> anyhow::Result<()> {
        let _project_transaction = self
            .project_transaction
            .lock()
            .map_err(|_| anyhow::anyhow!("project transaction lock poisoned"))?;
        let document = ProjectDocument::load(path)?;
        let project_parent = std::path::Path::new(path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let canonical_project_parent = std::fs::canonicalize(project_parent)
            .map_err(|error| anyhow::anyhow!("failed to resolve project directory: {error}"))?;
        let mut document = document;
        // Validate the sidecar before mutating native state. A malformed
        // monitor graph must never leave the currently loaded project
        // partially hydrated or make rollback impossible.
        let control_room_path = format!("{path}.control-room.json");
        let persisted_control_room = match document.control_room.clone() {
            Some(state) => {
                if !state.validate() {
                    return Err(anyhow::anyhow!("invalid embedded control room state"));
                }
                Some(state)
            }
            None => match std::fs::read(&control_room_path) {
            Ok(bytes) => {
                let state: crate::control_room::ControlRoomState = serde_json::from_slice(&bytes)
                    .map_err(|error| anyhow::anyhow!("invalid control room state: {error}"))?;
                if !state.validate() {
                    return Err(anyhow::anyhow!("invalid control room state"));
                }
                Some(state)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(anyhow::anyhow!("failed to read control room state: {error}")),
            },
        };
        // A project publishes a MIDI event sidecar next to itself. Validate it
        // under the same rule as the control-room state: a sidecar that exists
        // but cannot be parsed is corrupt project data, and silently ignoring it
        // would drop the user's events while reporting a successful open.
        // Validate before any native state is touched so a rejected project
        // leaves the currently open one intact.
        let midi_sidecar_path = format!("{path}.midi-events.json");
        match std::fs::read_to_string(&midi_sidecar_path) {
            Ok(contents) => {
                let events: Vec<midi::MIDIEvent> = serde_json::from_str(&contents)
                    .map_err(|error| anyhow::anyhow!("invalid MIDI event sidecar: {error}"))?;
                if events.iter().any(|event| !event.validate()) {
                    return Err(anyhow::anyhow!("invalid MIDI event sidecar"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "failed to read MIDI event sidecar: {error}"
                ))
            }
        }
        // A plugin binary is part of the cache identity.  Never hydrate a
        // state blob captured from a different binary: keep the project
        // editable, but let the plugin start from its defaults and acquire a
        // fresh state through the normal admission path.
        for plugin in &mut document.plugin_instances {
            if plugin.binary_hash.is_empty() || plugin.bundle_path.trim().is_empty() {
                continue;
            }
            let bundle_path = std::path::Path::new(&plugin.bundle_path);
            let resolved_path = if bundle_path.is_absolute() {
                bundle_path.to_path_buf()
            } else {
                project_parent.join(bundle_path)
            };
            let current_hash = crate::plugin_catalog::binary_hash_for_path(
                resolved_path.to_string_lossy().as_ref(),
            );
            if current_hash.as_deref() != Some(plugin.binary_hash.as_str()) {
                plugin.state_blob.clear();
                plugin.gui_state.clear();
                // Parameter IDs are owned by the plugin binary too. Carrying
                // values across a binary replacement can silently control a
                // different parameter or fail the whole project transaction.
                plugin.parameter_values.clear();
                plugin.state_generation = plugin.state_generation.saturating_add(1);
                if let Some(track) = document
                    .tracks
                    .iter_mut()
                    .find(|track| track.id == plugin.track_id)
                {
                    let slot_index = plugin.slot_index as usize;
                    if let Some(state) = track.plugin_states.get_mut(plugin.slot_index as usize) {
                        state.clear();
                    }
                    if let Some(state) = track
                        .sandbox_plugin_states
                        .get_mut(plugin.slot_index as usize)
                    {
                        state.clear();
                    }
                    if let Some(values) = track.plugin_parameter_values.get_mut(slot_index) {
                        values.clear();
                    }
                    if let Some(state) = track.plugin_gui_states.get_mut(slot_index) {
                        state.clear();
                    }
                }
            }
        }
        for artifact in &document.freeze_artifacts {
            let cache_path = std::path::Path::new(&artifact.path);
            let cache_path = if cache_path.is_absolute() || cache_path.is_file() {
                cache_path.to_path_buf()
            } else {
                project_parent.join(cache_path)
            };
            let bytes = std::fs::read(&cache_path).map_err(|error| {
                anyhow::anyhow!(
                    "freeze cache missing for track {} ({}): {}",
                    artifact.track_id,
                    cache_path.display(),
                    error
                )
            })?;
            let checksum = PersistenceOrchestrator::calculate_checksum(&bytes);
            if checksum != artifact.content_checksum {
                return Err(anyhow::anyhow!(
                    "freeze cache checksum mismatch for track {}",
                    artifact.track_id
                ));
            }
        }
        let engine = self
            .engine_opt()
            .ok_or_else(|| anyhow::anyhow!("AudioEngine is unavailable"))?;

        // The Rust ProjectDocument is JSON and is intentionally independent
        // from the legacy native binary serializer. Hydrate the native engine
        // through its typed FFI commands so no project field is silently lost.
        // Validate all filesystem inputs before mutating the current project.
        let mut resolved_region_paths = Vec::with_capacity(document.regions.len());
        for region in &document.regions {
            let path = std::path::Path::new(&region.path);
            if !path.is_absolute()
                && path
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
            {
                return Err(anyhow::anyhow!(
                    "project region asset escapes the project directory: {}",
                    region.path
                ));
            }
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                canonical_project_parent.join(path)
            };
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "project region asset is missing: {}",
                        region.path
                    ));
                }
            };
            // Project hydration must never follow an external symlink.  Apart
            // from making asset collection non-reproducible, following one
            // here would let a project read files outside its declared asset
            // set and would make the native rollback non-deterministic.
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(anyhow::anyhow!(
                    "project region asset must be a regular non-symlink file: {}",
                    region.path
                ));
            }
            let canonical_asset = std::fs::canonicalize(&path).map_err(|error| {
                anyhow::anyhow!("failed to resolve project region asset {}: {error}", region.path)
            })?;
            if !std::path::Path::new(&region.path).is_absolute()
                && !canonical_asset.starts_with(&canonical_project_parent)
            {
                return Err(anyhow::anyhow!(
                    "project region asset escapes the project directory through a symlink: {}",
                    region.path
                ));
            }
            let canonical_metadata = std::fs::metadata(&canonical_asset).map_err(|error| {
                anyhow::anyhow!("failed to inspect project region asset {}: {error}", region.path)
            })?;
            if !canonical_metadata.is_file() {
                return Err(anyhow::anyhow!(
                    "project region asset must resolve to a regular file: {}",
                    region.path
                ));
            }
            resolved_region_paths.push(canonical_asset.to_string_lossy().into_owned());
        }

        let sample_rate = u32::try_from(document.sample_rate as u64)
            .map_err(|_| anyhow::anyhow!("project sample rate is out of range"))?;
        // Keep the current device block size if it is a usable one;
        // otherwise fall back to a standard offline block. A fresh engine
        // reports 0, and `max(1)` turned that into a 1-sample block, which
        // the native graph accepts but cannot render through — every bounce
        // of a just-loaded project came out full-length and silent.
        // This mirrors the validated set in `apply_audio_config`.
        let block_size = match crate::engine_config::block_frames(engine) {
            size @ (32 | 64 | 128 | 256 | 512 | 1024 | 2048) => size,
            _ => 1024,
        };

        // Hydration mutates the native graph incrementally. Keep a native
        // snapshot so a failed track/region restore cannot leave a partially
        // loaded project visible to audio, UI, or autosave.
        let rollback_path = std::env::temp_dir().join(format!(
            "hirari-hydrate-rollback-{}-{}.native",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        let rollback_path_string = rollback_path.to_string_lossy().into_owned();
        let midi_events_before = self
            .midi_events
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI event lock poisoned"))?
            .clone();
        if !engine.save_project(&rollback_path_string) {
            return Err(anyhow::anyhow!(
                "failed to snapshot current native project before hydration"
            ));
        }
        let rollback = || -> anyhow::Result<()> {
            let restored = engine.load_project(&rollback_path_string);
            // Hydration may have recorded normal-looking edits before an
            // asset/plugin failure. Those actions refer to the discarded
            // candidate graph and must never leak into the next project
            // interaction, even if the native rollback itself fails.
            engine.clear_undo_history();
            let _ = std::fs::remove_file(&rollback_path);
            if !restored {
                return Err(anyhow::anyhow!("native project rollback failed"));
            }
            if let Ok(mut events) = self.midi_events.lock() {
                *events = midi_events_before.clone();
            }
            Ok(())
        };
        let rollback_error = |error: anyhow::Error| -> anyhow::Error {
            match rollback() {
                Ok(()) => error,
                Err(restore_error) => {
                    anyhow::anyhow!("{error}; native rollback failed: {restore_error}")
                }
            }
        };

        crate::engine_transport::set_playing(engine, engine.get_core(), false);
        engine.new_project();
        // new_project creates a starter track for an empty UI. Remove it so
        // the persisted track list is reproduced exactly. Track ID 1 is the
        // reserved bootstrap ID; zero is the invalid/stale-ID sentinel and
        // must never be used as a real removal target.
        engine.remove_track(1);
        engine.apply_config(document.metadata.bpm, sample_rate, block_size);
        // Root and scale are Core-owned now, so the document restores them
        // through the Core setter rather than the native engine. The failure
        // contract is unchanged: an out-of-range scale in the file still
        // rolls the whole load back.
        if !self.set_tonal_scale(document.metadata.key_root, document.metadata.scale_type) {
            return Err(rollback_error(anyhow::anyhow!(
                "project contains an unsupported tonal scale"
            )));
        }
        self.tempo_clear_events(document.metadata.bpm as f64);
        self.tempo_clear_signatures();
        for event in &document.tempo_events {
            if !self.tempo_write_event(event.beat, event.bpm, event.ramp) {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid tempo event"
                )));
            }
        }
        for event in &document.time_signature_events {
            if !self.tempo_write_signature(event.beat, event.numerator, event.denominator) {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid time signature event"
                )));
            }
        }

        let native_track_ids = self.restore_project_track_index(
            &document, engine, sample_rate, &rollback_error,
        )?;

        let aux_ids_json = serde_json::to_string(&document.aux_track_ids)
            .map_err(|_| rollback_error(anyhow::anyhow!("Aux track metadata is malformed")))?;
        if !self.restore_aux_track_ids_json(&aux_ids_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid Aux metadata"
            )));
        }

        self.restore_project_elements(
            &document, engine, sample_rate, &resolved_region_paths, &native_track_ids, &project_parent, &rollback_error,
        )?;

        self.finalize_loaded_project(
            &document,
            engine,
            &native_track_ids,
            &rollback_path,
            persisted_control_room,
            &rollback_error,
        )?;
        Ok(())
    }
}
