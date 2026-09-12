impl AuraCore {
    pub fn save_project_v2(&self, path: &str, name: &str, bpm: f32) -> anyhow::Result<()> {
        let _project_transaction = self
            .project_transaction
            .lock()
            .map_err(|_| anyhow::anyhow!("project transaction lock poisoned"))?;
        let Some(engine) = self.engine.as_ref() else {
            return Err(anyhow::anyhow!("AudioEngine is unavailable"));
        };
        let save_generation = self.project_generation();
        // Native layout JSON does not contain control-plane identity. Keep the
        // UUID from the existing canonical document when editing an existing
        // project; otherwise every save would look like a different project
        // to history, asset manifests, and external automation.
        let existing_project_id = if std::path::Path::new(path).is_file() {
            Some(ProjectDocument::load(path)?.project_id)
        } else {
            None
        };
        let layout = engine.get_project_layout_json();
        // The engine is authoritative after interactive edits.  The caller's
        // BPM is retained only as a legacy fallback for an unavailable or
        // invalid engine value; otherwise a UI/CLI tempo edit could be lost
        // on the next save even though the native tempo map was updated.
        let engine_bpm = engine.get_tempo();
        let persisted_bpm = if engine_bpm.is_finite() && (20.0..=300.0).contains(&engine_bpm) {
            engine_bpm
        } else {
            bpm
        };
        let mut document = ProjectDocument::from_layout_json(
            name,
            persisted_bpm,
            engine.get_sample_rate(),
            layout.as_str(),
        )?;
        document.audio_routes =
            serde_json::from_str(engine.get_routing_snapshot_json().as_str())
                .map_err(|_| anyhow::anyhow!("native routing snapshot is malformed"))?;
        document.cycle_start_sample = engine.cycle_start();
        document.cycle_end_sample = engine.cycle_end();
        document.cycle_enabled =
            engine.is_loop_enabled() && document.cycle_end_sample > document.cycle_start_sample;
        document.metronome_enabled = engine.is_metronome_enabled();
        document.master_gain = engine.get_master_gain();
        document.metadata.key_root = engine.tonal_root();
        document.metadata.scale_type = engine.tonal_scale_type();
        if let Some(project_id) = existing_project_id {
            document.project_id = project_id;
        }
        // Freeze metadata is derived from the native layout and the cache
        // file, then stamped with the same project/audio generations as the
        // document being saved.  A stale cache is rejected instead of being
        // silently persisted as if it belonged to the current graph.
        let project_generation = self.project_generation().max(1);
        let audio_generation = engine.get_audio_config_generation().max(1);
        for artifact in &mut document.freeze_artifacts {
            artifact.project_generation = project_generation;
            artifact.audio_generation = audio_generation;
            let original_path = std::path::PathBuf::from(&artifact.path);
            let bytes = std::fs::read(&original_path)
                .with_context(|| format!("freeze cache is unreadable: {}", artifact.path))?;
            artifact.content_checksum = PersistenceOrchestrator::calculate_checksum(&bytes);
            let project_parent = std::path::Path::new(path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            if let (Ok(parent), Ok(cache)) = (
                std::fs::canonicalize(project_parent),
                std::fs::canonicalize(&original_path),
            ) {
                if let Ok(relative) = cache.strip_prefix(&parent) {
                    artifact.path = relative.to_string_lossy().into_owned();
                }
            }
        }
        // Persist the renderable graph as part of the canonical document.
        // Target IDs are stable track IDs, while the audio configuration
        // generation prevents a saved target from being reused after a
        // device-format change without revalidation.
        let offline_generation = engine.get_audio_config_generation().max(1);
        document.render_targets = document
            .tracks
            .iter()
            .map(|track| RenderTargetContract {
                target_id: format!("track:{}", track.id),
                kind: if track.track_type == "Bus" {
                    RenderTargetKind::Bus
                } else {
                    RenderTargetKind::Track
                },
                source_id: track.id,
                pre_fader: false,
                include_inserts: true,
                include_tail: true,
                offline_generation,
            })
            .chain(std::iter::once(RenderTargetContract {
                target_id: "master".to_owned(),
                kind: RenderTargetKind::Master,
                source_id: 0,
                pre_fader: false,
                include_inserts: true,
                include_tail: true,
                offline_generation,
            }))
            .collect();
        document.openutau_vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?
            .clone();
        document.track_stacks = self
            .track_stacks
            .lock()
            .map_err(|_| anyhow::anyhow!("Track stack metadata lock poisoned"))?
            .clone();
        document.markers = self
            .markers
            .lock()
            .map_err(|_| anyhow::anyhow!("Arrangement marker lock poisoned"))?
            .clone();
        document.vca_groups = serde_json::from_str(engine.get_vca_snapshot_json().as_str())
            .map_err(|_| anyhow::anyhow!("VCA snapshot is malformed"))?;
        let project_track_ids: std::collections::HashSet<u32> =
            document.tracks.iter().map(|track| track.id).collect();
        document.aux_track_ids = document
            .aux_track_ids
            .iter()
            .copied()
            .filter(|track_id| project_track_ids.contains(track_id))
            .collect();
        for group in &mut document.vca_groups {
            group
                .track_ids
                .retain(|track_id| project_track_ids.contains(track_id));
        }
        document
            .vca_groups
            .retain(|group| !group.track_ids.is_empty());
        document.aux_track_ids = serde_json::from_str(&self.aux_track_ids_json())
            .map_err(|_| anyhow::anyhow!("Aux track metadata is malformed"))?;
        document.macro_mappings = self
            .macro_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("Macro mapping lock poisoned"))?
            .clone();
        document.midi_learn_mappings = self
            .midi_learn_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI mapping lock poisoned"))?
            .clone();
        document.midi_notes = self
            .scheduled_midi_notes
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI note lock poisoned"))?
            .clone();
        document.chord_track = self
            .chord_track
            .lock()
            .map_err(|_| anyhow::anyhow!("chord track lock poisoned"))?
            .clone();
        document.midi_events = self
            .midi_events
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI event lock poisoned"))?
            .clone();
        // Comping is part of the editable arrangement, not merely a UI cache.
        // Persist the typed representation in the canonical project document
        // so project_v2 does not silently lose the chosen take sections.
        let comping_snapshot = self.comping_snapshot_json();
        let comping_state: crate::comping::CompingOrchestrator =
            serde_json::from_str(&comping_snapshot)
                .map_err(|_| anyhow::anyhow!("comping state is not serializable"))?;
        if !comping_state.audit_comping() {
            return Err(anyhow::anyhow!("comping state failed validation"));
        }
        document.comp_takes = comping_state
            .takes
            .iter()
            .map(|take| crate::project_contracts::CompTakeContract {
                id: take.id,
                name: take.name.clone(),
                start_sample: take.start_sample,
                end_sample: take.end_sample,
            })
            .collect();
        document.comp_segments = comping_state
            .current_comp
            .iter()
            .map(|segment| crate::project_contracts::CompSegmentContract {
                take_id: segment.take_id,
                start_sample: segment.start,
                length_samples: segment.len,
                crossfade_samples: segment.crossfade_samples,
            })
            .collect();
        let tempo_values = self.get_tempo_events();
        if !tempo_values.len().is_multiple_of(3) {
            return Err(anyhow::anyhow!("native tempo map returned malformed data"));
        }
        document.tempo_events = tempo_values
            .as_chunks::<3>()
            .0
            .iter()
            .map(|event| crate::project_contracts::TempoEventContract {
                beat: event[0],
                bpm: event[1],
                ramp: event[2] != 0.0,
            })
            .collect();
        let signature_values = self.get_time_signature_events();
        if !signature_values.len().is_multiple_of(3) {
            return Err(anyhow::anyhow!(
                "native time signature map returned malformed data"
            ));
        }
        document.time_signature_events = signature_values
            .as_chunks::<3>()
            .0
            .iter()
            .map(
                |event| crate::project_contracts::TimeSignatureEventContract {
                    beat: event[0],
                    numerator: event[1] as u8,
                    denominator: event[2] as u8,
                },
            )
            .collect();
        if self.project_generation() != save_generation {
            return Err(anyhow::anyhow!(
                "project changed while it was being serialized; save was rejected"
            ));
        }
        let control_room = self
            .control_room
            .lock()
            .map_err(|_| anyhow::anyhow!("control room lock poisoned"))?
            .clone();
        if !control_room.validate() {
            return Err(anyhow::anyhow!("control room state is invalid"));
        }
        // Keep the monitor graph in the canonical project document.  The
        // sidecar below remains for backwards compatibility, but a project
        // save is now self-contained and cannot succeed while silently
        // dropping the control-room state.
        document.control_room = Some(control_room.clone());
        document.save_atomic(path)?;
        // Control Room is a session-scoped monitor graph rather than part of
        // the native track layout. Persist it beside the project document so
        // save/load cannot silently reset monitor selection, cues, or the
        // reference track.
        let control_room_path = format!("{path}.control-room.json");
        let control_room_json = serde_json::to_vec_pretty(&control_room)?;
        let control_room_path = std::path::PathBuf::from(control_room_path);
        let control_room_tmp = std::path::PathBuf::from(format!(
            "{}.tmp-{}-{}",
            control_room_path.display(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        let write_result = (|| -> anyhow::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&control_room_tmp)
                .context("failed to create control room temporary file")?;
            use std::io::Write;
            file.write_all(&control_room_json)
                .context("failed to write control room state")?;
            file.sync_all()
                .context("failed to flush control room state")?;
            std::fs::rename(&control_room_tmp, &control_room_path)
                .context("failed to replace control room state")?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = std::fs::remove_file(&control_room_tmp);
        }
        // The embedded state is authoritative.  A sidecar write can fail on
        // read-only or network filesystems without invalidating the already
        // committed project document; clean up the temporary file and return
        // success so callers do not retry and overwrite a valid save.
        let _ = write_result;
        Ok(())
    }
}
