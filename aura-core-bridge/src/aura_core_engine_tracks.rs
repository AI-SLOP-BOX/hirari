impl AuraCore {
    /// Track type codes are part of the native bridge contract. Keep the
    /// legacy numeric entry point for ABI compatibility, but reject unknown
    /// values before they reach the engine and expose named helpers below.
    pub fn add_track(&self, t_type: u32) -> u32 {
        if t_type > 4 {
            return 0;
        }
        let id = self.engine.as_ref().map_or(0, |e| e.add_track(t_type));
        if id != 0 {
            self.publish_production_event(crate::production_events::ProductionEvent::TrackAdded {
                track_id: id,
                name: format!("Track {id}"),
            });
        }
        // Beginner-friendly default routing: newly created signal tracks are
        // sent to the first available Bus. Bus/Vocal tracks remain un-routed
        // so the operation cannot create a cycle or a self-route.
        if id != 0 && matches!(t_type, 0..=2) {
            if let Some(engine) = self.engine.as_ref() {
                if let Ok(layout) =
                    serde_json::from_str::<serde_json::Value>(&engine.get_project_layout_json())
                {
                    if let Some(bus_id) = layout
                        .get("tracks")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|tracks| {
                            tracks.iter().find(|track| {
                                track.get("type").and_then(serde_json::Value::as_str) == Some("Bus")
                            })
                        })
                        .and_then(|track| track.get("id").and_then(serde_json::Value::as_u64))
                    {
                        let _ = engine.set_route_gain(id, bus_id as u32, 1.0, true);
                    }
                }
            }
        }
        id
    }

    pub fn add_audio_track(&self) -> u32 {
        self.add_track(0)
    }

    pub fn add_midi_track(&self) -> u32 {
        self.add_track(1)
    }

    pub fn add_instrument_track(&self) -> u32 {
        self.add_track(2)
    }

    pub fn add_bus_track(&self) -> u32 {
        self.add_track(3)
    }

    /// Aux channels share the native Bus signal type, but remain a distinct
    /// named operation for UI and automation clients.
    pub fn add_aux_track(&self) -> u32 {
        let id = self.add_bus_track();
        if id != 0 {
            if let Ok(mut aux_ids) = self.aux_track_ids.lock() {
                aux_ids.insert(id);
            }
            // Keep Aux creation visibly distinct in the arrange/mixer views
            // even though the native signal path uses the shared BusTrack
            // implementation.
            let _ = self.set_track_name(id, &format!("Aux {}", id));
        }
        id
    }

    pub fn add_vocal_track(&self) -> u32 {
        self.add_track(4)
    }

    pub fn aux_track_ids_json(&self) -> String {
        let mut ids = self
            .aux_track_ids
            .lock()
            .map(|value| value.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        ids.sort_unstable();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn restore_aux_track_ids_json(&self, snapshot: &str) -> bool {
        let Ok(ids) = serde_json::from_str::<Vec<u32>>(snapshot) else {
            return false;
        };
        if ids.contains(&0) || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return false;
        }
        let Ok(mut current) = self.aux_track_ids.lock() else {
            return false;
        };
        *current = ids.into_iter().collect();
        true
    }

    pub fn add_vca_group(&self, group_id: u32, gain: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.add_vca_group(group_id, gain))
    }

    pub fn assign_track_to_vca(&self, track_id: u32, group_id: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.assign_track_to_vca(track_id, group_id))
    }

    pub fn set_vca_group_gain(&self, group_id: u32, gain: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_vca_group_gain(group_id, gain))
    }

    pub fn get_vca_track_gain(&self, track_id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(1.0, |engine| engine.get_vca_track_gain(track_id))
    }

    pub fn get_vca_snapshot_json(&self) -> String {
        self.engine
            .as_ref()
            .map(|engine| engine.get_vca_snapshot_json().to_string())
            .unwrap_or_else(|| "[]".to_owned())
    }

    pub fn clear_vca_groups(&self) {
        if let Some(engine) = self.engine.as_ref() {
            engine.clear_vca_groups();
        }
    }

    /// Publishes a native immutable freeze snapshot for a track.  This is a
    /// real graph operation, not merely a project flag: the native engine
    /// renders and owns the frozen audio before returning success.
    pub fn freeze_track_diagnostic_json(&self, track_id: u32, total_samples: u64) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({
                "ok": false,
                "code": "engine_unavailable",
                "retryable": true,
            })
            .to_string();
        };
        let sample_rate = engine.get_sample_rate().round() as u32;
        if total_samples == 0 || sample_rate == 0 {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_freeze_range",
                "retryable": false,
            })
            .to_string();
        }
        if engine.freeze_track(track_id, total_samples, sample_rate) {
            return serde_json::json!({
                "ok": true,
                "operation": "freeze_track",
                "track_id": track_id,
                "total_samples": total_samples,
                "sample_rate": sample_rate,
                "audio_generation": engine.get_audio_config_generation(),
                "frozen": true,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "freeze_track_rejected",
            "track_id": track_id,
            "retryable": true,
        })
        .to_string()
    }

    pub fn freeze_track_to_project_end_diagnostic_json(&self, track_id: u32) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok":false,"code":"engine_unavailable","retryable":true})
                .to_string();
        };
        let sample_rate = engine.get_sample_rate().round() as u32;
        if sample_rate == 0 || !engine.freeze_track_to_project_end(track_id, sample_rate) {
            return serde_json::json!({"ok":false,"code":"freeze_track_rejected","track_id":track_id}).to_string();
        }
        serde_json::json!({"ok":true,"operation":"freeze_track","track_id":track_id,"sample_rate":sample_rate,"scope":"project_end"}).to_string()
    }

    pub fn freeze_track_to_project_end(&self, track_id: u32) -> bool {
        self.engine.as_ref().is_some_and(|engine| {
            let sample_rate = engine.get_sample_rate().round() as u32;
            sample_rate != 0 && engine.freeze_track_to_project_end(track_id, sample_rate)
        })
    }

    pub fn unfreeze_track(&self, track_id: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.unfreeze_track(track_id))
    }

    pub fn freeze_track_to_file_diagnostic_json(
        &self,
        track_id: u32,
        path: &str,
        total_samples: u64,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok": false, "code": "engine_unavailable", "retryable": true}).to_string();
        };
        let sample_rate = engine.get_sample_rate().round() as u32;
        if path.trim().is_empty() || total_samples == 0 || sample_rate == 0 {
            return serde_json::json!({"ok": false, "code": "invalid_freeze_request", "retryable": false}).to_string();
        }
        if engine.freeze_track_to_file(track_id, path, total_samples, sample_rate) {
            return serde_json::json!({
                "ok": true,
                "operation": "freeze_track",
                "track_id": track_id,
                "path": path,
                "total_samples": total_samples,
                "sample_rate": sample_rate,
                "audio_generation": engine.get_audio_config_generation(),
                "frozen": true,
            })
            .to_string();
        }
        serde_json::json!({"ok": false, "code": "freeze_track_file_rejected", "track_id": track_id, "retryable": true}).to_string()
    }

    pub fn unfreeze_track_diagnostic_json(&self, track_id: u32) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({
                "ok": false,
                "code": "engine_unavailable",
                "retryable": true,
            })
            .to_string();
        };
        if engine.unfreeze_track(track_id) {
            return serde_json::json!({
                "ok": true,
                "operation": "unfreeze_track",
                "track_id": track_id,
                "frozen": false,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "track_not_found",
            "track_id": track_id,
            "retryable": false,
        })
        .to_string()
    }

    pub fn track_freeze_status_diagnostic_json(&self, track_id: u32) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok": false, "code": "engine_unavailable"}).to_string();
        };
        serde_json::json!({
            "ok": true,
            "operation": "track_freeze_status",
            "track_id": track_id,
            "frozen": engine.is_track_frozen(track_id),
        })
        .to_string()
    }
    pub fn add_track_diagnostic_json(&self, t_type: u32) -> String {
        if t_type > 4 {
            return serde_json::json!({
                "ok": false,
                "code": "unsupported_track_type",
                "track_type": t_type,
                "supported_track_types": ["Audio", "Midi", "Instrument", "Bus", "Vocal"],
                "retryable": false,
            })
            .to_string();
        }
        if self.engine.is_null() {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "engine_unavailable",
                    "audio engine unavailable",
                )
                .retryable(true),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let track_id = self.add_track(t_type);
        if track_id != 0 {
            return format!("{{\"ok\":true,\"track_id\":{track_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_creation_failed",
                "track allocation failed",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn remove_track(&self, id: u32) -> bool {
        let removed = self.engine.as_ref().is_some_and(|e| e.remove_track(id));
        if !removed {
            return false;
        }
        if let Ok(mut stacks) = self.track_stacks.lock() {
            for stack in stacks.iter_mut() {
                stack.member_track_ids.retain(|member| *member != id);
            }
            stacks.retain(|stack| !stack.member_track_ids.is_empty());
        }
        if let Ok(mut bases) = self.track_stack_base_volumes.lock() {
            bases.remove(&id);
        }
        let _ = self.apply_track_stack_gains();
        true
    }
    pub fn remove_track_diagnostic_json(&self, id: u32) -> String {
        if self.engine.is_null() {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "engine_unavailable",
                    "audio engine unavailable",
                )
                .retryable(true),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if self.remove_track(id) {
            return format!("{{\"ok\":true,\"track_id\":{id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "track removal rejected",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn set_track_name(&self, id: u32, name: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_track_name(id, name))
    }

    pub fn set_track_name_diagnostic_json(&self, id: u32, name: &str) -> String {
        if id == 0 || name.trim().is_empty() || name.contains('\0') || name.len() > 256 {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_track_name",
                "track id or name is invalid",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.set_track_name(id, name) {
            return format!("{{\"ok\":true,\"track_id\":{id}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "track rename was rejected",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn new_project(&self) {
        if let Some(e) = self.engine.as_ref() {
            e.new_project();
        }
        if let Ok(mut chords) = self.chord_track.lock() {
            chords.clear();
        }
        if let Ok(mut history) = self.chord_history.lock() {
            history.clear();
        }
        if let Ok(mut redo) = self.chord_redo_history.lock() {
            redo.clear();
        }
        if let Ok(mut history) = self.midi_lyric_history.lock() {
            history.clear();
        }
        self.reset_markers();
    }
    pub fn duplicate_track(&self, id: u32) -> u32 {
        self.engine.as_ref().map_or(0, |e| e.duplicate_track(id))
    }

    pub fn duplicate_track_diagnostic_json(&self, id: u32) -> String {
        if id == 0 {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_track_id",
                    "track id must be non-zero",
                )
                .object(format!("track:{id}")),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let new_id = engine.duplicate_track(id);
        if new_id != 0 {
            return format!("{{\"ok\":true,\"source_track_id\":{id},\"track_id\":{new_id}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "track duplication was rejected",
            )
            .object(format!("track:{id}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn add_region(&self, tid: u32, path: &str, start: f64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.add_region(tid, path, start))
    }

    pub fn add_region_diagnostic_json(&self, tid: u32, path: &str, start: f64) -> String {
        if tid == 0 || path.is_empty() || path.contains('\0') || !start.is_finite() || start < 0.0 {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_region_input",
                "region target, path, or start position is invalid",
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if !Path::new(path).is_file() {
            let error = crate::bridge_error::BridgeError::new(
                "region_audio_not_found",
                "region audio path is not a regular file",
            )
            .object(path)
            .retryable(true);
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.add_region(tid, path, start) {
            return format!("{{\"ok\":true,\"track_id\":{tid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "region_add_rejected",
                "native engine rejected region insertion",
            )
            .object(format!("track:{tid}"))
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
    pub fn add_region_at_beat(&self, tid: u32, path: &str, beat: f64) -> bool {
        self.add_region(tid, path, self.beats_to_samples(beat.max(0.0)) as f64)
    }
    pub fn replace_region_audio(&self, tid: u32, rid: u32, path: &str) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.replace_region_audio(tid, rid, path))
    }

    pub fn replace_region_audio_diagnostic_json(&self, tid: u32, rid: u32, path: &str) -> String {
        if tid == 0 || rid == 0 || path.is_empty() || path.contains('\0') {
            let error = crate::bridge_error::BridgeError::new(
                "invalid_region_input",
                "track, region, or audio path is invalid",
            )
            .object(format!("track:{tid}/region:{rid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if !Path::new(path).is_file() {
            let error = crate::bridge_error::BridgeError::new(
                "region_audio_not_found",
                "replacement audio path is not a regular file",
            )
            .object(path)
            .retryable(true);
            return serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.replace_region_audio(tid, rid, path) {
            return format!("{{\"ok\":true,\"track_id\":{tid},\"region_id\":{rid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "region_replace_rejected",
                "native engine rejected audio replacement",
            )
            .object(format!("track:{tid}/region:{rid}"))
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
}
