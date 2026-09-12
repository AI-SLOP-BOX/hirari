impl ProjectDocument {
    /// Converts the persisted track label to the Native Engine enum value.
    /// Keeping this mapping here prevents the FFI hydration path from silently
    /// falling back to Audio for an unknown or misspelled track type.
    pub fn native_track_type_code(track_type: &str) -> Result<u32> {
        match track_type {
            "Audio" => Ok(0),
            "Midi" => Ok(1),
            "Instrument" => Ok(2),
                "Bus" | "Aux" => Ok(3),
            "Vocal" => Ok(4),
            _ => bail!("unsupported track type {track_type:?}"),
        }
    }

    /// Add a clean, empty track to an offline project document. IDs are
    /// monotonic within the document and the type is validated before any
    /// mutation is published.
    pub fn add_track(&mut self, name: impl Into<String>, track_type: &str) -> Result<u32> {
        let name = name.into();
        if name.trim().is_empty() || name.len() > 1024 { bail!("track name is invalid"); }
        Self::native_track_type_code(track_type)?;
        let id = self.tracks.iter().map(|track| track.id).max().unwrap_or(0)
            .checked_add(1).ok_or_else(|| anyhow::anyhow!("track id space exhausted"))?;
        self.tracks.push(ProjectTrack {
            id, name, track_type: track_type.to_owned(), volume: 1.0, pan: 0.0,
            muted: false, solo: false, record_armed: false, phase_invert: false,
            track_delay_samples: 0, volume_automation: Vec::new(), pan_automation: Vec::new(),
            track_delay_automation: Vec::new(), plugin_types: Vec::new(), plugin_bypasses: Vec::new(),
            plugin_parameter_values: Vec::new(), plugin_states: Vec::new(), plugin_gui_states: Vec::new(),
            plugin_state_versions: Vec::new(), sandbox_plugin_paths: Vec::new(),
            sandbox_plugin_states: Vec::new(), sandbox_plugin_state_versions: Vec::new(),
        });
        self.metadata.tracks_count = self.tracks.len() as u32;
        Ok(id)
    }

    /// Rename an existing track while preserving its stable identity.
    pub fn set_track_name(&mut self, track_id: u32, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() || name.len() > 1024 {
            bail!("track name is invalid");
        }
        let track = self
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or_else(|| anyhow::anyhow!("track was not found"))?;
        track.name = name;
        Ok(())
    }

    /// Remove a track and all project records owned by it.
    pub fn remove_track(&mut self, track_id: u32) -> Result<()> {
        let before = self.tracks.len();
        self.tracks.retain(|track| track.id != track_id);
        if self.tracks.len() == before {
            bail!("track was not found");
        }
        self.aux_track_ids.retain(|id| *id != track_id);
        self.regions.retain(|region| region.track_id != track_id);
        self.midi_notes.retain(|note| note.track_id != track_id);
        self.render_targets.retain(|target| target.source_id != track_id);
        self.audio_routes.retain(|route| {
            route.source_id != track_id && route.destination_id != track_id
        });
        self.sidechain_routes.retain(|route| {
            route.source_id != track_id && route.destination_id != track_id
        });
        self.feedback_routes.retain(|route| {
            route.source_id != track_id && route.destination_id != track_id
        });
        self.metadata.tracks_count = self.tracks.len() as u32;
        Ok(())
    }

    pub fn insert_builtin_plugin(&mut self, track_id: u32, plugin_type: u32) -> Result<u32> {
        if plugin_type == 0 { bail!("plugin type must be non-zero"); }
        let track = self.tracks.iter_mut().find(|track| track.id == track_id)
            .ok_or_else(|| anyhow::anyhow!("track was not found"))?;
        let slot = track.plugin_types.len() as u32;
        if slot >= 1024 { bail!("plugin slot limit exceeded"); }
        track.plugin_types.push(plugin_type);
        track.plugin_bypasses.push(false);
        track.plugin_parameter_values.push(Vec::new());
        track.plugin_states.push(Vec::new());
        track.plugin_gui_states.push(Vec::new());
        track.plugin_state_versions.push(1);
        Ok(slot)
    }

    pub fn save_atomic(&self, path: &str) -> Result<()> {
        self.validate()?;
        SovereignPersistence::save_document(path, self)
    }

    pub fn load(path: &str) -> Result<Self> {
        let document: Self = SovereignPersistence::load_json(path)?;
        document.validate()?;
        Ok(document)
    }
}
