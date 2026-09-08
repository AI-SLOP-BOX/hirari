use crate::project_contracts::{RenderTargetContract, RenderTargetKind};

impl AuraCore {
    /// Returns per-track and master plugin latency for MixConsole display.
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

    /// Exports the canonical MIDI note model as a portable MusicXML score.
    /// Sample positions are quantized to divisions while preserving lyrics.
    pub fn export_musicxml(&self, path: &str) -> anyhow::Result<usize> {
        use std::fs;
        if path.trim().is_empty() {
            return Err(anyhow::anyhow!("MusicXML path is required"));
        }
        let notes = self
            .scheduled_midi_notes
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI note lock poisoned"))?
            .clone();
        let rate = self
            .engine
            .as_ref()
            .map(|e| e.get_sample_rate())
            .unwrap_or(48_000.0)
            .max(1.0);
        let divisions = 480u64;
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<score-partwise version=\"3.1\"><work><work-title>Aura Score</work-title></work><part-list><score-part id=\"P1\"><part-name>Aura</part-name></score-part></part-list><part id=\"P1\">");
        let mut sorted = notes;
        sorted.sort_by_key(|n| n.start_sample);
        let mut measure = 1u64;
        for note in &sorted {
            let start = ((note.start_sample as f64 / rate) * 2.0 * divisions as f64).round() as u64;
            let duration = (((note.length_samples as f64 / rate) * 2.0 * divisions as f64).round()
                as u64)
                .max(1);
            xml.push_str(&format!("<measure number=\"{}\"><note><pitch><step>{}</step><octave>{}</octave></pitch><duration>{}</duration><voice>1</voice><type>quarter</type><velocity>{}</velocity>{}</note></measure>", measure, ["C","C","D","D","E","F","F","G","G","A","A","B"][(note.pitch % 12) as usize], note.pitch / 12, duration, note.velocity, if note.lyric.is_empty() { String::new() } else { format!("<lyric><text>{}</text></lyric>", note.lyric.replace('&', "&amp;").replace('<', "&lt;")) }));
            measure = (start / (divisions * 8)).saturating_add(1);
        }
        xml.push_str("</part></score-partwise>\n");
        fs::write(path, xml)?;
        Ok(sorted.len())
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
                    .unwrap_or_else(|| std::ffi::OsStr::new("project.aura")),
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
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        if bytes.len() >= 40 && bytes[0..4] == *b"RIFF" && bytes[24..28] == *b"WAVE" {
            let Ok((_, _, samples)) =
                crate::export::read_wave64_float32(std::path::Path::new(path))
            else {
                return false;
            };
            return !require_audio || samples.iter().any(|sample| *sample != 0.0);
        }
        if !valid_pcm_or_float_wav(&bytes) {
            return false;
        }
        if !float_wav_samples_are_finite(&bytes) {
            return false;
        }
        !require_audio
            || bytes
                .get(12..)
                .is_some_and(|payload| payload.iter().any(|byte| *byte != 0))
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
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return serde_json::json!({
                        "code": "render_output_unreadable",
                        "message": error.to_string(),
                        "retryable": true,
                    })
                    .to_string();
                }
            };
            let valid = if bytes.len() >= 40 && bytes[0..4] == *b"RIFF" && bytes[24..28] == *b"WAVE"
            {
                crate::export::read_wave64_float32(std::path::Path::new(path))
                    .map(|(_, _, samples)| {
                        !require_audio || samples.iter().any(|sample| *sample != 0.0)
                    })
                    .unwrap_or(false)
            } else {
                valid_pcm_or_float_wav(&bytes)
                    && float_wav_samples_are_finite(&bytes)
                    && (!require_audio
                        || bytes
                            .get(12..)
                            .is_some_and(|payload| payload.iter().any(|byte| *byte != 0)))
            };
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
        if format > 1 {
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
        if format == 0 || format == 1 {
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
        if format > 1 {
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
                    return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}".to_owned()
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
        if format > 1 || !tail_seconds.is_finite() || !(0.0..=60.0).contains(&tail_seconds) {
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
                    return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}".to_owned()
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
        document.save_atomic(path)?;
        // Control Room is a session-scoped monitor graph rather than part of
        // the native track layout. Persist it beside the project document so
        // save/load cannot silently reset monitor selection, cues, or the
        // reference track.
        let control_room_path = format!("{path}.control-room.json");
        let control_room = self
            .control_room
            .lock()
            .map_err(|_| anyhow::anyhow!("control room lock poisoned"))?
            .clone();
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
        write_result
    }

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
        let persisted_control_room = match std::fs::read(&control_room_path) {
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
            let native_track_id = *native_track_ids
                .get(&region.track_id)
                .ok_or_else(|| anyhow::anyhow!("region references an unknown track"))
                .map_err(rollback_error)?;
            if !engine.add_region(native_track_id, &region.path, region.start as f64) {
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
                    candidate.path == region.path
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

        if (engine.get_sample_rate() - document.sample_rate).abs() > 0.5 {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded engine sample rate differs from project"
            )));
        }
        if document.cycle_enabled {
            if !engine.set_cycle_range(document.cycle_start_sample, document.cycle_end_sample, true)
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid cycle range"
                )));
            }
        } else {
            engine.set_loop(false);
        }
        engine.set_metronome_enabled(document.metronome_enabled);
        if !engine.set_master_gain(document.master_gain) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains an invalid master gain"
            )));
        }
        engine.clear_midi_notes();
        for note in &document.midi_notes {
            engine.set_midi_note(
                note.track_id,
                note.pitch,
                note.velocity,
                note.start_sample,
                note.length_samples,
            );
        }
        engine.clear_vca_groups();
        for group in &document.vca_groups {
            if !engine.add_vca_group(group.id, group.gain) {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid VCA group"
                )));
            }
            for track_id in &group.track_ids {
                let Some(native_track_id) = native_track_ids.get(track_id).copied() else {
                    return Err(rollback_error(anyhow::anyhow!(
                        "loaded VCA group references an unknown track"
                    )));
                };
                if !engine.assign_track_to_vca(native_track_id, group.id) {
                    return Err(rollback_error(anyhow::anyhow!(
                        "loaded project contains an invalid VCA track assignment"
                    )));
                }
            }
        }
        let mut restored_comping = crate::comping::CompingOrchestrator::new();
        for take in &document.comp_takes {
            restored_comping.add_take(crate::comping::Take {
                id: take.id,
                name: take.name.clone(),
                start_sample: take.start_sample,
                end_sample: take.end_sample,
            });
        }
        restored_comping.set_segments(
            document
                .comp_segments
                .iter()
                .map(|segment| crate::comping::CompSegment {
                    take_id: segment.take_id,
                    start: segment.start_sample,
                    len: segment.length_samples,
                    crossfade_samples: segment.crossfade_samples,
                })
                .collect(),
        );
        if !restored_comping.audit_comping() {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid comping state"
            )));
        }
        let comping_json = serde_json::to_string(&restored_comping)
            .map_err(|error| rollback_error(anyhow::anyhow!(error)))?;
        if !self.restore_comping_snapshot_json(&comping_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "failed to restore project comping state"
            )));
        }
        *self
            .scheduled_midi_notes
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI note lock poisoned"))? = document.midi_notes.clone();
        *self
            .chord_track
            .lock()
            .map_err(|_| anyhow::anyhow!("chord track lock poisoned"))? =
            document.chord_track.clone();
        *self
            .midi_events
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI event lock poisoned"))? =
            document.midi_events.clone();
        *self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))? =
            document.openutau_vocals.clone();
        let track_stacks_json = serde_json::to_string(&document.track_stacks).map_err(|error| {
            anyhow::anyhow!("track stack snapshot serialization failed: {error}")
        })?;
        if !self.restore_track_stacks_json(&track_stacks_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid track stack state"
            )));
        }
        let markers_json = serde_json::to_string(&document.markers).map_err(|error| {
            rollback_error(anyhow::anyhow!(
                "marker snapshot serialization failed: {error}"
            ))
        })?;
        if !self.restore_markers_json(&markers_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid arrangement markers"
            )));
        }
        *self
            .macro_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("Macro mapping lock poisoned"))? =
            document.macro_mappings.clone();
        *self
            .midi_learn_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI mapping lock poisoned"))? =
            document.midi_learn_mappings.clone();
        // Hardware pickup must be reacquired after hydration; otherwise a
        // controller could jump a restored parameter on its first message.
        self.midi_pickup_acquired
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI pickup state lock poisoned"))?
            .clear();
        // Hydration uses the same mutation APIs as interactive edits. Do not
        // expose those internal mutations as the first Undo steps of the new
        // document.
        engine.clear_undo_history();
        if let Ok(mut history) = self.midi_lyric_history.lock() {
            history.clear();
        }
        if let Ok(mut history) = self.chord_history.lock() {
            history.clear();
        }
        if let Ok(mut redo) = self.chord_redo_history.lock() {
            redo.clear();
        }
        if let Some(state) = persisted_control_room {
            // Rehydrate both the Rust control-plane snapshot and the native
            // realtime monitor graph. Keeping only the Rust copy would make
            // the UI look correct while the audio callback still used the
            // default speaker set after reopening a project.
            engine.reset_control_room();
            for (index, name) in state.monitor_outputs.iter().enumerate() {
                if index > 0 && !engine.add_control_room_speaker(name, state.monitor_output_gains[index]) {
                    return Err(rollback_error(anyhow::anyhow!("failed to restore control room output")));
                }
                if !engine.set_control_room_speaker_gain(index as u32, state.monitor_output_gains[index])
                    || !engine.set_control_room_speaker_enabled(index as u32, state.monitor_output_enabled[index])
                {
                    return Err(rollback_error(anyhow::anyhow!("failed to restore control room output settings")));
                }
            }
            if !engine.select_control_room_speaker(state.active_output as u32) {
                return Err(rollback_error(anyhow::anyhow!("failed to restore active control room output")));
            }
            for cue in &state.cues {
                if !engine.upsert_control_room_cue(cue.id, cue.gain, cue.enabled) {
                    return Err(rollback_error(anyhow::anyhow!("failed to restore control room cue")));
                }
            }
            engine.set_control_room_dim(state.dim);
            engine.set_control_room_talkback(state.talkback, state.talkback_gain);
            *self
                .control_room
                .lock()
                .map_err(|_| anyhow::anyhow!("control room lock poisoned"))? = state;
        }
        let _ = std::fs::remove_file(&rollback_path);
        Ok(())
    }

    /// Structured project hydration result for CLI/UI callers.  The legacy
    /// `load_project_v2` Result remains available to Rust callers, while this
    /// boundary preserves whether a failed load was a plugin-state timeout,
    /// a missing asset, or a generic validation error.  A failed hydration
    /// has already attempted native rollback before this method returns.
    pub fn load_project_v2_diagnostic_json(&self, path: &str) -> String {
        match self.load_project_v2(path) {
            Ok(()) => serde_json::json!({
                "ok": true,
                "path": path,
                "project_generation": self.project_generation(),
                "audio_generation": self.audio_config_generation(),
            })
            .to_string(),
            Err(error) => {
                let message = error.to_string();
                let code = if message.contains("state-timeout") {
                    "plugin_state_timeout"
                } else if message.contains("state-checksum-mismatch") {
                    "plugin_state_checksum_mismatch"
                } else if message.contains("state-version-unsupported") {
                    "plugin_state_version_unsupported"
                } else if message.contains("missing") {
                    "project_asset_missing"
                } else {
                    "project_hydration_failed"
                };
                let retryable = matches!(code, "plugin_state_timeout" | "project_asset_missing");
                let bridge_error = crate::bridge_error::BridgeError::new(code, message)
                    .retryable(retryable)
                    .object(format!("project:{path}"))
                    .at_generation(self.project_generation());
                serde_json::json!({
                    "ok": false,
                    "path": path,
                    "error": bridge_error,
                    "project_generation": self.project_generation(),
                    "audio_generation": self.audio_config_generation(),
                })
                .to_string()
            }
        }
    }

    pub fn scan_project_assets(&self, project_dir: &str, assets: Vec<String>) -> Vec<String> {
        SovereignPersistence::scan_assets(project_dir, &assets)
    }

    pub fn get_region_waveform(&self, tid: u32, rid: u32) -> Vec<f32> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.get_region_waveform(tid, rid).into_iter().collect()
        })
    }
    pub fn queue_region_waveform(&self, tid: u32, rid: u32) -> u64 {
        self.engine.as_ref().map_or(0, |e| e.queue_region_waveform(tid, rid))
    }
    pub fn poll_region_waveform(&self, request: u64) -> Vec<f32> {
        self.engine.as_ref().map_or_else(Vec::new, |e| {
            e.poll_region_waveform(request).into_iter().collect()
        })
    }
    pub fn region_waveform_pending(&self, request: u64) -> bool {
        self.engine.as_ref().is_some_and(|e| e.region_waveform_pending(request))
    }
    pub fn get_track_correlation(&self, tid: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |e| e.get_track_correlation(tid))
    }
    pub fn get_project_layout_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return String::new();
        };
        let raw = engine.get_project_layout_json();
        let Ok(mut layout) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let Ok(aux_ids) = serde_json::from_str::<Vec<u32>>(&self.aux_track_ids_json()) else {
            return raw;
        };
        let Some(tracks) = layout.as_array_mut() else {
            return raw;
        };
        for track in tracks {
            let Some(id) = track.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            if aux_ids.contains(&(id as u32))
                && track.get("type").and_then(serde_json::Value::as_str) == Some("Bus")
            {
                track["type"] = serde_json::Value::String("Aux".to_owned());
            }
        }
        serde_json::to_string(&layout).unwrap_or(raw)
    }

    /// Generation of the exact layout snapshot returned above.  Commands
    /// must echo this value back when applying a mutation, preventing a UI or
    /// external harness from editing a newer project from an old snapshot.
    pub fn project_generation(&self) -> u64 {
        crate::command_api::snapshot_generation(self.get_project_layout_json().as_bytes())
    }

    pub fn take_mix_snapshot_json(&self, name: &str, states_json: &str) -> String {
        self.take_mix_snapshot_with_plugins_json(name, states_json, "[]")
    }

    pub fn take_mix_snapshot_with_plugins_json(
        &self,
        name: &str,
        states_json: &str,
        plugins_json: &str,
    ) -> String {
        self.take_mix_snapshot_with_plugins_and_routing_json(name, states_json, plugins_json, "[]")
    }

    pub fn take_mix_snapshot_with_plugins_and_routing_json(
        &self,
        name: &str,
        states_json: &str,
        plugins_json: &str,
        routing_json: &str,
    ) -> String {
        let Ok(states) = serde_json::from_str::<std::collections::HashMap<u32, f32>>(states_json)
        else {
            return "{\"ok\":false,\"code\":\"invalid_snapshot_state\"}".into();
        };
        let Ok(plugins) =
            serde_json::from_str::<Vec<crate::snapshots::PluginSnapshotState>>(plugins_json)
        else {
            return "{\"ok\":false,\"code\":\"invalid_snapshot_plugins\"}".into();
        };
        let Ok(routing) = serde_json::from_str::<serde_json::Value>(routing_json) else {
            return "{\"ok\":false,\"code\":\"invalid_snapshot_routing\"}".into();
        };
        let Ok(mut snapshots) = self.mix_snapshots.lock() else {
            return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
        };
        let before = snapshots.snapshots.len();
        snapshots.take_snapshot_with_plugins_and_routing(
            name,
            states,
            plugins,
            routing.to_string(),
        );
        let accepted = snapshots.snapshots.len() >= before
            && snapshots
                .snapshots
                .iter()
                .any(|snapshot| snapshot.name == name);
        serde_json::json!({"ok": accepted, "operation": "take_mix_snapshot", "name": name, "count": snapshots.snapshots.len()}).to_string()
    }

    pub fn diff_mix_snapshots_json(&self, first: usize, second: usize) -> String {
        let Ok(snapshots) = self.mix_snapshots.lock() else {
            return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
        };
        if first >= snapshots.snapshots.len() || second >= snapshots.snapshots.len() {
            return "{\"ok\":false,\"code\":\"snapshot_not_found\"}".into();
        }
        serde_json::json!({"ok": true, "operation": "diff_mix_snapshots", "first": first, "second": second, "diff": snapshots.diff_snapshots(first, second)}).to_string()
    }

    pub fn recall_mix_snapshot_json(&self, index: usize) -> String {
        let Ok(snapshots) = self.mix_snapshots.lock() else {
            return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
        };
        let Some(snapshot) = snapshots.snapshots.get(index) else {
            return "{\"ok\":false,\"code\":\"snapshot_not_found\"}".into();
        };
        let routing = serde_json::from_str::<serde_json::Value>(&snapshot.routing_state)
            .unwrap_or_else(|_| serde_json::json!([]));
        serde_json::json!({"ok": true, "operation": "recall_mix_snapshot", "index": index, "name": snapshot.name, "states": snapshot.parameter_states, "plugin_states": snapshot.plugin_states, "routing": routing}).to_string()
    }

    /// Apply a captured scene to native track parameters. Capture uses four
    /// stable slots per track: `track_id*4 + {0:volume,1:pan,2:mute,3:solo}`;
    /// unknown IDs are ignored for forward compatibility.
    pub fn apply_mix_snapshot_json(&self, index: usize) -> String {
        let (states, plugin_states, routing_state) = {
            let Ok(snapshots) = self.mix_snapshots.lock() else {
                return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
            };
            let Some(snapshot) = snapshots.snapshots.get(index) else {
                return "{\"ok\":false,\"code\":\"snapshot_not_found\"}".into();
            };
            (
                snapshot.parameter_states.clone(),
                snapshot.plugin_states.clone(),
                snapshot.routing_state.clone(),
            )
        };
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\"}".into();
        };
        let mut applied = 0usize;
        for (parameter_id, value) in states {
            let track_id = parameter_id / 2;
            let slot = parameter_id % 4;
            let accepted = if slot == 0 {
                value.is_finite()
                    && (0.0..=2.0).contains(&value)
                    && engine.set_track_volume(track_id, value)
            } else if slot == 1 {
                value.is_finite()
                    && (-1.0..=1.0).contains(&value)
                    && engine.set_track_pan(track_id, value)
            } else if slot == 2 {
                (value == 0.0 || value == 1.0) && engine.set_track_mute(track_id, value > 0.5)
            } else {
                (value == 0.0 || value == 1.0) && engine.set_track_solo(track_id, value > 0.5)
            };
            if accepted {
                applied += 1;
            }
        }
        for plugin in plugin_states {
            if !engine.set_plugin_bypass(plugin.track_id, plugin.plugin_index, plugin.bypassed) {
                continue;
            }
            applied += 1;
            for (parameter_id, value) in plugin.parameters.into_iter().enumerate() {
                if value.is_finite()
                    && engine.set_plugin_parameter_without_undo(
                        plugin.track_id,
                        plugin.plugin_index,
                        parameter_id as u32,
                        value,
                    )
                {
                    applied += 1;
                }
            }
        }
        if let Ok(routes) = serde_json::from_str::<Vec<crate::project_contracts::AudioRouteContract>>(
            &routing_state,
        ) {
            for route in routes {
                if route.source_id != route.destination_id
                    && route.gain.is_finite()
                    && (0.0..=2.0).contains(&route.gain)
                    && engine.set_route_gain(
                        route.source_id,
                        route.destination_id,
                        route.gain,
                        true,
                    )
                {
                    applied += 1;
                }
            }
        }
        serde_json::json!({"ok": true, "operation": "apply_mix_snapshot", "index": index, "applied": applied}).to_string()
    }

    pub fn poll_events_into(&self, out: &mut Vec<ffi::BridgeEvent>) {
        if let Some(e) = self.engine.as_ref() {
            let core = ffi::get_unified_engine(e);

            // --- INDUSTRIAL: Zero-Allocation Collection ---
            // Reuses the passed vector to avoid reallocations.
            out.clear();
            let mut ev = ffi::BridgeEvent {
                timestamp: 0,
                event_type: 0,
                track_id: 0,
                value: 0.0,
                label: [0; 128],
            };

            for _ in 0..128 {
                // Support higher burst volume (Point 1)
                if ffi::pop_event(core, &mut ev) {
                    out.push(ev);
                } else {
                    break;
                }
            }
        }
    }
}
