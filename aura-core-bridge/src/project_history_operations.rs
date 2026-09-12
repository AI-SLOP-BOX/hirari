impl ProjectHistoryStore {
    pub fn tag(&self, name: &str, commit_id: Option<&str>) -> Result<ProjectRef> {
        validate_ref_name(name)?;
        let _lock = HistoryLock::acquire(&self.root)?;
        let commit_id = match commit_id {
            Some(id) => {
                self.read_commit(id)?;
                id.to_owned()
            }
            None => self
                .status()?
                .head
                .ok_or_else(|| anyhow::anyhow!("cannot tag without a commit"))?,
        };
        atomic_write(
            &self.root.join("refs/tags").join(name),
            commit_id.as_bytes(),
        )?;
        Ok(ProjectRef {
            name: name.to_owned(),
            commit_id,
        })
    }

    pub fn load_commit(&self, commit_id: &str) -> Result<ProjectDocument> {
        let commit = self.read_commit(commit_id)?;
        let project_id = read_to_string(self.root.join("PROJECT_ID"))?
            .trim()
            .to_owned();
        if commit.project_id != project_id {
            bail!("commit belongs to a different project history");
        }
        let bytes = read(
            self.root
                .join("snapshots")
                .join(format!("{}.json", commit.snapshot_hash)),
        )?;
        if content_hash(&bytes) != commit.snapshot_hash {
            bail!("snapshot checksum mismatch for commit {commit_id}");
        }
        let project: ProjectDocument =
            serde_json::from_slice(&bytes).context("invalid project snapshot")?;
        project.validate()?;
        Ok(project)
    }

    fn validate_working_project_identity(&self, project_file: &Path) -> Result<()> {
        if !project_file.exists() {
            return Ok(());
        }
        let expected = read_to_string(self.root.join("PROJECT_ID"))?
            .trim()
            .to_owned();
        let current = ProjectDocument::load(
            project_file
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?,
        )?;
        if current.project_id != expected {
            bail!(
                "working project identity does not match this history (expected {expected}, found {})",
                current.project_id
            );
        }
        Ok(())
    }

    pub fn log(&self, limit: usize) -> Result<Vec<ProjectCommit>> {
        let mut commits = Vec::new();
        let mut cursor = self.status()?.head;
        for _ in 0..limit.clamp(1, 512) {
            let Some(id) = cursor else {
                break;
            };
            let commit = self.read_commit(&id)?;
            cursor = commit.parent.clone();
            commits.push(commit);
        }
        Ok(commits)
    }

    pub fn diff_commits(&self, from: &str, to: &str) -> Result<SnapshotDiff> {
        let before = serde_json::to_value(self.load_commit(from)?)?;
        let after = serde_json::to_value(self.load_commit(to)?)?;
        let mut changed_sections = Vec::new();
        let mut changes = Vec::new();
        for section in [
            "metadata",
            "sample_rate",
            "tempo_events",
            "time_signature_events",
            "tracks",
            "regions",
            "plugin_instances",
            "midi_learn_mappings",
            "midi_notes",
            "chord_track",
            "macro_mappings",
            "warp_markers",
            "render_targets",
            "openutau_vocals",
            "freeze_artifacts",
            "sidechain_routes",
            "feedback_routes",
            "audio_routes",
        ] {
            if before.get(section) != after.get(section) {
                let before_hash = content_hash(&serde_json::to_vec(&before[section])?);
                let after_hash = content_hash(&serde_json::to_vec(&after[section])?);
                changed_sections.push(section.to_owned());
                changes.push(SnapshotChange {
                    section: section.to_owned(),
                    before_hash,
                    after_hash,
                    entity_id: None,
                    operation: "changed".to_owned(),
                    fields: Vec::new(),
                });
                append_entity_changes(section, &before[section], &after[section], &mut changes);
            }
        }
        Ok(SnapshotDiff {
            from: from.to_owned(),
            to: to.to_owned(),
            changed_sections,
            changes,
        })
    }

    pub fn cherry_pick(
        &self,
        current: &ProjectDocument,
        commit_id: &str,
        sections: &[String],
    ) -> Result<ProjectDocument> {
        if sections.is_empty() || sections.len() > 16 {
            bail!("cherry-pick requires 1..=16 sections");
        }
        let source = self.load_commit(commit_id)?;
        let mut result = current.clone();
        for section in sections {
            match section.as_str() {
                "metadata" => result.metadata = source.metadata.clone(),
                "sample_rate" => result.sample_rate = source.sample_rate,
                "tempo_events" => result.tempo_events = source.tempo_events.clone(),
                "time_signature_events" => {
                    result.time_signature_events = source.time_signature_events.clone()
                }
                "tracks" => result.tracks = source.tracks.clone(),
                "regions" => result.regions = source.regions.clone(),
                "plugin_instances" => result.plugin_instances = source.plugin_instances.clone(),
                "midi_learn_mappings" => {
                    result.midi_learn_mappings = source.midi_learn_mappings.clone()
                }
                "midi_notes" => result.midi_notes = source.midi_notes.clone(),
                "chord_track" => result.chord_track = source.chord_track.clone(),
                "macro_mappings" => result.macro_mappings = source.macro_mappings.clone(),
                "warp_markers" => result.warp_markers = source.warp_markers.clone(),
                "render_targets" => result.render_targets = source.render_targets.clone(),
                "openutau_vocals" => result.openutau_vocals = source.openutau_vocals.clone(),
                "freeze_artifacts" => result.freeze_artifacts = source.freeze_artifacts.clone(),
                "sidechain_routes" => result.sidechain_routes = source.sidechain_routes.clone(),
                "feedback_routes" => result.feedback_routes = source.feedback_routes.clone(),
                "audio_routes" => result.audio_routes = source.audio_routes.clone(),
                other => bail!("unknown cherry-pick section: {other}"),
            }
        }
        result.metadata.tracks_count = result.tracks.len() as u32;
        result.validate()?;
        Ok(result)
    }

    fn current_branch(&self) -> Result<String> {
        let branch = read_to_string(self.root.join("HEAD"))?.trim().to_owned();
        validate_ref_name(&branch)?;
        Ok(branch)
    }

    fn asset_manifest_hash(&self, project: &ProjectDocument) -> Result<String> {
        // Include every external audio file that can affect the audible
        // result. Region paths alone miss OpenUtau source/render pairs.
        let mut paths = project
            .regions
            .iter()
            .map(|region| region.path.clone())
            .collect::<Vec<_>>();
        for vocal in &project.openutau_vocals {
            paths.push(vocal.source_path.clone());
            paths.push(vocal.rendered_audio_path.clone());
        }
        paths.sort();
        paths.dedup();
        let mut manifest = Vec::with_capacity(paths.len());
        for path in paths {
            let requested = Path::new(&path);
            let absolute = if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                self.project_root.join(requested)
            };
            let display_path = absolute
                .strip_prefix(&self.project_root)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or(path);
            let is_symlink = std::fs::symlink_metadata(&absolute)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false);
            let fingerprint = match fingerprint_asset_file(&absolute) {
                Ok((byte_count, content_hash, sample_rate, channels, frame_count)) => {
                    AssetFingerprint {
                        path: display_path,
                        exists: true,
                        is_symlink,
                        bytes: byte_count,
                        content_hash,
                        sample_rate,
                        channels,
                        frame_count,
                    }
                }
                Err(_) => AssetFingerprint {
                    path: display_path,
                    exists: false,
                    is_symlink,
                    bytes: 0,
                    content_hash: content_hash(&[]),
                    sample_rate: None,
                    channels: None,
                    frame_count: None,
                },
            };
            manifest.push(fingerprint);
        }
        Ok(content_hash(&serde_json::to_vec(&manifest)?))
    }

    fn ensure_project_identity(&self, project_id: &str) -> Result<()> {
        let identity_path = self.root.join("PROJECT_ID");
        let stored = read_to_string(&identity_path)?.trim().to_owned();
        if stored == project_id {
            return Ok(());
        }
        // `open()` creates the marker before the first commit.  Adopt the
        // document's UUID exactly once while the history is still empty;
        // after a commit, replacing a project at the same path fails closed.
        let has_commits = std::fs::read_dir(self.root.join("commits"))
            .map(|entries| entries.flatten().next().is_some())
            .unwrap_or(true);
        if !has_commits && self.read_ref("heads", &self.current_branch()?)?.is_none() {
            atomic_write(&identity_path, project_id.as_bytes())?;
            return Ok(());
        }
        bail!("project UUID does not match this history store")
    }

    fn read_ref(&self, group: &str, name: &str) -> Result<Option<String>> {
        let path = self.root.join("refs").join(group).join(name);
        if !path.exists() {
            return Ok(None);
        }
        let value = read_to_string(path)?.trim().to_owned();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    fn read_commit(&self, id: &str) -> Result<ProjectCommit> {
        validate_commit_id(id)?;
        let commit: ProjectCommit =
            serde_json::from_slice(&read(self.root.join("commits").join(format!("{id}.json")))?)?;
        if commit.schema_version != HISTORY_SCHEMA_VERSION || commit.commit_id != id {
            bail!("invalid commit metadata");
        }
        if commit.project_id.is_empty()
            || commit.project_format_version == 0
            || commit.snapshot_hash.len() != 64
            || commit.asset_manifest_hash.len() != 64
            || commit.plugin_manifest_hash.len() != 64
            || commit.platform.trim().is_empty()
        {
            bail!("incomplete commit metadata");
        }
        let material = commit_id_material(&commit);
        if content_hash(material.as_bytes()) != id {
            bail!("commit checksum mismatch");
        }
        Ok(commit)
    }
}
