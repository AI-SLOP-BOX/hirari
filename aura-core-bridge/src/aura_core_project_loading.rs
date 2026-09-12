impl AuraCore {
    pub fn load_project_v2(&self, path: &str) -> anyhow::Result<()> {
        let _project_transaction = self
            .project_transaction
            .lock()
            .map_err(|_| anyhow::anyhow!("project transaction lock poisoned"))?;
        let document = ProjectDocument::load(path)?;
        let project_parent = std::path::Path::new(path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
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
                plugin.state_generation = plugin.state_generation.saturating_add(1);
                if let Some(track) = document
                    .tracks
                    .iter_mut()
                    .find(|track| track.id == plugin.track_id)
                {
                    if let Some(state) = track.plugin_states.get_mut(plugin.slot_index as usize) {
                        state.clear();
                    }
                    if let Some(state) = track
                        .sandbox_plugin_states
                        .get_mut(plugin.slot_index as usize)
                    {
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
            .engine
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AudioEngine is unavailable"))?;

        // The Rust ProjectDocument is JSON and is intentionally independent
        // from the legacy native binary serializer. Hydrate the native engine
        // through its typed FFI commands so no project field is silently lost.
        // Validate all filesystem inputs before mutating the current project.
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
                project_parent.join(path)
            };
            let metadata = match std::fs::symlink_metadata(path) {
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
        }

        let sample_rate = u32::try_from(document.sample_rate as u64)
            .map_err(|_| anyhow::anyhow!("project sample rate is out of range"))?;
        let block_size = engine.get_block_size().max(1);

        // Hydration mutates the native graph incrementally. Keep a native
        // snapshot so a failed track/region restore cannot leave a partially
        // loaded project visible to audio, UI, or autosave.
        let rollback_path = std::env::temp_dir().join(format!(
            "aura-hydrate-rollback-{}-{}.native",
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

        engine.set_playing(false);
        engine.new_project();
        // new_project creates a starter track for an empty UI. Remove it so
        // the persisted track list is reproduced exactly. Track ID 1 is the
        // reserved bootstrap ID; zero is the invalid/stale-ID sentinel and
        // must never be used as a real removal target.
        engine.remove_track(0);
        engine.apply_config(document.metadata.bpm, sample_rate, block_size);
        if !engine.set_tonal_scale(document.metadata.key_root, document.metadata.scale_type) {
            return Err(rollback_error(anyhow::anyhow!(
                "project contains an unsupported tonal scale"
            )));
        }
        engine.clear_tempo_events(document.metadata.bpm as f64);
        engine.clear_time_signature_events();
        for event in &document.tempo_events {
            if !engine.set_tempo_event(event.beat, event.bpm, event.ramp) {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid tempo event"
                )));
            }
        }
        for event in &document.time_signature_events {
            if !engine.set_time_signature_event(event.beat, event.numerator, event.denominator) {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid time signature event"
                )));
            }
        }

        let mut native_track_ids = std::collections::HashMap::with_capacity(document.tracks.len());
        let mut used_native_track_ids =
            std::collections::HashSet::with_capacity(document.tracks.len());
        for track in &document.tracks {
            let native_id = match ProjectDocument::native_track_type_code(&track.track_type) {
                Ok(track_type) => engine.add_track(track_type),
                Err(error) => {
                    return Err(rollback_error(error));
                }
            };
            if native_id == 0 {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to create track {}",
                    track.id
                )));
            }
            if !used_native_track_ids.insert(native_id) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine returned duplicate track id {native_id}"
                )));
            }
            if !engine.set_track_name(native_id, &track.name) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to name track {}",
                    track.id
                )));
            }
            // Project loading is a control-thread state transition, not an
            // audio callback command. Applying these values immediately
            // prevents stale queued commands from overwriting later edits
            // when the first render/audio block drains the queue.
            if !engine.set_track_volume(native_id, track.volume)
                || !engine.set_track_pan(native_id, track.pan)
                || !engine.set_track_mute(native_id, track.muted)
                || !engine.set_track_solo(native_id, track.solo)
                || !engine.set_track_armed(native_id, track.record_armed)
                || !engine.set_phase_invert(native_id, track.phase_invert)
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore state for track {}",
                    track.id
                )));
            }
            let mut sandbox_index = 0u32;
            for (plugin_index, plugin_type) in track.plugin_types.iter().copied().enumerate() {
                if plugin_type == u32::MAX {
                    let Some(path) = track.sandbox_plugin_paths.get(sandbox_index as usize) else {
                        return Err(rollback_error(anyhow::anyhow!(
                            "sandbox plugin metadata is missing for track {}",
                            track.id
                        )));
                    };
                    if !engine.add_sandboxed_plugin(native_id, path) {
                        let admission = engine.get_last_sandbox_failure_text(native_id);
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore sandbox plugin {plugin_index} on track {}: {}",
                            track.id, admission
                        )));
                    }
                    if track
                        .plugin_bypasses
                        .get(plugin_index)
                        .copied()
                        .unwrap_or(false)
                        && !engine.set_plugin_bypass(native_id, plugin_index as u32, true)
                    {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore bypass state for sandbox plugin {plugin_index} on track {}",
                            track.id
                        )));
                    }
                    if let Some(state) = track.sandbox_plugin_states.get(sandbox_index as usize) {
                        if !state.is_empty()
                            && !engine.set_sandbox_plugin_state(native_id, sandbox_index, state)
                        {
                            let state_error = engine
                                .get_sandbox_plugin_state_error_text(native_id, sandbox_index);
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore sandbox plugin state {plugin_index} on track {}: {}",
                                track.id, state_error
                            )));
                        }
                    }
                    sandbox_index += 1;
                    for (parameter_id, value) in track
                        .plugin_parameter_values
                        .get(plugin_index)
                        .into_iter()
                        .flatten()
                        .copied()
                        .enumerate()
                    {
                        if !engine.set_plugin_parameter_without_undo(
                            native_id,
                            plugin_index as u32,
                            parameter_id as u32,
                            value,
                        ) {
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore parameter {parameter_id} for sandbox plugin {plugin_index} on track {}",
                                track.id
                            )));
                        }
                    }
                } else {
                    if plugin_type > 10 || !engine.add_plugin(native_id, plugin_type) {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore unsupported plugin {plugin_index} on track {}",
                            track.id
                        )));
                    }
                    if let Some(state) = track.plugin_states.get(plugin_index) {
                        if !state.is_empty()
                            && !engine.set_plugin_state(native_id, plugin_index as u32, state)
                        {
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore plugin state {plugin_index} on track {}",
                                track.id
                            )));
                        }
                    }
                    if track
                        .plugin_bypasses
                        .get(plugin_index)
                        .copied()
                        .unwrap_or(false)
                        && !engine.set_plugin_bypass(native_id, plugin_index as u32, true)
                    {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore bypass state for plugin {plugin_index} on track {}",
                            track.id
                        )));
                    }
                    for (parameter_id, value) in track
                        .plugin_parameter_values
                        .get(plugin_index)
                        .into_iter()
                        .flatten()
                        .copied()
                        .enumerate()
                    {
                        if !engine.set_plugin_parameter_without_undo(
                            native_id,
                            plugin_index as u32,
                            parameter_id as u32,
                            value,
                        ) {
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore parameter {parameter_id} for plugin {plugin_index} on track {}",
                                track.id
                            )));
                        }
                    }
                }
            }
            for (parameter_id, points) in [
                (0u32, &track.volume_automation),
                (1u32, &track.pan_automation),
            ] {
                if !points.is_empty() {
                    let packed = points
                        .iter()
                        .flat_map(|point| {
                            [point.time, f64::from(point.value), f64::from(point.curve)]
                        })
                        .collect();
                    if !engine.set_automation_data(native_id, parameter_id, packed) {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore automation {} for track {}",
                            parameter_id,
                            track.id
                        )));
                    }
                }
            }
            if !track.track_delay_automation.is_empty() {
                let packed = track
                    .track_delay_automation
                    .iter()
                    .flat_map(|point| [point.time, f64::from(point.value), f64::from(point.curve)])
                    .collect();
                if !engine.set_track_delay_automation(native_id, packed) {
                    return Err(rollback_error(anyhow::anyhow!(
                        "native engine failed to restore track delay automation for track {}",
                        track.id
                    )));
                }
            }
            native_track_ids.insert(track.id, native_id);
        }
        let aux_ids_json = serde_json::to_string(&document.aux_track_ids)
            .map_err(|_| rollback_error(anyhow::anyhow!("Aux track metadata is malformed")))?;
        if !self.restore_aux_track_ids_json(&aux_ids_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid Aux metadata"
            )));
        }

        // Region IDs are allocated by the native engine. Since the project was
        // reset immediately above, the first imported region is deterministic;
        // use the native layout after each import to obtain its actual ID.
        let mut used_native_region_ids = std::collections::HashSet::new();
        for region in &document.regions {
            let region_path = std::path::Path::new(&region.path);
            let region_path = if region_path.is_absolute() {
                region_path.to_path_buf()
            } else {
                project_parent.join(region_path)
            };
            let region_path = region_path.to_string_lossy().into_owned();
            let native_track_id = *native_track_ids
                .get(&region.track_id)
                .ok_or_else(|| anyhow::anyhow!("region references an unknown track"))
                .map_err(rollback_error)?;
            if !engine.add_region(native_track_id, &region_path, region.start as f64) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to import region {}",
                    region.id
                )));
            }
            let layout = engine.get_project_layout_json();
            let native_regions: Vec<NativeLayoutTrack> = serde_json::from_str(&layout)
                .context("native engine returned malformed project layout")
                .map_err(rollback_error)?;
            let native_region = native_regions
                .into_iter()
                .filter(|track| track.id == native_track_id)
                .flat_map(|track| track.regions)
                .find(|candidate| {
                    candidate.path == region_path
                        && candidate.start == region.start
                        && (region.base_length > 0 || candidate.len == region.length)
                        && !used_native_region_ids.contains(&candidate.id)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "native engine did not preserve region position for {}",
                        region.id
                    )
                })
                .map_err(rollback_error)?;
            used_native_region_ids.insert(native_region.id);
            let trim_ok = if region.base_length > 0 {
                let start_norm = (region
                    .source_offset
                    .saturating_sub(region.base_source_offset)
                    as f64
                    / region.base_length as f64) as f32;
                let end_norm = (region
                    .source_offset
                    .saturating_sub(region.base_source_offset)
                    .saturating_add(region.length) as f64
                    / region.base_length as f64) as f32;
                engine.set_region_trim(native_track_id, native_region.id, start_norm, end_norm)
            } else {
                true
            };
            if !engine.set_region_gain(native_track_id, native_region.id, region.clip_gain)
                || !engine.set_region_muted(native_track_id, native_region.id, region.muted)
                || !engine.set_region_fades(
                    native_track_id,
                    native_region.id,
                    region.fade_in_samples as f32,
                    region.fade_out_samples as f32,
                )
                || !trim_ok
                || !engine.set_region_reverse(native_track_id, native_region.id, region.reverse)
                || !engine.set_region_warp_ratio(
                    native_track_id,
                    native_region.id,
                    region.warp_ratio,
                )
                || !engine.set_region_pitch_semitones(
                    native_track_id,
                    native_region.id,
                    region.pitch_semitones,
                )
                || !engine.set_region_loop_count(
                    native_track_id,
                    native_region.id,
                    region.loop_count,
                )
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore region envelope {}",
                    region.id
                )));
            }
        }

        // Sidechain routes are logical project data; their runtime buffers
        // must be recreated only after every referenced track and plugin has
        // been hydrated. A failure rolls the whole hydration back instead of
        // leaving a project with a visually connected but silent edge.
        for route in &document.sidechain_routes {
            let native_source_id = *native_track_ids
                .get(&route.source_id)
                .ok_or_else(|| anyhow::anyhow!("sidechain source references an unknown track"))
                .map_err(rollback_error)?;
            let native_destination_id = *native_track_ids
                .get(&route.destination_id)
                .ok_or_else(|| anyhow::anyhow!("sidechain destination references an unknown track"))
                .map_err(rollback_error)?;
            if !engine.set_sidechain_link(
                native_source_id,
                native_destination_id,
                route.plugin_index,
                route.tap_point,
                true,
            ) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine rejected persisted sidechain route"
                )));
            }
        }

        for route in &document.feedback_routes {
            let native_source_id = *native_track_ids
                .get(&route.source_id)
                .ok_or_else(|| anyhow::anyhow!("feedback source references an unknown track"))
                .map_err(rollback_error)?;
            let native_destination_id = *native_track_ids
                .get(&route.destination_id)
                .ok_or_else(|| anyhow::anyhow!("feedback destination references an unknown track"))
                .map_err(rollback_error)?;
            if !engine.set_feedback_route(native_source_id, native_destination_id, route.gain, true)
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine rejected persisted feedback route"
                )));
            }
        }

        for route in &document.audio_routes {
            let native_source_id = *native_track_ids
                .get(&route.source_id)
                .ok_or_else(|| anyhow::anyhow!("audio route source references an unknown track"))
                .map_err(rollback_error)?;
            let native_destination_id = *native_track_ids
                .get(&route.destination_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("audio route destination references an unknown track")
                })
                .map_err(rollback_error)?;
            if !engine.set_route_gain(native_source_id, native_destination_id, route.gain, true) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine rejected persisted audio route"
                )));
            }
        }

        // Freeze caches are immutable render artifacts on disk, but the
        // native graph owns the runtime buffer. Rebuild that buffer only
        // after the complete track/plugin/region graph has been hydrated;
        // otherwise a partial project could become audible as a frozen track.
        for artifact in &document.freeze_artifacts {
            let native_track_id = *native_track_ids
                .get(&artifact.track_id)
                .ok_or_else(|| anyhow::anyhow!("freeze artifact references an unknown track"))
                .map_err(rollback_error)?;
            let cache_path = std::path::Path::new(&artifact.path);
            let resolved_cache_path = if cache_path.is_absolute() {
                cache_path.to_path_buf()
            } else {
                project_parent.join(cache_path)
            };
            if !engine.restore_track_freeze_from_file(
                native_track_id,
                resolved_cache_path.to_string_lossy().as_ref(),
                artifact.total_samples,
                sample_rate,
            ) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore freeze artifact for track {}",
                    artifact.track_id
                )));
            }
        }

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
