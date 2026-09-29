impl HirariCore {
    pub fn save_project_v2(&self, path: &str, name: &str, bpm: f32) -> anyhow::Result<()> {
        let _project_transaction = self
            .project_transaction
            .lock()
            .map_err(|_| anyhow::anyhow!("project transaction lock poisoned"))?;
        let mut document = self.build_project_document_v2(name, bpm)?;
        Self::finalize_project_document_snapshot_v2(path, &mut document)?;
        document.save_atomic(path)?;
        Ok(())
    }

    /// Captures an immutable project document while holding the Core
    /// transaction lock without reading project or freeze files. UI clients
    /// can move file-dependent finalization, serialization, and publication
    /// to a worker after this call returns.
    pub fn project_document_snapshot_v2(
        &self,
        name: &str,
        bpm: f32,
    ) -> anyhow::Result<ProjectDocument> {
        let _project_transaction = self
            .project_transaction
            .lock()
            .map_err(|_| anyhow::anyhow!("project transaction lock poisoned"))?;
        self.build_project_document_v2(name, bpm)
    }

    /// Builds the typed project snapshot from a layout captured by the native
    /// engine snapshot worker. Revisions are checked both before and after
    /// Rust-owned state is copied, so edits during capture reject the save.
    pub fn project_document_snapshot_v2_from_native_layout(
        &self,
        name: &str,
        bpm: f32,
        native_revision: u64,
        project_revision: u64,
        layout_json: String,
    ) -> anyhow::Result<ProjectDocument> {
        let (layout, aux_track_ids) = self.project_layout_snapshot_from_native_json(layout_json);
        self.project_document_snapshot_v2_from_normalized_layout_seed(
            name,
            bpm,
            native_revision,
            project_revision,
            layout,
            aux_track_ids,
            None,
        )
    }

    /// Enriches a layout document seed parsed by a worker with Core-owned
    /// session state, while checking both project revisions around capture.
    pub fn project_document_snapshot_v2_from_layout_seed(
        &self,
        name: &str,
        bpm: f32,
        native_revision: u64,
        project_revision: u64,
        layout: String,
        aux_track_ids: String,
        document_seed: ProjectDocument,
    ) -> anyhow::Result<ProjectDocument> {
        self.project_document_snapshot_v2_from_normalized_layout_seed(
            name,
            bpm,
            native_revision,
            project_revision,
            layout,
            aux_track_ids,
            Some(document_seed),
        )
    }

    fn project_document_snapshot_v2_from_normalized_layout_seed(
        &self,
        name: &str,
        bpm: f32,
        native_revision: u64,
        project_revision: u64,
        layout: String,
        aux_track_ids: String,
        document_seed: Option<ProjectDocument>,
    ) -> anyhow::Result<ProjectDocument> {
        let _project_transaction = self
            .project_transaction
            .lock()
            .map_err(|_| anyhow::anyhow!("project transaction lock poisoned"))?;
        let Some(engine) = self.engine.as_ref() else {
            return Err(anyhow::anyhow!("AudioEngine is unavailable"));
        };
        if engine.get_project_state_revision() != native_revision
            || self.project_state_revision() != project_revision
            || document_seed.as_ref().is_some_and(|document| {
                (document.sample_rate - engine.get_sample_rate()).abs() > 0.5
            })
        {
            return Err(anyhow::anyhow!(
                "project changed before snapshot capture completed"
            ));
        }
        self.build_project_document_v2_with_layout(
            name,
            bpm,
            Some((
                layout,
                native_revision,
                project_revision,
                aux_track_ids,
                document_seed,
            )),
        )
    }

    /// Resolves file-backed data after the Core snapshot has been captured.
    /// This is intentionally independent of HirariCore so UI saves can run it
    /// on the serialization worker.
    pub fn finalize_project_document_snapshot_v2(
        path: &str,
        document: &mut ProjectDocument,
    ) -> anyhow::Result<()> {
        // Native layout JSON does not contain control-plane identity. Keep the
        // UUID from the existing canonical document when editing an existing
        // project; otherwise history and external automation see a new project
        // after every save.
        if std::path::Path::new(path).is_file() {
            document.project_id = ProjectDocument::load(path)?.project_id;
        }
        for artifact in &mut document.freeze_artifacts {
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
        Ok(())
    }

    fn build_project_document_v2(&self, name: &str, bpm: f32) -> anyhow::Result<ProjectDocument> {
        self.build_project_document_v2_with_layout(name, bpm, None)
    }

    fn build_project_document_v2_with_layout(
        &self,
        name: &str,
        bpm: f32,
        async_layout: Option<(String, u64, u64, String, Option<ProjectDocument>)>,
    ) -> anyhow::Result<ProjectDocument> {
        let Some(engine) = self.engine.as_ref() else {
            return Err(anyhow::anyhow!("AudioEngine is unavailable"));
        };
        // Use one canonical layout snapshot for document construction and
        // retain its native fingerprint plus Aux identity for the final
        // stale-save check.
        let (
            layout,
            native_layout_fingerprint,
            aux_track_ids_snapshot,
            expected_revisions,
            document_seed,
        ) = match async_layout {
            Some((layout, native_revision, project_revision, aux_ids, document_seed)) => (
                layout,
                None,
                aux_ids,
                Some((native_revision, project_revision)),
                document_seed,
            ),
            None => {
                let (layout, fingerprint, aux_ids) = self.project_layout_snapshot_for_save();
                (layout, Some(fingerprint), aux_ids, None, None)
            }
        };
        let save_generation = crate::command_api::snapshot_generation(layout.as_bytes());
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
        let mut document = match document_seed {
            Some(document) => document,
            None => ProjectDocument::from_layout_json(
                name,
                persisted_bpm,
                engine.get_sample_rate(),
                layout.as_str(),
            )?,
        };
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
        // Freeze metadata is derived from the native layout and the cache
        // file, then stamped with the same project/audio generations as the
        // document being saved. Content checksums are read by the publication
        // worker after this Core snapshot has been released.
        let project_generation = save_generation.max(1);
        let audio_generation = engine.get_audio_config_generation().max(1);
        for artifact in &mut document.freeze_artifacts {
            artifact.project_generation = project_generation;
            artifact.audio_generation = audio_generation;
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
        document.aux_track_ids = serde_json::from_str(&aux_track_ids_snapshot)
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
                regions: take
                    .regions
                    .iter()
                    .map(|region| crate::project_contracts::CompTakeRegionContract {
                        track_id: region.track_id,
                        region_id: region.region_id,
                    })
                    .collect(),
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
        let stale = if let Some((native_revision, project_revision)) = expected_revisions {
            engine.get_project_state_revision() != native_revision
                || self.project_state_revision() != project_revision
        } else {
            self.native_project_layout_fingerprint() != native_layout_fingerprint.unwrap_or(0)
                || self.aux_track_ids_json() != aux_track_ids_snapshot
        };
        if stale {
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
        // Keep the monitor graph in the canonical project document so the
        // project save is self-contained and cannot silently drop it.
        document.control_room = Some(control_room);
        Ok(document)
    }
}
