//! Stems-batch workflow for HirariCore, split out of hirari_core_project_plugins.rs.
use super::*;

impl HirariCore {
    /// Render one WAV per project track by selecting each track through a
    /// render-only solo state. The selection is never added to Undo/Redo and
    /// the original solo state is restored even when a stem fails.
    /// Phase 1 of a stems export: validate the whole batch, derive every
    /// destination through the one shared naming policy, and park the queue
    /// on the core.
    ///
    /// Returns `{"ok":true,"total":N,"format":F}` on success, or the same bare
    /// error document `hirari render stems` has always emitted — the codes and
    /// messages are unchanged on purpose, they are the CLI's contract.
    ///
    /// The control-plane transaction is held only while the layout is
    /// snapshotted, not across the renders that follow. A render never mutates
    /// the project, and each surface is already serialized against save and
    /// load for the whole batch: the CLI is single threaded, and the GUI's
    /// single-slot operation gate refuses Save/Load/Recover while its Render
    /// lease is held (its autosave additionally requires `!is_rendering`).
    /// Holding this mutex for the length of a multi-minute render would only
    /// stall `save_project_with_optional_ui_midi_notes`, which shares it.
    pub fn stem_batch_prepare(
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
        let Some(engine) = self.engine_opt() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let layout: serde_json::Value =
            match serde_json::from_str(engine.project_layout_json().as_str()) {
                Ok(layout) => layout,
                Err(_) => {
                    return "{\"code\":\"invalid_project_snapshot\",\"retryable\":false}"
                        .to_owned()
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

        let mut planned = Vec::with_capacity(selected_tracks.len());
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
            let name = crate::stem_export::safe_stem_name(raw_name, track_id, &mut names);
            planned.push((track_id, output_dir_path.join(format!("{name}.wav"))));
        }

        // Reject the whole batch before changing any render-only solo state.
        // Otherwise a pre-existing second stem would cause the first stem to
        // render and then be deleted during rollback, making a dry-run look
        // destructive and needlessly exercising the audio graph.
        if let Some((_, existing)) = planned.iter().find(|(_, path)| path.exists()) {
            return serde_json::json!({
                "code": "stem_output_exists",
                "path": existing,
                "retryable": false,
                "restored_render_target": true,
            })
            .to_string();
        }

        engine.set_offline_render_tail_seconds(Self::bounded_render_tail_seconds(tail_seconds));
        engine.set_offline_render_options(pre_fader, include_inserts);

        match self.stem_batch.lock() {
            Ok(mut parked) => {
                // A batch that was started and never settled still owns its
                // published files; drop them rather than leak them.
                if let Some(mut abandoned) = parked.take() {
                    abandoned.rollback();
                }
                let batch = crate::stem_export::StemBatchState::new(planned, format);
                let total = batch.progress().1;
                *parked = Some(batch);
                serde_json::json!({"ok": true, "total": total, "format": format}).to_string()
            }
            Err(_) => stem_batch_unavailable(),
        }
    }

    /// Phase 2: arm the render target for the next stem and report the file
    /// the surface has to render. A finished batch returns its outputs and is
    /// cleared from the core.
    pub fn stem_batch_next(&self) -> String {
        let mut parked = match self.stem_batch.lock() {
            Ok(parked) => parked,
            Err(_) => return stem_batch_unavailable(),
        };
        let Some(batch) = parked.as_mut() else {
            return "{\"code\":\"stem_batch_not_started\",\"retryable\":false}".to_owned();
        };
        if batch.is_complete() {
            let (outputs, format) = (batch.outputs(), batch.format());
            *parked = None;
            return serde_json::json!({
                "ok": true,
                "state": "complete",
                "outputs": outputs,
                "format": format,
            })
            .to_string();
        }
        let (track_id, path) = match batch.next_target() {
            Some(target) => target,
            None => {
                *parked = None;
                return "{\"code\":\"stem_batch_not_started\",\"retryable\":false}".to_owned();
            }
        };
        if !self
            .engine_opt()
            .is_some_and(|engine| engine.set_offline_render_target(track_id))
        {
            // Reported exactly as the blocking driver reported it when arming
            // was folded into the render call, so no document changed shape.
            batch.rollback();
            *parked = None;
            if let Some(engine) = self.engine_opt() {
                engine.clear_offline_render_target();
            }
            return serde_json::json!({
                "code": "stem_render_failed",
                "message": format!("stem render failed for track {track_id}"),
                "retryable": true,
                "restored_render_target": true,
            })
            .to_string();
        }
        let (rendered, total) = batch.progress();
        serde_json::json!({
            "ok": true,
            "state": "ready",
            "index": rendered,
            "total": total,
            "format": batch.format(),
            "track_id": track_id,
            "output": path.to_string_lossy(),
        })
        .to_string()
    }

    /// Phase 3: record the armed stem's result. A published stem has to
    /// validate as a WAV before the queue advances; a failed bounce or an
    /// unusable file rolls the whole batch back, so a failure never leaves a
    /// partial set of stems behind.
    pub fn stem_batch_settle(&self, rendered: bool) -> String {
        let mut parked = match self.stem_batch.lock() {
            Ok(parked) => parked,
            Err(_) => return stem_batch_unavailable(),
        };
        let Some(batch) = parked.as_mut() else {
            return "{\"code\":\"stem_batch_not_started\",\"retryable\":false}".to_owned();
        };
        // The armed stem is still the head of the queue: only `advance` moves it.
        let failure: Option<String> = match batch.next_target() {
            None => Some("stem batch had no armed target".to_owned()),
            Some((track_id, path)) => {
                if !rendered {
                    Some(format!("stem render failed for track {track_id}"))
                } else if !self.validate_render_output(path.to_string_lossy().as_ref(), false) {
                    Some(format!("stem output failed validation for track {track_id}"))
                } else {
                    batch.advance(path);
                    None
                }
            }
        };
        if let Some(message) = failure {
            batch.rollback();
            *parked = None;
            if let Some(engine) = self.engine_opt() {
                engine.clear_offline_render_target();
            }
            return serde_json::json!({
                "code": "stem_render_failed",
                "message": message,
                "retryable": true,
                "restored_render_target": true,
            })
            .to_string();
        }

        // Drop the render-only target after every stem, so a batch that stops
        // here and one that continues leave the engine in the same state.
        if let Some(engine) = self.engine_opt() {
            engine.clear_offline_render_target();
        }
        if batch.is_complete() {
            let (outputs, format) = (batch.outputs(), batch.format());
            *parked = None;
            return serde_json::json!({
                "ok": true,
                "state": "complete",
                "outputs": outputs,
                "format": format,
            })
            .to_string();
        }
        let (rendered_count, total) = batch.progress();
        serde_json::json!({"ok": true, "state": "stem_done", "index": rendered_count, "total": total})
            .to_string()
    }

    /// Aborts a batch: removes everything it published and clears the
    /// render-only target, so a cancelled export leaves no partial stems.
    pub fn stem_batch_abort(&self) -> String {
        match self.stem_batch.lock() {
            Ok(mut parked) => match parked.as_mut() {
                Some(batch) => {
                    batch.rollback();
                    *parked = None;
                    if let Some(engine) = self.engine_opt() {
                        engine.clear_offline_render_target();
                    }
                    serde_json::json!({"ok": true, "state": "aborted"}).to_string()
                }
                None => "{\"code\":\"stem_batch_not_started\",\"retryable\":false}".to_owned(),
            },
            Err(_) => stem_batch_unavailable(),
        }
    }

    /// `(rendered, total)` for a running batch, or `None` when nothing is
    /// being exported. Stays a plain tuple so a polling surface can show
    /// progress without parsing a document every tick.
    pub fn stem_batch_progress(&self) -> Option<(u32, u32)> {
        let parked = self.stem_batch.lock().ok()?;
        parked
            .as_ref()
            .map(|batch| batch.progress())
    }

    /// The blocking driver `hirari render stems` uses.
    ///
    /// Everything that decides *what* a stems export does — selection,
    /// naming, output validation, rollback — lives in `stem_batch_*`, which
    /// the GUI drives from its telemetry timer. This loop adds only the one
    /// thing a blocking caller has: it renders each armed stem with the
    /// engine's synchronous bounce instead of waiting on the engine's own
    /// bounce thread. The documents it returns are unchanged.
    pub fn bounce_stems_diagnostic_json(
        &self,
        output_dir: &str,
        format: u32,
        requested_track_ids: &[u32],
        tail_seconds: f32,
        pre_fader: bool,
        include_inserts: bool,
    ) -> String {
        let prepared = self.stem_batch_prepare(
            output_dir,
            format,
            requested_track_ids,
            tail_seconds,
            pre_fader,
            include_inserts,
        );
        if !stem_batch_succeeded(&prepared) {
            return prepared;
        }
        loop {
            let armed = self.stem_batch_next();
            if !stem_batch_succeeded(&armed) {
                return armed;
            }
            let armed_value = match serde_json::from_str::<serde_json::Value>(&armed) {
                Ok(value) => value,
                Err(_) => return stem_batch_unreadable(),
            };
            match armed_value
                .get("state")
                .and_then(serde_json::Value::as_str)
            {
                Some("complete") => {
                    return stem_batch_success_document(
                        &armed_value,
                        format,
                        tail_seconds,
                        pre_fader,
                        include_inserts,
                    );
                }
                Some("ready") => {
                    let output = match armed_value
                        .get("output")
                        .and_then(serde_json::Value::as_str)
                    {
                        Some(output) => output.to_owned(),
                        None => {
                            self.stem_batch_abort();
                            return stem_batch_unreadable();
                        }
                    };
                    let rendered = self
                        .engine_opt()
                        .is_some_and(|engine| engine.bounce_project(output.as_str(), format));
                    let settled = self.stem_batch_settle(rendered);
                    if !stem_batch_succeeded(&settled) {
                        return settled;
                    }
                    let settled_value = match serde_json::from_str::<serde_json::Value>(&settled) {
                        Ok(value) => value,
                        Err(_) => return stem_batch_unreadable(),
                    };
                    if settled_value
                        .get("state")
                        .and_then(serde_json::Value::as_str)
                        == Some("complete")
                    {
                        return stem_batch_success_document(
                            &settled_value,
                            format,
                            tail_seconds,
                            pre_fader,
                            include_inserts,
                        );
                    }
                }
                _ => return armed,
            }
        }
    }

    // --- RUST-POWERED PERSISTENCE (Point 3) ---
}
