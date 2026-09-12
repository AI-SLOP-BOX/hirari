impl AuraCore {
    pub fn push_midi_monitor_event(
        &self,
        timestamp: u64,
        status: u8,
        data1: u8,
        data2: u8,
    ) -> bool {
        if status & 0x80 == 0 || data1 > 127 || data2 > 127 {
            return false;
        }
        self.midi_monitor
            .lock()
            .map(|mut m| {
                m.push(crate::midi_monitor::MidiMonitorEvent {
                    timestamp,
                    status,
                    data1,
                    data2,
                })
            })
            .is_ok()
    }
    pub fn clear_midi_monitor(&self) {
        if let Ok(mut m) = self.midi_monitor.lock() {
            m.clear();
        }
    }
    pub fn midi_monitor_json(&self, count: usize) -> String {
        self.midi_monitor
            .lock()
            .map(|m| {
                serde_json::json!({"ok":true,"events":m.snapshot(count.min(4096))}).to_string()
            })
            .unwrap_or_else(|_| "{\"ok\":false}".to_owned())
    }
    /// Returns silent sample ranges without modifying the project or audio.
    pub fn detect_silence(&self, samples: &[f32], threshold: f32, min_length: usize) -> String {
        let ranges = crate::silence_detector::detect_silence(samples, threshold, min_length);
        serde_json::json!({
            "ok": true,
            "operation": "detect_silence",
            "threshold": threshold,
            "min_length": min_length,
            "ranges": ranges.into_iter().map(|range| serde_json::json!({"start": range.start, "length": range.length})).collect::<Vec<_>>(),
        }).to_string()
    }

    /// Adds a canonical chord-track event for composition and arrangement clients.
    pub fn add_chord_event(&self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) -> bool {
        if name.trim().is_empty() || intervals.iter().any(|interval| *interval > 127) {
            return false;
        }
        let before = self
            .chord_track
            .lock()
            .map(|track| track.clone())
            .unwrap_or_default();
        let Ok(mut track) = self.chord_track.lock() else {
            return false;
        };
        track.push(crate::harmonic::ChordEvent {
            tick,
            root: root.min(127),
            intervals,
            name: name.chars().take(128).collect(),
        });
        track.sort_by_key(|event| event.tick);
        let after = track.clone();
        drop(track);
        if let Ok(mut history) = self.chord_history.lock() {
            history.push(crate::ChordHistoryEntry {
                depth_after: 0,
                before,
                after,
            });
        }
        if let Ok(mut redo) = self.chord_redo_history.lock() {
            redo.clear();
        }
        true
    }

    pub fn chord_track_json(&self) -> String {
        let track = self
            .chord_track
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();
        serde_json::to_string(&track).unwrap_or_else(|_| "[]".to_owned())
    }

    fn record_chord_snapshot(
        &self,
        before: Vec<crate::harmonic::ChordEvent>,
        after: Vec<crate::harmonic::ChordEvent>,
    ) {
        if before == after {
            return;
        }
        if let Ok(mut history) = self.chord_history.lock() {
            history.push(crate::ChordHistoryEntry {
                depth_after: 0,
                before,
                after,
            });
        }
        if let Ok(mut redo) = self.chord_redo_history.lock() {
            redo.clear();
        }
    }

    pub fn remove_chord_events_range(&self, start_tick: u64, end_tick: u64) -> usize {
        let Ok(mut track) = self.chord_track.lock() else {
            return 0;
        };
        let before = track.clone();
        let old_len = track.len();
        track.retain(|event| event.tick < start_tick || event.tick > end_tick);
        let after = track.clone();
        drop(track);
        self.record_chord_snapshot(before, after);
        old_len.saturating_sub(
            self.chord_track
                .lock()
                .map(|track| track.len())
                .unwrap_or(old_len),
        )
    }

    pub fn clear_chord_track(&self) -> usize {
        let Ok(mut track) = self.chord_track.lock() else {
            return 0;
        };
        let before = track.clone();
        let removed = track.len();
        track.clear();
        let after = track.clone();
        drop(track);
        self.record_chord_snapshot(before, after);
        removed
    }

    fn region_diagnostic<F>(&self, op: &str, tid: u32, rid: u32, valid: bool, apply: F) -> String
    where
        F: FnOnce(&crate::ffi::AudioEngine) -> bool,
    {
        if tid == 0 || rid == 0 {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_region_target",
                "track and region ids must be non-zero",
            )
            .object(format!("track:{tid}/region:{rid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if !valid {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_region_value",
                format!("{op} received an invalid value"),
            )
            .object(format!("track:{tid}/region:{rid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if apply(engine) {
            return format!(
                "{{\"ok\":true,\"operation\":\"{op}\",\"track_id\":{tid},\"region_id\":{rid}}}"
            );
        } else {
            crate::bridge_error::BridgeError::new(
                "region_not_found_or_rejected",
                format!("{op} was rejected"),
            )
            .object(format!("track:{tid}/region:{rid}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn move_region_diagnostic_json(&self, tid: u32, rid: u32, start: f64) -> String {
        self.region_diagnostic(
            "move_region",
            tid,
            rid,
            start.is_finite() && start >= 0.0,
            |e| e.move_region(tid, rid, self.beats_to_samples(start) as f64),
        )
    }

    pub fn split_region_diagnostic_json(&self, tid: u32, rid: u32, beat: f64) -> String {
        self.region_diagnostic(
            "split_region",
            tid,
            rid,
            beat.is_finite() && beat > 0.0,
            |e| e.split_region(tid, rid, self.beats_to_samples(beat) as f64),
        )
    }

    pub fn set_region_gain_diagnostic_json(&self, tid: u32, rid: u32, gain: f32) -> String {
        self.region_diagnostic(
            "set_region_gain",
            tid,
            rid,
            gain.is_finite() && (-24.0..=24.0).contains(&gain),
            |e| e.set_region_gain(tid, rid, gain),
        )
    }

    pub fn set_region_reverse_diagnostic_json(&self, tid: u32, rid: u32, reverse: bool) -> String {
        self.region_diagnostic("set_region_reverse", tid, rid, true, |e| {
            e.set_region_reverse(tid, rid, reverse)
        })
    }

    pub fn set_region_muted_diagnostic_json(&self, tid: u32, rid: u32, muted: bool) -> String {
        self.region_diagnostic("set_region_muted", tid, rid, true, |e| {
            e.set_region_muted(tid, rid, muted)
        })
    }

    pub fn set_region_fades_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        fade_in: f32,
        fade_out: f32,
    ) -> String {
        let valid = fade_in.is_finite()
            && fade_out.is_finite()
            && (0.0..=1.0).contains(&fade_in)
            && (0.0..=1.0).contains(&fade_out);
        self.region_diagnostic("set_region_fades", tid, rid, valid, |e| {
            e.set_region_fades(tid, rid, fade_in, fade_out)
        })
    }

    pub fn set_region_trim_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        start: f32,
        end: f32,
    ) -> String {
        let valid = start.is_finite()
            && end.is_finite()
            && (0.0..=1.0).contains(&start)
            && (0.0..=1.0).contains(&end)
            && start < end;
        self.region_diagnostic("set_region_trim", tid, rid, valid, |e| {
            e.set_region_trim(tid, rid, start, end)
        })
    }

    pub fn set_region_warp_ratio_diagnostic_json(&self, tid: u32, rid: u32, ratio: f64) -> String {
        self.region_diagnostic(
            "set_region_warp_ratio",
            tid,
            rid,
            ratio.is_finite() && (0.25..=4.0).contains(&ratio),
            |e| e.set_region_warp_ratio(tid, rid, ratio),
        )
    }

    pub fn set_region_pitch_diagnostic_json(&self, tid: u32, rid: u32, semitones: f32) -> String {
        self.region_diagnostic(
            "set_region_pitch",
            tid,
            rid,
            semitones.is_finite() && (-48.0..=48.0).contains(&semitones),
            |e| e.set_region_pitch_semitones(tid, rid, semitones),
        )
    }

    pub fn set_region_loop_diagnostic_json(&self, tid: u32, rid: u32, count: u32) -> String {
        self.region_diagnostic(
            "set_region_loop_count",
            tid,
            rid,
            (1..=1024).contains(&count),
            |e| e.set_region_loop_count(tid, rid, count),
        )
    }

    pub fn request_video_frame_diagnostic_json(&self, seconds: f64) -> String {
        if !seconds.is_finite() || seconds < 0.0 {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_video_position",
                "video position must be finite and non-negative",
            );
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.request_video_frame(seconds) {
            return format!(
                "{{\"ok\":true,\"operation\":\"request_video_frame\",\"seconds\":{seconds}}}"
            );
        } else {
            crate::bridge_error::BridgeError::new(
                "video_request_rejected",
                "video frame request was rejected",
            )
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn load_video_diagnostic_json(&self, path: &str) -> String {
        let path = std::path::Path::new(path);
        if path.as_os_str().is_empty() || path.to_string_lossy().contains('\0') {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_video_path",
                "video path is empty or invalid",
            );
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        if !path.is_file() {
            let result = crate::bridge_error::BridgeError::new(
                "video_not_found",
                "video path does not refer to a regular file",
            )
            .retryable(true);
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.load_video(path.to_string_lossy().as_ref()) {
            return "{\"ok\":true,\"operation\":\"load_video\"}".to_owned();
        } else {
            crate::bridge_error::BridgeError::new(
                "video_load_rejected",
                "video loading was rejected",
            )
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    fn track_diagnostic<F>(&self, op: &str, tid: u32, valid: bool, apply: F) -> String
    where
        F: FnOnce(&crate::ffi::AudioEngine) -> bool,
    {
        if tid == 0 || !valid {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_track_command",
                format!("{op} received an invalid target or value"),
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation());
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if apply(engine) {
            return format!("{{\"ok\":true,\"operation\":\"{op}\",\"track_id\":{tid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                format!("{op} was rejected"),
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_spatial_position_diagnostic_json(&self, tid: u32, x: f32, y: f32, z: f32) -> String {
        let valid = x.is_finite()
            && y.is_finite()
            && z.is_finite()
            && (-1.0..=1.0).contains(&x)
            && (-1.0..=1.0).contains(&y)
            && (-1.0..=1.0).contains(&z);
        self.track_diagnostic("set_spatial_position", tid, valid, |e| {
            e.set_spatial_position(tid, x, y, z)
        })
    }

    pub fn set_track_armed_diagnostic_json(&self, tid: u32, armed: bool) -> String {
        self.track_diagnostic("set_track_armed", tid, true, |e| {
            e.set_track_armed(tid, armed)
        })
    }

    pub fn set_phase_invert_diagnostic_json(&self, tid: u32, inverted: bool) -> String {
        self.track_diagnostic("set_phase_invert", tid, true, |e| {
            e.set_phase_invert(tid, inverted)
        })
    }

    pub fn set_track_eq_diagnostic_json(
        &self,
        tid: u32,
        low_gain: f32,
        low_cut: f32,
        high_gain: f32,
        high_cut: f32,
    ) -> String {
        let valid = [low_gain, low_cut, high_gain, high_cut]
            .iter()
            .all(|value| value.is_finite())
            && (-24.0..=24.0).contains(&low_gain)
            && (0.0..=1.0).contains(&low_cut)
            && (-24.0..=24.0).contains(&high_gain)
            && (0.0..=1.0).contains(&high_cut);
        if tid == 0 || !valid {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_track_command",
                    "set_track_eq received an invalid target or value",
                )
                .object(format!("track:{tid}"))
                .at_generation(self.project_generation()),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "engine_unavailable",
                    "audio engine unavailable",
                )
                .retryable(true),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        };
        if engine.set_track_eq(tid, low_gain, low_cut, high_gain, high_cut) {
            format!("{{\"ok\":true,\"operation\":\"set_track_eq\",\"track_id\":{tid}}}")
        } else {
            serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "track_not_found_or_rejected",
                    "set_track_eq was rejected",
                )
                .object(format!("track:{tid}"))
                .at_generation(self.project_generation()),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
        }
    }
}
