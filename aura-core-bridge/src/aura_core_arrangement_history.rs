impl AuraCore {
    pub fn undo(&self) {
        if let Some(e) = self.engine.as_ref() {
            if e.get_undo_count() == 0 {
                if let Ok(mut history) = self.chord_history.lock() {
                    if let Some(entry) = history.pop() {
                        if let Ok(mut chords) = self.chord_track.lock() {
                            *chords = entry.before.clone();
                        }
                        if let Ok(mut redo) = self.chord_redo_history.lock() {
                            redo.push(entry);
                        }
                    }
                }
                if let Ok(mut history) = self.comping_history.lock() {
                    if let Some(entry) = history.pop() {
                        let _ = self.restore_comping_snapshot_json(
                            &serde_json::to_string(&entry.before).unwrap_or_default(),
                        );
                        if let Ok(mut redo) = self.comping_redo_history.lock() {
                            redo.push(entry);
                        }
                    }
                }
                return;
            }
            let depth_before = e.get_undo_count();
            e.undo();
            self.sync_midi_note_metadata_from_engine();
            if let Ok(history) = self.chord_history.lock() {
                if let Some(entry) = history
                    .iter()
                    .find(|entry| entry.depth_after == depth_before)
                {
                    if let Ok(mut chords) = self.chord_track.lock() {
                        *chords = entry.before.clone();
                    }
                }
            }
            if let Ok(history) = self.midi_lyric_history.lock() {
                if let Some(entry) = history
                    .iter()
                    .find(|entry| entry.depth_after == depth_before)
                {
                    if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
                        *notes = entry.before.clone();
                    }
                }
            }
        }
    }

    pub fn begin_undo_transaction(&self, name: &str) {
        if let Some(e) = self.engine.as_ref() {
            e.begin_undo_transaction(name);
        }
    }

    pub fn end_undo_transaction(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.end_undo_transaction())
    }

    pub fn abort_undo_transaction(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.abort_undo_transaction())
    }

    pub fn set_automation_record_mode(&self, mode: u32) -> bool {
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        engine.set_automation_record_mode(mode.min(4));
        true
    }

    pub fn undo_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let before = engine.get_undo_count();
        if before == 0 {
            if let Ok(mut history) = self.chord_history.lock() {
                if let Some(entry) = history.pop() {
                    if let Ok(mut chords) = self.chord_track.lock() {
                        *chords = entry.before.clone();
                    }
                    if let Ok(mut redo) = self.chord_redo_history.lock() {
                        redo.push(entry);
                    }
                    return serde_json::json!({"ok": true, "operation": "undo", "domain": "chord_track"}).to_string();
                }
            }
            if let Ok(mut history) = self.comping_history.lock() {
                if let Some(entry) = history.pop() {
                    let _ = self.restore_comping_snapshot_json(
                        &serde_json::to_string(&entry.before).unwrap_or_default(),
                    );
                    if let Ok(mut redo) = self.comping_redo_history.lock() {
                        redo.push(entry);
                    }
                    return serde_json::json!({"ok": true, "operation": "undo", "domain": "comping"}).to_string();
                }
            }
            return serde_json::json!({
                "ok": false,
                "code": "undo_history_empty",
                "retryable": false,
                "undo_depth": 0,
            })
            .to_string();
        }
        engine.undo();
        self.sync_midi_note_metadata_from_engine();
        if let Ok(history) = self.chord_history.lock() {
            if let Some(entry) = history.iter().find(|entry| entry.depth_after == before) {
                if let Ok(mut chords) = self.chord_track.lock() {
                    *chords = entry.before.clone();
                }
            }
        }
        if let Ok(history) = self.midi_lyric_history.lock() {
            if let Some(entry) = history.iter().find(|entry| entry.depth_after == before) {
                if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
                    *notes = entry.before.clone();
                }
            }
        }
        if engine.get_undo_count() < before {
            return serde_json::json!({"ok": true, "operation": "undo"}).to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "undo_history_empty",
            "retryable": false,
            "undo_depth": before,
        })
        .to_string()
    }

    pub fn redo(&self) {
        if let Some(e) = self.engine.as_ref() {
            if e.get_redo_count() == 0 {
                if let Ok(mut redo) = self.chord_redo_history.lock() {
                    if let Some(entry) = redo.pop() {
                        if let Ok(mut chords) = self.chord_track.lock() {
                            *chords = entry.after.clone();
                        }
                        if let Ok(mut history) = self.chord_history.lock() {
                            history.push(entry);
                        }
                    }
                }
                if let Ok(mut redo) = self.comping_redo_history.lock() {
                    if let Some(entry) = redo.pop() {
                        let _ = self.restore_comping_snapshot_json(
                            &serde_json::to_string(&entry.after).unwrap_or_default(),
                        );
                        if let Ok(mut history) = self.comping_history.lock() {
                            history.push(entry);
                        }
                    }
                }
                return;
            }
            e.redo();
            self.sync_midi_note_metadata_from_engine();
            let depth_after = e.get_undo_count();
            if let Ok(history) = self.chord_history.lock() {
                if let Some(entry) = history
                    .iter()
                    .find(|entry| entry.depth_after == depth_after)
                {
                    if let Ok(mut chords) = self.chord_track.lock() {
                        *chords = entry.after.clone();
                    }
                }
            }
            if let Ok(history) = self.midi_lyric_history.lock() {
                if let Some(entry) = history
                    .iter()
                    .find(|entry| entry.depth_after == depth_after)
                {
                    if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
                        *notes = entry.after.clone();
                    }
                }
            }
        }
    }

    pub fn redo_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let before = engine.get_redo_count();
        if before == 0 {
            if let Ok(mut redo) = self.chord_redo_history.lock() {
                if let Some(entry) = redo.pop() {
                    if let Ok(mut chords) = self.chord_track.lock() {
                        *chords = entry.after.clone();
                    }
                    if let Ok(mut history) = self.chord_history.lock() {
                        history.push(entry);
                    }
                    return serde_json::json!({"ok": true, "operation": "redo", "domain": "chord_track"}).to_string();
                }
            }
            if let Ok(mut redo) = self.comping_redo_history.lock() {
                if let Some(entry) = redo.pop() {
                    let _ = self.restore_comping_snapshot_json(
                        &serde_json::to_string(&entry.after).unwrap_or_default(),
                    );
                    if let Ok(mut history) = self.comping_history.lock() {
                        history.push(entry);
                    }
                    return serde_json::json!({"ok": true, "operation": "redo", "domain": "comping"}).to_string();
                }
            }
            return serde_json::json!({
                "ok": false,
                "code": "redo_history_empty",
                "retryable": false,
                "redo_depth": 0,
            })
            .to_string();
        }
        engine.redo();
        self.sync_midi_note_metadata_from_engine();
        if let Ok(history) = self.chord_history.lock() {
            let depth_after = engine.get_undo_count();
            if let Some(entry) = history
                .iter()
                .find(|entry| entry.depth_after == depth_after)
            {
                if let Ok(mut chords) = self.chord_track.lock() {
                    *chords = entry.after.clone();
                }
            }
        }
        if let Ok(history) = self.midi_lyric_history.lock() {
            let depth_after = engine.get_undo_count();
            if let Some(entry) = history
                .iter()
                .find(|entry| entry.depth_after == depth_after)
            {
                if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
                    *notes = entry.after.clone();
                }
            }
        }
        if engine.get_redo_count() < before {
            return serde_json::json!({"ok": true, "operation": "redo"}).to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "redo_history_empty",
            "retryable": false,
            "redo_depth": before,
        })
        .to_string()
    }

    pub fn undo_depth(&self) -> u32 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_undo_count())
    }

    pub fn redo_depth(&self) -> u32 {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.get_redo_count())
    }

    pub fn start_render(&self) {
        let path = std::env::temp_dir().join("aura_master.wav");
        let Some(path_str) = path.to_str() else {
            report_aura_log(0, "Render failed: output path is not valid UTF-8");
            return;
        };
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        if engine.bounce_project(path_str, 0) {
            report_aura_log(2, &format!("Render completed: {}", path_str));
        } else {
            report_aura_log(0, &format!("Render failed: {}", path_str));
        }
    }

    pub fn start_render_async(&self) -> bool {
        let path = std::env::temp_dir().join("aura_master.wav");
        self.start_render_async_to(path.to_string_lossy().as_ref())
    }

    /// Starts an asynchronous render using an explicit output path. UI and
    /// concurrent callers should prefer this method over the legacy
    /// environment-based entry point so parallel renders cannot collide.
    pub fn start_render_async_to(&self, path: &str) -> bool {
        if !is_wav_output_path(path) {
            return false;
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        engine.bounce_project_async(path, 0)
    }

    pub fn start_render_diagnostic_json(&self, path: &str) -> String {
        if !is_wav_output_path(path) {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_render_path",
                "render output must be a supported WAV path",
            );
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.bounce_project_async(path, 0) {
            return format!(
                "{{\"ok\":true,\"operation\":\"start_render\",\"path\":{}}}",
                serde_json::to_string(path).unwrap_or_default()
            );
        } else {
            crate::bridge_error::BridgeError::new(
                "render_rejected",
                "render could not be scheduled",
            )
            .retryable(true)
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    /// Returns the native bounce state and progress when the bridge is alive.
    /// `None` is intentionally distinct from a valid idle/zero-progress state.
    pub fn get_bounce_status(&self) -> Option<(u32, f32)> {
        self.bounce_snapshot()
            .map(|snapshot| (snapshot.state, snapshot.progress))
    }

    /// Returns render state even when the native progress provider cannot
    /// produce a finite value. This keeps an indeterminate render distinct
    /// from a disconnected engine.
    pub fn bounce_snapshot(&self) -> Option<BounceSnapshot> {
        let engine = self.engine.as_ref()?;
        let (progress, progress_available) =
            normalize_bounce_progress(engine.get_bounce_progress());
        Some(BounceSnapshot {
            state: engine.get_bounce_state(),
            progress,
            progress_available,
        })
    }

    pub fn cancel_render(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.cancel_bounce())
    }

    pub fn cancel_render_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.cancel_bounce() {
            return "{\"ok\":true,\"operation\":\"cancel_render\"}".to_owned();
        } else {
            crate::bridge_error::BridgeError::new(
                "render_not_active",
                "no active render accepted cancellation",
            )
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn execute_auto_mixing(&self) {
        if let Some(e) = self.engine.as_ref() {
            let _ = e.execute_auto_mixing();
        }
    }
    pub fn execute_auto_arrangement(&self) {
        if let Some(e) = self.engine.as_ref() {
            let _ = e.execute_auto_arrangement();
        }
    }

    pub fn execute_auto_mixing_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.execute_auto_mixing() {
            return "{\"ok\":true,\"operation\":\"execute_auto_mixing\"}".to_owned();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "project_empty_or_rejected",
                "auto mixing requires at least one track",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn execute_auto_arrangement_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.execute_auto_arrangement() {
            return "{\"ok\":true,\"operation\":\"execute_auto_arrangement\"}".to_owned();
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "project_empty_or_rejected",
                "auto arrangement requires at least one track",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
}
