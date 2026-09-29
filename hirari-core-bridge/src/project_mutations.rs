fn media_sha256(path: &std::path::Path) -> Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let sha256 = format!("{digest:x}");
    let checksum = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
    Ok((sha256, checksum))
}

impl ProjectDocument {
    /// Lists unique project media references that cannot currently be opened
    /// as regular files. Returned strings are the values stored in the
    /// document, so callers can present them and pass them back to
    /// `relink_missing_media` without guessing which field owned the path.
    pub fn missing_media_references(&self, project_path: &str) -> Result<Vec<String>> {
        let project_file = std::path::Path::new(project_path);
        let parent = project_file
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let parent = std::fs::canonicalize(parent)?;
        let mut references = std::collections::BTreeSet::new();
        references.extend(self.regions.iter().map(|region| region.path.as_str()));
        references.extend(
            self.freeze_artifacts
                .iter()
                .map(|artifact| artifact.path.as_str()),
        );
        for vocal in &self.openutau_vocals {
            references.insert(vocal.source_path.as_str());
            references.insert(vocal.rendered_audio_path.as_str());
        }

        let mut missing = Vec::new();
        for reference in references {
            if reference.trim().is_empty() {
                continue;
            }
            let authored = std::path::Path::new(reference);
            if !authored.is_absolute()
                && authored
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
            {
                // Traversal is a project error, not a missing-media case that
                // should be silently repaired by searching elsewhere.
                continue;
            }
            let candidate = if authored.is_absolute() {
                authored.to_path_buf()
            } else {
                parent.join(authored)
            };
            let valid = std::fs::symlink_metadata(&candidate)
                .map(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
                .unwrap_or(false);
            if !valid {
                missing.push(reference.to_owned());
            }
        }
        Ok(missing)
    }

    /// Explicitly replaces one missing media reference after checking every
    /// persisted identity constraint associated with it. Freeze and OpenUtau
    /// files have saved checksums and cannot be substituted with different
    /// content; ordinary audio regions are confirmed by the user's selection.
    pub fn relink_missing_media(
        &mut self,
        project_path: &str,
        missing_reference: &str,
        replacement_path: &str,
    ) -> Result<usize> {
        let missing = self.missing_media_references(project_path)?;
        if !missing.iter().any(|path| path == missing_reference) {
            bail!("media reference is not missing from this project");
        }
        let replacement = std::path::Path::new(replacement_path);
        let metadata = std::fs::symlink_metadata(replacement)
            .context("selected media file cannot be inspected")?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!("selected media must be a regular, non-symlink file");
        }
        let replacement = std::fs::canonicalize(replacement)?;
        let (sha256, checksum) =
            media_sha256(&replacement).context("selected media file cannot be read")?;

        for artifact in self
            .freeze_artifacts
            .iter()
            .filter(|artifact| artifact.path == missing_reference)
        {
            if checksum != artifact.content_checksum {
                bail!("selected freeze media does not match its saved checksum");
            }
        }
        for vocal in &self.openutau_vocals {
            let expected_hashes = [
                (vocal.source_path == missing_reference).then_some(vocal.source_hash.as_str()),
                (vocal.rendered_audio_path == missing_reference)
                    .then_some(vocal.rendered_audio_hash.as_str()),
            ];
            if expected_hashes
                .into_iter()
                .flatten()
                .filter(|expected| !expected.is_empty())
                .any(|expected| sha256 != expected)
            {
                bail!("selected OpenUtau media does not match its saved checksum");
            }
        }

        let project_parent = std::path::Path::new(project_path)
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let project_parent = std::fs::canonicalize(project_parent)?;
        let openutau_reference = self.openutau_vocals.iter().any(|vocal| {
            vocal.source_path == missing_reference || vocal.rendered_audio_path == missing_reference
        });
        let stored_replacement = if openutau_reference {
            replacement.to_string_lossy().into_owned()
        } else {
            replacement
                .strip_prefix(&project_parent)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or_else(|_| replacement.to_string_lossy().into_owned())
        };
        let mut updated = 0;
        for region in &mut self.regions {
            if region.path == missing_reference {
                region.path.clone_from(&stored_replacement);
                updated += 1;
            }
        }
        for artifact in &mut self.freeze_artifacts {
            if artifact.path == missing_reference {
                artifact.path.clone_from(&stored_replacement);
                updated += 1;
            }
        }
        for vocal in &mut self.openutau_vocals {
            if vocal.source_path == missing_reference {
                vocal.source_path.clone_from(&stored_replacement);
                updated += 1;
            }
            if vocal.rendered_audio_path == missing_reference {
                vocal.rendered_audio_path.clone_from(&stored_replacement);
                updated += 1;
            }
        }
        if updated == 0 {
            bail!("media reference is not used by this project");
        }
        self.validate()?;
        Ok(updated)
    }

    /// Loads a project document and upgrades supported older contracts before
    /// validation so newly expanded automation domains remain backward
    /// compatible with projects written by contract version 1.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        let mut document: Self = serde_json::from_slice(bytes)
            .context("project document JSON is malformed")?;
        match document.contract_version {
            1 => document.contract_version = crate::project_contracts::PROJECT_CONTRACT_VERSION,
            crate::project_contracts::PROJECT_CONTRACT_VERSION => {}
            version => bail!("unsupported project contract version {version}"),
        }
        document.validate()?;
        Ok(document)
    }

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
            track_delay_automation: Vec::new(), plugin_automation: Vec::new(),
            plugin_types: Vec::new(), plugin_bypasses: Vec::new(),
            plugin_parameter_values: Vec::new(), plugin_states: Vec::new(), plugin_gui_states: Vec::new(),
            plugin_state_versions: Vec::new(), sandbox_plugin_paths: Vec::new(),
            sandbox_plugin_states: Vec::new(), sandbox_plugin_state_versions: Vec::new(),
            expression_map: Vec::new(),
            expression_map_pro: None,
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
        self.audio_input_assignments
            .retain(|assignment| assignment.track_id != track_id);
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
        let bytes = std::fs::read(path)?;
        Self::from_json_bytes(&bytes)
    }
}
