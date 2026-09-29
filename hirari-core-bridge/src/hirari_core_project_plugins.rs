impl HirariCore {
    pub fn set_offline_render_target(&self, track_id: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_offline_render_target(track_id))
    }

    pub fn clear_offline_render_target(&self) {
        if let Some(engine) = self.engine.as_ref() {
            engine.clear_offline_render_target();
        }
    }

    pub fn set_offline_render_tail_seconds(&self, seconds: f32) {
        if let Some(engine) = self.engine.as_ref() {
            engine.set_offline_render_tail_seconds(seconds);
        }
    }

    pub fn set_offline_render_options(&self, pre_fader: bool, include_inserts: bool) {
        if let Some(engine) = self.engine.as_ref() {
            engine.set_offline_render_options(pre_fader, include_inserts);
        }
    }

    pub fn plugin_latency_snapshot_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\"}".to_owned();
        };
        let tracks: serde_json::Value = serde_json::from_str(&engine.get_project_layout_json())
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        let mut entries = Vec::new();
        if let Some(items) = tracks.as_array() {
            for item in items {
                if let Some(id) = item
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                {
                    entries.push(serde_json::json!({"track_id":id,"latency_ms":engine.get_track_latency_ms(id)}));
                }
            }
        }
        serde_json::json!({"ok":true,"master_latency_ms":engine.get_latency_ms(),"tracks":entries})
            .to_string()
    }

    /// Exports the canonical MIDI model as a multi-part MusicXML score.
    /// Beats are derived through the active tempo map, and track names,
    /// note durations, velocities, lyrics, meter changes, and tempo anchors
    /// are preserved. Tempo ramp shapes are flattened to their event values.
    pub fn export_musicxml(&self, path: &str) -> anyhow::Result<usize> {
        if path.trim().is_empty() {
            return Err(anyhow::anyhow!("MusicXML path is required"));
        }
        let notes = self
            .scheduled_midi_notes
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI note lock poisoned"))?
            .clone();
        let mut layouts = std::collections::HashMap::<u32, String>::new();
        let layout: serde_json::Value = serde_json::from_str(&self.get_project_layout_json())
            .map_err(|error| anyhow::anyhow!("project layout is invalid: {error}"))?;
        if let Some(tracks) = layout.as_array() {
            for track in tracks {
                let Some(id) = track.get("id").and_then(serde_json::Value::as_u64) else { continue };
                let name = track.get("name").and_then(serde_json::Value::as_str).unwrap_or("MIDI Part");
                layouts.insert(id as u32, name.to_owned());
            }
        }
        let mut grouped = std::collections::BTreeMap::<u32, Vec<crate::musicxml::MusicXmlExportNote>>::new();
        for note in &notes {
            let end_sample = note.start_sample.saturating_add(note.length_samples);
            let start_beat = self.samples_to_beats(note.start_sample);
            let end_beat = self.samples_to_beats(end_sample);
            grouped.entry(note.track_id).or_default().push(crate::musicxml::MusicXmlExportNote {
                start_beat,
                length_beats: (end_beat - start_beat).max(1.0 / 960.0),
                pitch: note.pitch,
                velocity: note.velocity,
                voice: note.midi_channel,
                lyric: note.lyric.clone(),
            });
        }
        let export_parts = grouped
            .into_iter()
            .map(|(track_id, notes)| crate::musicxml::MusicXmlExportPart {
                name: layouts.get(&track_id).cloned().unwrap_or_else(|| format!("MIDI {track_id}")),
                notes,
            })
            .collect::<Vec<_>>();
        if export_parts.is_empty() {
            return Err(anyhow::anyhow!("project contains no MIDI notes to export"));
        }
        let mut time_signatures = Vec::new();
        let mut tempo_events = Vec::new();
        if let Some(engine) = self.engine.as_ref() {
            let packed = engine.get_time_signature_events();
            if packed.len() % 3 != 0 {
                return Err(anyhow::anyhow!("project time-signature map is invalid"));
            }
            for event in packed.chunks_exact(3) {
                let numerator = event[1] as u8;
                let denominator = event[2] as u8;
                time_signatures.push(crate::musicxml::MusicXmlTimeSignature {
                    beat: event[0], numerator, denominator,
                });
            }
            let packed = engine.get_tempo_events();
            if packed.len() % 3 != 0 {
                return Err(anyhow::anyhow!("project tempo map is invalid"));
            }
            for event in packed.chunks_exact(3) {
                tempo_events.push(crate::musicxml::MusicXmlExportTempoEvent {
                    beat: event[0],
                    bpm: event[1],
                });
            }
        }
        let count = export_parts.iter().map(|part| part.notes.len()).sum();
        let title = std::path::Path::new(path)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Hirari Score");
        let xml = crate::musicxml::export_musicxml(&export_parts, &time_signatures, &tempo_events, title)
            .map_err(anyhow::Error::msg)?;
        crate::musicxml::write_musicxml(std::path::Path::new(path), &xml)
            .map_err(anyhow::Error::msg)?;
        Ok(count)
    }

    /// Collects the current project file and every referenced media asset into
    /// a self-contained archive directory. Sources are never modified; name
    /// collisions are disambiguated deterministically and a manifest records
    /// the original-to-archived mapping for relinking.
    pub fn archive_project(&self, project_path: &str, archive_dir: &str) -> anyhow::Result<usize> {
        use std::collections::HashMap;
        use std::fs;
        use std::path::{Path, PathBuf};
        if project_path.trim().is_empty() || archive_dir.trim().is_empty() {
            return Err(anyhow::anyhow!("project and archive paths are required"));
        }
        let project = Path::new(project_path);
        if !project.is_file() {
            return Err(anyhow::anyhow!("project file does not exist"));
        }
        let root = Path::new(archive_dir);
        fs::create_dir_all(root)?;
        let assets = root.join("Assets");
        fs::create_dir_all(&assets)?;
        let layout: serde_json::Value = serde_json::from_str(&self.get_project_layout_json())
            .map_err(|_| anyhow::anyhow!("project layout is invalid"))?;
        let mut seen = HashMap::<String, PathBuf>::new();
        let mut manifest = String::from("source\tarchived\tstatus\n");
        if let Some(tracks) = layout.as_array() {
            for track in tracks {
                if let Some(regions) = track.get("regions").and_then(|v| v.as_array()) {
                    for region in regions {
                        let Some(source) = region
                            .get("path")
                            .and_then(|v| v.as_str())
                            .filter(|p| !p.is_empty())
                        else {
                            continue;
                        };
                        let source_path = Path::new(source);
                        let Some(file_name) = source_path.file_name().and_then(|n| n.to_str())
                        else {
                            continue;
                        };
                        let mut target = assets.join(file_name);
                        if let Some(existing) = seen.get(source) {
                            target = existing.clone();
                        } else {
                            let mut suffix = 1u32;
                            while target.exists() {
                                let stem = source_path
                                    .file_stem()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("asset");
                                let ext = source_path
                                    .extension()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("");
                                target = assets.join(if ext.is_empty() {
                                    format!("{stem}-{suffix}")
                                } else {
                                    format!("{stem}-{suffix}.{ext}")
                                });
                                suffix += 1;
                            }
                            if source_path.is_file() {
                                fs::copy(source_path, &target)?;
                                seen.insert(source.to_owned(), target.clone());
                            }
                        }
                        let status = if target.is_file() {
                            "collected"
                        } else {
                            "missing"
                        };
                        manifest.push_str(&format!(
                            "{}\t{}\t{}\n",
                            source,
                            target.strip_prefix(root).unwrap_or(&target).display(),
                            status
                        ));
                    }
                }
            }
        }
        fs::copy(
            project,
            root.join(
                project
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("project.hirari")),
            ),
        )?;
        fs::write(root.join("archive-manifest.tsv"), manifest)?;
        Ok(seen.len())
    }

    /// Read-only canonical native route snapshot for CLI, UI, and automation.
    pub fn audio_routes_json(&self) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.get_routing_snapshot_json().to_string())
            .unwrap_or_else(|| "[]".to_owned())
    }

    /// Returns the native canonical WAVE/WAVE64 parser diagnostic. This keeps
    /// UI callers from duplicating format parsing and preserves native error
    /// reasons across the CXX boundary.
    pub fn read_wav_diagnostic_json(&self, path: &str, format: u32) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.read_wav_diagnostic_json(path, format).to_string())
            .unwrap_or_else(|| "{\"ok\":false,\"code\":\"engine_unavailable\"}".to_owned())
    }

    /// Validates a published WAV without touching the native graph. When
    /// `require_audio` is true, the data chunk must contain at least one
    /// non-zero byte; callers rendering an intentionally silent project can
    /// pass false.
    pub fn validate_render_output(&self, path: &str, require_audio: bool) -> bool {
        if !is_wav_output_path(path) {
            return false;
        }
        validate_render_wave_file(std::path::Path::new(path), require_audio).unwrap_or(false)
    }

    pub fn validate_render_output_diagnostic_json(
        &self,
        path: &str,
        require_audio: bool,
    ) -> String {
        let result = if !is_wav_output_path(path) {
            crate::bridge_error::BridgeError::new(
                "invalid_render_path",
                "render output must be a WAV path",
            )
        } else {
            match std::fs::File::open(path) {
                Ok(_) => {}
                Err(error) => {
                    return serde_json::json!({
                        "code": "render_output_unreadable",
                        "message": error.to_string(),
                        "retryable": true,
                    })
                    .to_string();
                }
            }
            let valid = validate_render_wave_file(std::path::Path::new(path), require_audio)
                .unwrap_or(false);
            if valid {
                return serde_json::json!({
                    "ok": true,
                    "operation": "validate_render_output",
                    "path": path,
                    "require_audio": require_audio,
                })
                .to_string();
            }
            crate::bridge_error::BridgeError::new(
                "invalid_render_output",
                "WAV output failed structural or sample validation",
            )
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn bounce_project(&self, path: &str, format: u32) -> bool {
        if !is_wav_output_path(path) {
            return false;
        }
        if format > 3 {
            return false;
        }
        let completed = self
            .engine
            .as_ref()
            .is_some_and(|e| e.bounce_project(path, format));
        if !completed {
            return false;
        }

        // Format 0 is the native WAV path. Do not report success until the
        // output is published and has a structurally valid RIFF/WAVE header.
        if format == 0 || format == 1 || format == 2 || format == 3 {
            return self.validate_render_output(path, false);
        }
        std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    }

    /// Structured bounce result for CLI/automation callers. The legacy bool
    /// API remains available for existing UI bindings.
    pub fn bounce_project_diagnostic_json(&self, path: &str, format: u32) -> String {
        if !is_wav_output_path(path) {
            return "{\"code\":\"invalid_render_path\",\"retryable\":false}".to_owned();
        }
        if format > 3 {
            return "{\"code\":\"unsupported_render_format\",\"retryable\":false}".to_owned();
        }
        self.engine.as_ref().map_or_else(
            || "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned(),
            |engine| {
                engine
                    .bounce_project_diagnostic_json(path, format)
                    .to_string()
            },
        )
    }

    /// Exposes the render graph's currently addressable targets to CLI, UI,
    /// and automation clients. This is deliberately derived from the native
    /// layout snapshot so an agent cannot render a stale or invented track.
    pub fn render_target_catalog_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let layout: serde_json::Value =
            match serde_json::from_str(engine.get_project_layout_json().as_str()) {
                Ok(value) => value,
                Err(_) => {
                    return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}"
                        .to_owned();
                }
            };
        let Some(tracks) = layout.as_array() else {
            return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}".to_owned();
        };
        let mut targets = Vec::with_capacity(tracks.len() + 1);
        for track in tracks {
            let Some(source_id) = track.get("id").and_then(serde_json::Value::as_u64) else {
                return "{\"code\":\"invalid_track_id\",\"retryable\":false}".to_owned();
            };
            if source_id == 0 || source_id > u32::MAX as u64 {
                return "{\"code\":\"invalid_track_id\",\"retryable\":false}".to_owned();
            }
            let name = track
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Track");
            let kind = match track.get("type").and_then(serde_json::Value::as_str) {
                Some("Bus") if name.starts_with("Aux ") => "Aux",
                Some("Bus") => "Bus",
                Some("Midi") => "Midi",
                Some("Instrument") => "Instrument",
                Some("Vocal") => "Vocal",
                _ => "Track",
            };
            targets.push(serde_json::json!({
                "target_id": format!("track:{source_id}"),
                "kind": kind,
                "source_id": source_id,
                "name": name,
                "pre_fader": false,
                "include_inserts": true,
                "include_tail": true,
            }));
        }
        targets.push(serde_json::json!({
            "target_id": "master",
            "kind": "Master",
            "source_id": 0,
            "name": "Master",
            "pre_fader": false,
            "include_inserts": true,
            "include_tail": true,
        }));
        serde_json::json!({
            "ok": true,
            "operation": "render_target_catalog",
            "targets": targets,
        })
        .to_string()
    }

    /// Render one WAV per project track by selecting each track through a
    /// render-only solo state. The selection is never added to Undo/Redo and
    /// the original solo state is restored even when a stem fails.
    pub fn bounce_stems_diagnostic_json(
        &self,
        output_dir: &str,
        format: u32,
        requested_track_ids: &[u32],
        tail_seconds: f32,
        pre_fader: bool,
        include_inserts: bool,
    ) -> String {
        if format > 3 || !tail_seconds.is_finite() || !(0.0..=60.0).contains(&tail_seconds) {
            return "{\"code\":\"unsupported_render_format\",\"retryable\":false}".to_owned();
        }
        let output_dir_path = std::path::Path::new(output_dir);
        if !output_dir_path.is_dir() {
            return "{\"code\":\"invalid_stem_output_directory\",\"retryable\":false}".to_owned();
        }
        let Ok(_transaction) = self.project_transaction.lock() else {
            return "{\"code\":\"project_transaction_busy\",\"retryable\":true}".to_owned();
        };
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let layout: serde_json::Value =
            match serde_json::from_str(engine.get_project_layout_json().as_str()) {
                Ok(layout) => layout,
                Err(_) => {
                    return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}"
                        .to_owned();
                }
            };
        let Some(tracks) = layout.as_array() else {
            return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}".to_owned();
        };
        if tracks.is_empty() {
            return "{\"code\":\"no_tracks_for_stem_export\",\"retryable\":false}".to_owned();
        }

        let requested: std::collections::HashSet<u32> =
            requested_track_ids.iter().copied().collect();
        if requested.len() != requested_track_ids.len() || requested.iter().any(|id| *id == 0) {
            return "{\"code\":\"invalid_stem_track_selection\",\"retryable\":false}".to_owned();
        }
        let selected_tracks: Vec<&serde_json::Value> = if requested.is_empty() {
            tracks.iter().collect()
        } else {
            tracks
                .iter()
                .filter(|track| {
                    track
                        .get("id")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|id| requested.contains(&(id as u32)))
                })
                .collect()
        };
        if !requested.is_empty() && selected_tracks.len() != requested.len() {
            return serde_json::json!({
                "code": "stem_track_not_found",
                "retryable": false,
                "requested_track_ids": requested_track_ids,
            })
            .to_string();
        }

        let mut track_state = Vec::with_capacity(selected_tracks.len());
        let mut names = std::collections::HashSet::with_capacity(tracks.len());
        for track in selected_tracks {
            let Some(track_id) = track
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .map(|id| id as u32)
            else {
                return "{\"code\":\"invalid_track_id\",\"retryable\":false}".to_owned();
            };
            let raw_name = track
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("track");
            let base: String = raw_name
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                        character
                    } else {
                        '_'
                    }
                })
                .collect();
            let base = if base.is_empty() {
                format!("track-{track_id}")
            } else {
                base
            };
            let mut name = base.clone();
            let mut suffix = 2usize;
            while !names.insert(name.clone()) {
                name = format!("{base}-{suffix}");
                suffix += 1;
            }
            track_state.push((track_id, output_dir_path.join(format!("{name}.wav"))));
        }

        let mut outputs = Vec::with_capacity(track_state.len());
        // Reject the whole batch before changing any render-only solo state.
        // Otherwise a pre-existing second stem would cause the first stem to
        // render and then be deleted during rollback, making a dry-run look
        // destructive and needlessly exercising the audio graph.
        if let Some(existing) = track_state
            .iter()
            .find(|(_, output_path)| output_path.exists())
        {
            return serde_json::json!({
                "code": "stem_output_exists",
                "path": existing.1,
                "retryable": false,
                "restored_render_target": true,
            })
            .to_string();
        }
        let mut failure: Option<String> = None;
        engine.set_offline_render_tail_seconds(tail_seconds);
        engine.set_offline_render_options(pre_fader, include_inserts);
        for (target_id, output_path) in &track_state {
            if !engine.set_offline_render_target(*target_id)
                || !engine.bounce_project(output_path.to_string_lossy().as_ref(), format)
                || !self.validate_render_output(output_path.to_string_lossy().as_ref(), false)
            {
                failure = Some(format!("stem render failed for track {target_id}"));
                break;
            }
            outputs.push(output_path.to_string_lossy().to_string());
        }
        engine.clear_offline_render_target();
        if let Some(message) = failure {
            for path in &outputs {
                let _ = std::fs::remove_file(path);
            }
            return serde_json::json!({
                "code": "stem_render_failed",
                "message": message,
                "retryable": true,
                "restored_render_target": true,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "bounce_stems",
            "format": format,
            "tail_seconds": tail_seconds,
            "pre_fader": pre_fader,
            "include_inserts": include_inserts,
            "outputs": outputs,
            "restored_render_target": true,
        })
        .to_string()
    }

    // --- RUST-POWERED PERSISTENCE (Point 3) ---
}
