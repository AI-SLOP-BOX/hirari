impl ProjectDocument {
    pub fn from_layout_json(
        name: impl Into<String>,
        bpm: f32,
        sample_rate: f64,
        layout_json: &str,
    ) -> Result<Self> {
        let layout_tracks: Vec<LayoutTrack> =
            serde_json::from_str(layout_json).context("project layout JSON is malformed")?;
        let mut regions = Vec::new();
        let mut tracks = Vec::with_capacity(layout_tracks.len());
        let mut freeze_artifacts = Vec::new();
        let mut sidechain_routes = Vec::new();
        let mut feedback_routes = Vec::new();
        let mut track_ids = HashSet::with_capacity(layout_tracks.len());

        for track in layout_tracks {
            if !track_ids.insert(track.id) {
                bail!("duplicate track id {}", track.id);
            }
            let track_id = track.id;
            for route in &track.sidechain_routes {
                if route.source_id == route.destination_id
                    || route.destination_id != track_id
                    || route.tap_point > 2
                {
                    bail!("invalid sidechain route for track {track_id}");
                }
                sidechain_routes.push(route.clone());
            }
            for route in &track.feedback_routes {
                if route.source_id == route.destination_id
                    || route.source_id != track_id
                    || !route.gain.is_finite()
                    || !(0.0..=2.0).contains(&route.gain)
                {
                    bail!("invalid feedback route for track {track_id}");
                }
                feedback_routes.push(route.clone());
            }
            if track.frozen && !track.frozen_path.trim().is_empty() {
                let bytes = std::fs::read(&track.frozen_path)
                    .with_context(|| format!("frozen audio cache is unreadable: {}", track.frozen_path))?;
                let total_samples = track.frozen_total_samples;
                let sample_rate = track.frozen_sample_rate;
                if total_samples == 0 || sample_rate == 0 {
                    bail!("frozen audio metadata is invalid for track {track_id}");
                }
                freeze_artifacts.push(FreezeArtifactContract {
                    track_id,
                    project_generation: 1,
                    audio_generation: 1,
                    total_samples,
                    sample_rate,
                    path: track.frozen_path.clone(),
                    content_checksum: crate::persistence::PersistenceOrchestrator::calculate_checksum(&bytes),
                });
            }
            tracks.push(ProjectTrack {
                id: track.id,
                name: track.name,
                track_type: track.track_type,
                volume: track.volume,
                pan: track.pan,
                muted: track.muted,
                solo: track.solo,
                record_armed: track.record_armed,
                phase_invert: track.phase_invert,
                track_delay_samples: track.track_delay_samples,
                volume_automation: track.volume_automation.clone(),
                pan_automation: track.pan_automation.clone(),
                track_delay_automation: track.track_delay_automation.clone(),
                plugin_types: track.plugin_types.clone(),
                plugin_bypasses: plugin_bypasses(&track.plugin_bypasses, track.plugin_types.len())?,
                plugin_parameter_values: plugin_parameter_values(
                    &track.plugin_parameter_values,
                    track.plugin_types.len(),
                )?,
                plugin_states: track
                    .plugin_state_hex
                    .iter()
                    .map(|encoded| decode_hex(encoded))
                    .collect::<Result<Vec<_>>>()?,
                plugin_gui_states: {
                    let decoded = track.plugin_gui_state_hex
                        .iter()
                        .map(|encoded| decode_hex(encoded))
                        .collect::<Result<Vec<_>>>()?;
                    if decoded.is_empty() { vec![Vec::new(); track.plugin_types.len()] } else { decoded }
                },
                plugin_state_versions: state_versions(
                    &track.plugin_state_versions,
                    track.plugin_state_hex.len(),
                )?,
                sandbox_plugin_paths: track.sandbox_plugin_paths.clone(),
                sandbox_plugin_states: track
                    .sandbox_plugin_state_hex
                    .iter()
                    .map(|encoded| decode_hex(encoded))
                    .collect::<Result<Vec<_>>>()?,
                sandbox_plugin_state_versions: state_versions(
                    &track.sandbox_plugin_state_versions,
                    track.sandbox_plugin_state_hex.len(),
                )?,
            });
            for region in track.regions {
                regions.push(ProjectRegion {
                    id: region.id,
                    track_id,
                    name: region.name,
                    path: region.path,
                    start: region.start,
                    length: region.len,
                    source_offset: region.source_offset,
                    base_source_offset: region.base_source_offset,
                    base_length: region.base_length,
                    muted: region.muted,
                    clip_gain: region.clip_gain,
                    fade_in_samples: region.fade_in_samples,
                    fade_out_samples: region.fade_out_samples,
                    warp_ratio: region.warp_ratio,
                    pitch_semitones: region.pitch_semitones,
                    reverse: region.reverse,
                    loop_count: region.loop_count,
                });
            }
        }

        let mut plugin_instances = Vec::new();
        for track in &tracks {
            let mut sandbox_index = 0usize;
            for (slot_index, plugin_type) in track.plugin_types.iter().copied().enumerate() {
                let (format, bundle_path, plugin_id, state_blob, capability, binary_hash) =
                    if plugin_type == u32::MAX {
                        let path = track
                            .sandbox_plugin_paths
                            .get(sandbox_index)
                            .cloned()
                            .unwrap_or_default();
                        let id = std::path::Path::new(&path)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .filter(|value| !value.is_empty())
                            .unwrap_or("external-plugin")
                            .to_owned();
                        let state = track
                            .sandbox_plugin_states
                            .get(sandbox_index)
                            .cloned()
                            .unwrap_or_default();
                        let binary_hash = crate::plugin_catalog::binary_hash_for_path(&path)
                            .unwrap_or_default();
                        sandbox_index += 1;
                        (
                            plugin_format_for_path(&path),
                            path,
                            id,
                            state,
                            "sandbox".to_owned(),
                            binary_hash,
                        )
                    } else {
                        (
                            PluginFormat::BuiltIn,
                            format!("builtin://{plugin_type}"),
                            format!("builtin:{plugin_type}"),
                            track
                                .plugin_states
                                .get(slot_index)
                                .cloned()
                                .unwrap_or_default(),
                            "builtin".to_owned(),
                            String::new(),
                        )
                    };
                plugin_instances.push(PluginInstanceContract {
                    instance_id: format!("track:{}:slot:{}", track.id, slot_index),
                    track_id: track.id,
                    slot_index: slot_index as u32,
                    format,
                    bundle_path,
                    plugin_id,
                    bus_layout: Vec::new(),
                    component_ids: Vec::new(),
                    input_channels: 0,
                    output_channels: 2,
                    sidechain_channels: 0,
                    parameter_ids: Vec::new(),
                    parameter_values: track
                        .plugin_parameter_values
                        .get(slot_index)
                        .cloned()
                        .unwrap_or_default(),
                    latency_samples: 0,
                    state_blob,
                    gui_state: track.plugin_gui_states.get(slot_index).cloned().unwrap_or_default(),
                    bypassed: track.plugin_bypasses.get(slot_index).copied().unwrap_or(false),
                    offline: false,
                    quarantined: false,
                    binary_hash,
                    capability,
                    plugin_version: String::new(),
                    architecture: std::env::consts::ARCH.to_owned(),
                    state_schema_version: PLUGIN_STATE_SCHEMA_VERSION,
                    state_generation: 0,
                });
            }
        }

        let document = Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            contract_version: PROJECT_CONTRACT_VERSION,
            project_id: new_project_id(),
            metadata: ProjectMetadata {
                name: name.into(),
                version: PROJECT_SCHEMA_VERSION,
                bpm,
                tracks_count: track_ids.len() as u32,
                key_root: 0,
                scale_type: 0,
            },
            sample_rate,
            master_gain: 1.0,
            cycle_start_sample: 0,
            cycle_end_sample: 0,
            cycle_enabled: false,
            metronome_enabled: false,
            aux_track_ids: Vec::new(),
            tracks,
            regions,
            plugin_instances,
            midi_learn_mappings: Vec::new(),
            midi_notes: Vec::new(),
            chord_track: Vec::new(),
            midi_events: Vec::new(),
            tempo_events: Vec::new(),
            time_signature_events: Vec::new(),
            macro_mappings: Vec::new(),
            warp_markers: Vec::new(),
            render_targets: Vec::new(),
            freeze_artifacts,
            sidechain_routes,
            feedback_routes,
            audio_routes: Vec::new(),
            openutau_vocals: Vec::new(),
            comp_takes: Vec::new(),
            comp_segments: Vec::new(),
            track_stacks: Vec::new(),
            markers: Vec::new(),
            vca_groups: Vec::new(),
            hardware_inserts: Vec::new(),
            control_room: None,
        };
        document.validate()?;
        Ok(document)
    }
}
