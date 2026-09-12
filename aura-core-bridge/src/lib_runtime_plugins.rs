impl AuraCore {
    pub fn installed_plugin_catalog_json(&self) -> String {
        crate::plugin_catalog::json()
    }

    pub fn set_plugin_favorite_diagnostic_json(&self, id: &str, favorite: bool) -> String {
        match crate::plugin_catalog::set_favorite(id, favorite) {
            Ok(changed) => serde_json::json!({
                "ok": true, "operation": "set_plugin_favorite", "id": id,
                "favorite": favorite, "changed": changed,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false, "code": "plugin_favorite_failed", "message": error.to_string(),
            })
            .to_string(),
        }
    }

    pub fn quarantine_plugin_diagnostic_json(&self, path: &str) -> String {
        match crate::plugin_catalog::quarantine_path(path) {
            Ok(plugin) => serde_json::json!({
                "ok": true,
                "operation": "quarantine_plugin",
                "plugin": plugin,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "plugin_quarantine_failed",
                "message": error,
            })
            .to_string(),
        }
    }

    pub fn clear_plugin_quarantine_diagnostic_json(&self, path: &str) -> String {
        match crate::plugin_catalog::clear_quarantine_path(path) {
            Ok(plugin) => serde_json::json!({
                "ok": true,
                "operation": "clear_plugin_quarantine",
                "plugin": plugin,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "plugin_quarantine_clear_failed",
                "message": error,
            })
            .to_string(),
        }
    }

    /// Insert a named installed plugin through the same sandbox admission path
    /// used by project hydration. The catalog's worker capability probe is
    /// authoritative: CLAP, AU, and VST3 are only offered when the packaged
    /// worker reports that exact backend as available for this build.
    pub fn add_named_sandboxed_plugin_diagnostic_json(&self, track_id: u32, alias: &str) -> String {
        let Some(plugin) = crate::plugin_catalog::resolve(alias) else {
            return serde_json::json!({"ok":false,"code":"plugin_not_found","alias":alias})
                .to_string();
        };
        if plugin.quarantined {
            return serde_json::json!({
                "ok": false,
                "code": "plugin_quarantined",
                "alias": alias,
                "binary_hash": plugin.binary_hash,
                "path": plugin.path,
            })
            .to_string();
        }
        if plugin.format == "internal" {
            let plugin_type = match plugin.path.as_str() {
                "Aura/Limiter" => 0,
                "Aura/Compressor" => 1,
                "Aura/Gate" => 2,
                "Aura/Saturation" => 3,
                "Aura/Transient" => 4,
                "Aura/DeEsser" => 5,
                "Aura/Delay" => 6,
                "Aura/Reverb" => 7,
                "Aura/DynamicEQ" => 8,
                "Aura/MidSide" => 9,
                "Aura/Width" => 10,
                _ => {
                    return serde_json::json!({
                        "ok": false,
                        "code": "unknown_internal_plugin",
                        "path": plugin.path,
                    })
                    .to_string()
                }
            };
            let result = self.add_plugin_diagnostic_json(track_id, plugin_type);
            return serde_json::json!({
                "plugin": plugin,
                "result": serde_json::from_str::<serde_json::Value>(&result)
                    .unwrap_or_else(|_| serde_json::json!({"ok":false,"code":"invalid_native_result"})),
            }).to_string();
        }
        if !plugin.sandboxed {
            return serde_json::json!({"ok":false,"code":"plugin_format_not_ready","format":plugin.format,"path":plugin.path}).to_string();
        }
        let result = self.add_sandboxed_plugin_diagnostic_json(track_id, &plugin.path);
        serde_json::json!({"plugin":plugin,"result":serde_json::from_str::<serde_json::Value>(&result).unwrap_or_else(|_| serde_json::json!({"ok":false,"code":"invalid_native_result"}))}).to_string()
    }

    pub fn openutau_status_json(&self) -> String {
        serde_json::to_string(&crate::openutau::status())
            .unwrap_or_else(|_| "{\"installed\":false}".into())
    }

    /// Return the bounded structured note model used by the in-DAW vocal
    /// editor. Unlike note_preview(), this keeps timing, pitch, and lyric
    /// fields machine-readable for CLI/LLM tooling and piano-roll editing.
    pub fn openutau_notes_json(&self, source_path: &str) -> String {
        if let Err(error) = crate::openutau::validate_source_file(source_path) {
            return serde_json::json!({"ok": false, "code": "openutau_source_rejected", "error": error}).to_string();
        }
        let notes = crate::openutau::parse_notes(source_path);
        serde_json::json!({
            "ok": true,
            "source_path": source_path,
            "note_count": notes.len(),
            "notes": notes,
        })
        .to_string()
    }

    pub fn openutau_midi_notes_json(
        &self,
        source_path: &str,
        track_id: u32,
        sample_rate: u32,
        ticks_per_beat: u32,
    ) -> String {
        match crate::openutau::notes_as_midi(source_path, track_id, sample_rate, ticks_per_beat) {
            Ok(notes) => serde_json::json!({
                "ok": true,
                "operation": "openutau_midi_notes",
                "source_path": source_path,
                "track_id": track_id,
                "sample_rate": sample_rate,
                "ticks_per_beat": ticks_per_beat,
                "notes": notes,
            })
            .to_string(),
            Err(error) => serde_json::json!({
                "ok": false,
                "code": "openutau_midi_conversion_failed",
                "error": error,
            })
            .to_string(),
        }
    }

    pub fn openutau_midi_notes_at_bpm_json(
        &self,
        source_path: &str,
        track_id: u32,
        sample_rate: u32,
        ticks_per_beat: u32,
        _bpm: f64,
    ) -> String {
        self.openutau_midi_notes_json(source_path, track_id, sample_rate, ticks_per_beat)
    }

    /// Validate an OpenUtau source/render pair before it becomes a project
    /// vocal region. The actual render is intentionally external to the
    /// realtime engine; once rendered, the pair is fully reproducible from
    /// project metadata and the referenced audio asset.
    pub fn openutau_import_diagnostic_json(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
    ) -> String {
        let import = crate::openutau::audit_import_files(source_path, rendered_audio_path);
        if let Ok(audit) = import {
            return serde_json::json!({
                "ok": true,
                "source_path": source_path,
                "rendered_audio_path": rendered_audio_path,
                "source_hash": audit.source_hash,
                "rendered_audio_hash": audit.rendered_audio_hash,
                "rendered_audio_bytes": audit.rendered_audio_bytes,
                "rendered_sample_rate": audit.rendered_sample_rate,
                "rendered_channels": audit.rendered_channels,
                "rendered_frames": audit.rendered_frames,
                "source_note_count": audit.source_note_count,
                "source_singers": audit.source_singers,
                "openutau": crate::openutau::status(),
            })
            .to_string();
        }
        serde_json::json!({
            "ok": false,
            "code": "openutau_import_rejected",
            "error": import.err(),
        })
        .to_string()
    }

    pub fn register_openutau_vocal(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
    ) -> anyhow::Result<()> {
        let audit = crate::openutau::audit_import_files(source_path, rendered_audio_path)
            .map_err(anyhow::Error::msg)?;
        let mut vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?;
        if !vocals.iter().any(|entry| {
            entry.source_path == source_path && entry.rendered_audio_path == rendered_audio_path
        }) {
            vocals.push(OpenUtauVocalContract {
                source_path: source_path.to_owned(),
                rendered_audio_path: rendered_audio_path.to_owned(),
                singer: String::new(),
                source_generation: self.project_generation(),
                source_hash: audit.source_hash,
                rendered_audio_hash: audit.rendered_audio_hash,
                rendered_audio_bytes: audit.rendered_audio_bytes,
                rendered_sample_rate: audit.rendered_sample_rate,
                rendered_channels: audit.rendered_channels,
                rendered_frames: audit.rendered_frames,
                source_note_count: audit.source_note_count,
                source_singers: audit.source_singers,
                tuning: OpenUtauTuningContract {
                    scoop: 0.35,
                    vibrato: 0.45,
                    dynamics: 0.60,
                    consonants: 0.50,
                },
            });
        }
        Ok(())
    }

    pub fn set_openutau_tuning(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
        scoop: f32,
        vibrato: f32,
        dynamics: f32,
        consonants: f32,
    ) -> anyhow::Result<()> {
        let values = [scoop, vibrato, dynamics, consonants];
        if values
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(anyhow::anyhow!(
                "OpenUtau tuning values must be normalized 0..=1"
            ));
        }
        let mut vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?;
        let Some(vocal) = vocals.iter_mut().find(|entry| {
            entry.source_path == source_path && entry.rendered_audio_path == rendered_audio_path
        }) else {
            return Err(anyhow::anyhow!("OpenUtau vocal source is not registered"));
        };
        vocal.tuning = OpenUtauTuningContract {
            scoop,
            vibrato,
            dynamics,
            consonants,
        };
        Ok(())
    }

    pub fn unregister_openutau_vocal(
        &self,
        source_path: &str,
        rendered_audio_path: &str,
    ) -> anyhow::Result<()> {
        let mut vocals = self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))?;
        vocals.retain(|entry| {
            !(entry.source_path == source_path && entry.rendered_audio_path == rendered_audio_path)
        });
        Ok(())
    }
}
