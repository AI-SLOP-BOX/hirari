impl AuraCore {
    pub fn enqueue_advanced_export_job_json(&self, job_json: &str) -> String {
        let Ok(job) = serde_json::from_str::<crate::advanced_export_engine::StemJobRust>(job_json) else {
            return r#"{"ok":false,"code":"invalid_advanced_export_job","retryable":false}"#.into();
        };
        let Ok(mut queue) = self.advanced_export.lock() else {
            return r#"{"ok":false,"code":"advanced_export_unavailable","retryable":true}"#.into();
        };
        let accepted = queue.queue_job(job);
        serde_json::json!({
            "ok": accepted,
            "queued": queue.active_jobs.len(),
            "error": queue.last_error
        }).to_string()
    }

    pub fn advanced_export_snapshot_json(&self) -> String {
        let Ok(queue) = self.advanced_export.lock() else {
            return r#"{"ok":false,"code":"advanced_export_unavailable","retryable":true}"#.into();
        };
        serde_json::json!({
            "ok": true,
            "active_jobs": queue.active_jobs,
            "last_error": queue.last_error,
            "renderer_connected": true
        }).to_string()
    }

    /// Executes the advanced stem queue through the native project renderer.
    /// The renderer callback is installed for this control-plane operation
    /// only; no audio callback or UI thread performs file I/O.
    pub fn execute_advanced_export_json(&self, output_dir: &str) -> String {
        let output = std::path::Path::new(output_dir);
        if !output.is_dir() {
            return serde_json::json!({"ok":false,"code":"invalid_export_directory"}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return r#"{"ok":false,"code":"engine_unavailable","retryable":true}"#.into();
        };
        let Ok(mut queue) = self.advanced_export.lock() else {
            return r#"{"ok":false,"code":"advanced_export_unavailable","retryable":true}"#.into();
        };
        let result = queue.execute_export_with_file_renderer(output, |track_id, path| {
            if engine.bounce_project(path.to_string_lossy().as_ref(), 0) {
                Ok(())
            } else {
                Err(format!("native bounce failed for track {track_id}"))
            }
        });
        match result {
            Ok(count) => serde_json::json!({"ok":true,"completed":count,"snapshot":queue.active_jobs.len()}).to_string(),
            Err(error) => serde_json::json!({"ok":false,"code":format!("{error:?}"),"queued":queue.active_jobs.len()}).to_string(),
        }
    }

    pub fn enqueue_export_job_json(&self, job_json: &str) -> String {
        let Ok(job) = serde_json::from_str::<crate::export::ExportJob>(job_json) else {
            return r#"{"ok":false,"code":"invalid_export_job","retryable":false}"#.into();
        };
        let Ok(mut queue) = self.export_queue.lock() else {
            return r#"{"ok":false,"code":"export_queue_unavailable","retryable":true}"#.into();
        };
        let before = queue.jobs.len();
        queue.add_job(job);
        serde_json::json!({"ok":queue.jobs.len() > before,"queued":queue.jobs.len(),"error":queue.last_error}).to_string()
    }

    pub fn export_queue_snapshot_json(&self) -> String {
        let Ok(queue) = self.export_queue.lock() else {
            return r#"{"ok":false,"code":"export_queue_unavailable","retryable":true}"#.into();
        };
        serde_json::json!({"ok":true,"jobs":queue.jobs,"last_error":queue.last_error}).to_string()
    }

    /// Runs queued WAV jobs through the native project bounce graph. Formats
    /// that require an external encoder fail explicitly instead of consuming
    /// the queue as if delivery succeeded.
    pub fn execute_export_queue_json(&self, output_dir: &str) -> String {
        let output = std::path::Path::new(output_dir);
        if output.as_os_str().is_empty() || !output.is_dir() {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_export_directory",
                "retryable": false
            }).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return r#"{"ok":false,"code":"engine_unavailable","retryable":true}"#.into();
        };
        let Ok(mut queue) = self.export_queue.lock() else {
            return r#"{"ok":false,"code":"export_queue_unavailable","retryable":true}"#.into();
        };
        let jobs = queue.jobs.clone();
        let mut completed = Vec::new();
        let mut completed_count = 0usize;
        for job in jobs {
            if !matches!(job.codec, crate::export::Codec::WAV) {
                queue.last_error = Some("external encoder required for requested codec".into());
                return serde_json::json!({"ok":false,"code":"external_encoder_required","completed":completed}).to_string();
            }
            let safe: String = job.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
            if safe.is_empty() {
                queue.last_error = Some("invalid export filename".into());
                return serde_json::json!({"ok":false,"code":"invalid_export_filename","completed":completed}).to_string();
            }
            // Never overwrite an existing render or collide two jobs with the
            // same display name.  The queue remains transactional: completed
            // jobs are removed only after every queued job has succeeded.
            let mut path = output.join(format!("{safe}.wav"));
            let mut suffix = 2usize;
            while path.exists() || completed.iter().any(|item: &String| item == path.to_string_lossy().as_ref()) {
                path = output.join(format!("{safe}_{suffix}.wav"));
                suffix += 1;
            }
            if !engine.bounce_project(path.to_string_lossy().as_ref(), 0) {
                queue.last_error = Some(format!("native bounce failed for {}", job.name));
                return serde_json::json!({"ok":false,"code":"bounce_failed","completed":completed}).to_string();
            }
            completed.push(path.to_string_lossy().to_string());
            completed_count += 1;
        }
        queue.jobs.drain(..completed_count);
        queue.last_error = None;
        serde_json::json!({"ok":true,"completed":completed}).to_string()
    }

    pub fn open_plugin_native_editor_json(&self, track: u32, plugin: u32, parent: u64) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return r#"{"ok":false,"code":"engine_unavailable","retryable":true}"#.into();
        };
        if !engine.has_plugin_native_editor(track, plugin) {
            return r#"{"ok":false,"code":"native_editor_unavailable","retryable":false}"#.into();
        }
        let session = engine.open_plugin_native_editor(track, plugin, parent);
        if session == 0 {
            return r#"{"ok":false,"code":"native_editor_host_unbound","retryable":true}"#.into();
        }
        serde_json::json!({"ok":true,"track":track,"plugin":plugin,"session":session,"embedded":true}).to_string()
    }

    pub fn close_plugin_native_editor_json(&self, track: u32, plugin: u32) -> String {
        let ok = self.engine.as_ref().is_some_and(|engine| {
            engine.close_plugin_native_editor(track, plugin)
        });
        serde_json::json!({"ok":ok,"track":track,"plugin":plugin}).to_string()
    }

    pub fn reset_sandboxed_plugin(&self, track_id: u32, index: u32) -> bool {
        self.restart_sandboxed_plugin(track_id, index)
    }
    pub fn plugin_editor_capability_diagnostic_json(&self, track: u32, index: u32) -> String {
        let native_editor = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.has_plugin_native_editor(track, index));
        let embedded = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.plugin_native_editor_embedded(track, index));
        // Detecting a vendor editor is not the same thing as embedding its
        // HWND/NSView.  Keep that distinction explicit so clients cannot
        // accidentally present the parameter fallback as a native editor.
        serde_json::json!({
            "ok": true,
            "native_editor": native_editor,
            "embedded": embedded,
            "embedding": if native_editor { "ui_thread_required" } else { "parameter_only" },
            "host_state": if embedded { "embedded" } else if native_editor { "available_not_embedded" } else { "parameter_fallback" },
        })
        .to_string()
    }
    pub fn plugin_parameter_snapshot_json(&self, track: u32, plugin: u32) -> String {
        let count = self.get_plugin_parameter_count(track, plugin).min(2048);
        let parameters = (0..count).map(|id| serde_json::json!({"id":id,"name":self.get_plugin_parameter_name(track,plugin,id),"normalized":self.get_plugin_parameter(track,plugin,id)})).collect::<Vec<_>>();
        let native_editor = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.has_plugin_native_editor(track, plugin));
        let embedded = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.plugin_native_editor_embedded(track, plugin));
        serde_json::json!({
            "ok": true,
            "parameters": parameters,
            "native_editor": native_editor,
            "embedded": embedded,
            "host_state": if embedded { "embedded" } else if native_editor { "available_not_embedded" } else { "parameter_fallback" },
        })
        .to_string()
    }
    pub fn pause_render(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.pause_bounce())
    }
    pub fn resume_render(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.resume_bounce())
    }
    pub fn capture_mix_snapshot_json(&self, name: &str) -> String {
        self.take_mix_snapshot_json(name, &self.control_snapshot_json())
    }
    pub fn mix_snapshot_catalog_json(&self) -> String {
        let names = self
            .mix_snapshots
            .lock()
            .map(|s| s.list_names())
            .unwrap_or_default();
        serde_json::json!({"ok":true,"snapshots":names}).to_string()
    }
    pub fn remove_mix_snapshot_json(&self, index: usize) -> String {
        let ok = self
            .mix_snapshots
            .lock()
            .ok()
            .and_then(|mut s| {
                s.snapshots
                    .get(index)
                    .map(|x| x.name.clone())
                    .map(|name| s.remove_snapshot(&name))
            })
            .unwrap_or(false);
        serde_json::json!({"ok":ok,"index":index}).to_string()
    }
    pub fn select_control_room_output(&self, index: u32) -> bool {
        self.select_control_room_speaker(index)
    }
    pub fn set_control_room_reference_enabled(&self, enabled: bool) -> bool {
        self.control_room
            .lock()
            .map(|mut state| state.set_reference_enabled(enabled))
            .unwrap_or(false)
    }
    pub fn render_queue_status_json(&self) -> String {
        match self.bounce_snapshot() {
            Some(snapshot) => serde_json::json!({"ok":true,"state":snapshot.state,"progress":snapshot.progress,"progress_available":snapshot.progress_available}).to_string(),
            None => r#"{"ok":false,"code":"engine_unavailable","retryable":true}"#.into(),
        }
    }
    pub fn midi_clock_status_json(&self) -> String {
        match self.engine.as_ref() {
            Some(engine) => serde_json::json!({"ok":true,"ticks":engine.midi_clock_ticks(),"last_tick":engine.midi_clock_last_tick(),"bpm":engine.midi_clock_rate()}).to_string(),
            None => r#"{"ok":false,"code":"engine_unavailable","retryable":true}"#.into(),
        }
    }
    pub fn midi_clock_tick_json(&self, timestamp: u64, bpm: Option<f64>) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return r#"{"ok":false,"code":"engine_unavailable","retryable":true}"#.into();
        };
        if let Some(rate) = bpm.filter(|value| value.is_finite() && (20.0..=300.0).contains(value))
        {
            engine.set_midi_clock_rate(rate);
        }
        engine.midi_clock_tick(timestamp);
        let mut sync = self
            .external_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sync.clock_tick();
        serde_json::json!({"ok":true,"timestamp":timestamp,"ticks":engine.midi_clock_ticks(),"bpm":engine.midi_clock_rate(),"beat":sync.beat,"running":sync.running}).to_string()
    }
    pub fn external_sync_status_json(&self) -> String {
        let sync = self
            .external_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        serde_json::json!({"ok":true,"enabled":sync.enabled,"protocol":format!("{:?}", sync.protocol),"source":format!("{:?}", sync.source),"running":sync.running,"beat":sync.beat,"timecode":sync.timecode}).to_string()
    }
    pub fn configure_external_sync_json(
        &self,
        protocol: &str,
        source: &str,
        enabled: bool,
    ) -> String {
        let protocol = match protocol.trim().to_ascii_lowercase().as_str() {
            "midi" | "midi_clock" | "midiclock" => crate::sync_transport::SyncProtocol::MidiClock,
            "mtc" => crate::sync_transport::SyncProtocol::Mtc,
            "ltc" => crate::sync_transport::SyncProtocol::Ltc,
            _ => return r#"{"ok":false,"code":"invalid_sync_protocol","retryable":false}"#.into(),
        };
        let source = match source.trim().to_ascii_lowercase().as_str() {
            "internal" => crate::sync_transport::SyncSource::Internal,
            value
                if value
                    .strip_prefix("midi:")
                    .and_then(|id| id.parse::<u32>().ok())
                    .is_some() =>
            {
                crate::sync_transport::SyncSource::MidiPort(
                    value
                        .strip_prefix("midi:")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0),
                )
            }
            value
                if value
                    .strip_prefix("audio:")
                    .and_then(|id| id.parse::<u32>().ok())
                    .is_some() =>
            {
                crate::sync_transport::SyncSource::AudioDevice(
                    value
                        .strip_prefix("audio:")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0),
                )
            }
            value
                if value
                    .strip_prefix("network:")
                    .and_then(|id| id.parse::<u16>().ok())
                    .is_some() =>
            {
                crate::sync_transport::SyncSource::Network(
                    value
                        .strip_prefix("network:")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0),
                )
            }
            _ => return r#"{"ok":false,"code":"invalid_sync_source","retryable":false}"#.into(),
        };
        let mut sync = self
            .external_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !source.validate()
            || (enabled
                && source == crate::sync_transport::SyncSource::Internal
                && protocol != crate::sync_transport::SyncProtocol::MidiClock)
        {
            return r#"{"ok":false,"code":"invalid_sync_configuration","retryable":false}"#.into();
        }
        sync.select_source(source);
        sync.configure(protocol, enabled);
        serde_json::json!({"ok":true,"enabled":sync.enabled,"protocol":format!("{:?}", sync.protocol),"source":format!("{:?}", sync.source),"running":sync.running}).to_string()
    }
    pub fn external_sync_mmc_json(&self, bytes: &[u8]) -> String {
        let Some(command) = crate::sync_transport::decode_mmc(bytes) else {
            return r#"{"ok":false,"code":"invalid_mmc_packet","retryable":false}"#.into();
        };
        let mut sync = self
            .external_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !sync.apply_mmc(command) {
            return r#"{"ok":false,"code":"sync_disabled","retryable":false}"#.into();
        }
        if let Some(engine) = self.engine.as_ref() {
            match command {
                crate::sync_transport::MmcCommand::Stop
                | crate::sync_transport::MmcCommand::RecordPunchOut => {
                    let _ = engine.try_set_playing(false);
                }
                crate::sync_transport::MmcCommand::Play
                | crate::sync_transport::MmcCommand::DeferredPlay
                | crate::sync_transport::MmcCommand::RecordPunchIn
                | crate::sync_transport::MmcCommand::FastForward
                | crate::sync_transport::MmcCommand::Rewind => {
                    let rate = engine.get_sample_rate();
                    let bpm = engine.get_tempo();
                    if rate.is_finite() && bpm.is_finite() && rate > 0.0 && bpm > 0.0 {
                        let samples = (sync.beat * rate * 60.0 / f64::from(bpm)).round();
                        if samples.is_finite() && samples >= 0.0 {
                            engine.set_playhead(samples.min(u64::MAX as f64) as u64);
                        }
                    }
                    let _ = engine.try_set_playing(true);
                }
            }
        }
        serde_json::json!({"ok":true,"operation":"mmc","running":sync.running,"beat":sync.beat})
            .to_string()
    }
    pub fn external_sync_mtc_json(&self, h: u8, m: u8, s: u8, f: u8, fps: u8) -> String {
        let timecode = crate::sync_transport::Timecode {
            hours: h,
            minutes: m,
            seconds: s,
            frames: f,
            fps,
        };
        let mut sync = self
            .external_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if sync.set_timecode(timecode).is_err() {
            return r#"{"ok":false,"code":"invalid_mtc_timecode","retryable":false}"#.into();
        }
        if let Some(engine) = self.engine.as_ref() {
            let rate = engine.get_sample_rate();
            if rate.is_finite() && rate > 0.0 {
                let samples =
                    (timecode.frame_index() as f64 * rate / f64::from(timecode.fps)).round();
                if samples.is_finite() && samples >= 0.0 {
                    engine.set_playhead(samples.min(u64::MAX as f64) as u64);
                }
            }
            let _ = engine.try_set_playing(sync.running);
        }
        serde_json::json!({"ok":true,"operation":"mtc","running":sync.running,"timecode":timecode})
            .to_string()
    }
    pub fn export_midi_file(&self, path: &str) -> Result<usize, String> {
        let notes = self
            .scheduled_midi_notes
            .lock()
            .map_err(|_| "MIDI note state unavailable".to_owned())?
            .clone();
        let Some(engine) = self.engine.as_ref() else {
            return Err("audio engine unavailable".into());
        };
        crate::midi_file::write_standard_midi(
            std::path::Path::new(path),
            &notes,
            engine.get_sample_rate(),
            engine.get_tempo() as f64,
        )
    }
    pub fn set_midi_note_vibrato_rate_without_undo(
        &self,
        track: u32,
        pitch: u8,
        start: u64,
        rate: u16,
    ) -> bool {
        if track == 0 || !(500..=20_000).contains(&rate) {
            return false;
        }
        let note_exists = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| {
                notes.iter().any(|note| {
                    note.track_id == track && note.pitch == pitch && note.start_sample == start
                })
            })
            .unwrap_or(false);
        if !note_exists {
            return false;
        }
        let Ok(mut rates) = self.midi_vibrato_rates.lock() else {
            return false;
        };
        rates.insert((track, pitch, start), rate);
        true
    }
}
