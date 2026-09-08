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

    pub fn move_region(&self, tid: u32, rid: u32, start: f64) -> bool {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return false;
        }
        let start_samples = self.beats_to_samples(start.max(0.0)) as f64;
        self.engine
            .as_ref()
            .is_some_and(|e| e.move_region(tid, rid, start_samples))
    }
    pub fn split_region(&self, tid: u32, rid: u32, beat: f64) -> bool {
        if tid == 0 || rid == 0 || !beat.is_finite() || beat <= 0.0 {
            return false;
        }
        let split_samples = self.beats_to_samples(beat.max(0.0)) as f64;
        self.engine
            .as_ref()
            .is_some_and(|e| e.split_region(tid, rid, split_samples))
    }
    pub fn duplicate_region(&self, tid: u32, rid: u32, start: f64) -> u32 {
        if tid == 0 || rid == 0 || !start.is_finite() || start < 0.0 {
            return 0;
        }
        self.engine.as_ref().map_or(0, |e| {
            e.duplicate_region(tid, rid, self.beats_to_samples(start))
        })
    }
    pub fn remove_region(&self, tid: u32, rid: u32) -> bool {
        if tid == 0 || rid == 0 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.remove_region(tid, rid))
    }
    pub fn set_region_gain(&self, tid: u32, rid: u32, gain: f32) -> bool {
        if !gain.is_finite() || !(-24.0..=24.0).contains(&gain) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_gain(tid, rid, gain))
    }
    pub fn set_region_fades(&self, tid: u32, rid: u32, fade_in: f32, fade_out: f32) -> bool {
        if !fade_in.is_finite()
            || !fade_out.is_finite()
            || !(0.0..=1.0).contains(&fade_in)
            || !(0.0..=1.0).contains(&fade_out)
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_fades(tid, rid, fade_in, fade_out))
    }
    pub fn set_region_reverse(&self, tid: u32, rid: u32, reverse: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_reverse(tid, rid, reverse))
    }

    pub fn set_region_trim(&self, tid: u32, rid: u32, start_norm: f32, end_norm: f32) -> bool {
        if !start_norm.is_finite()
            || !end_norm.is_finite()
            || !(0.0..=1.0).contains(&start_norm)
            || !(0.0..=1.0).contains(&end_norm)
            || start_norm >= end_norm
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_trim(tid, rid, start_norm, end_norm))
    }

    pub fn set_region_warp_ratio(&self, tid: u32, rid: u32, ratio: f64) -> bool {
        if !ratio.is_finite() || !(0.25..=4.0).contains(&ratio) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_warp_ratio(tid, rid, ratio))
    }

    pub fn set_region_pitch_semitones(&self, tid: u32, rid: u32, semitones: f32) -> bool {
        if !semitones.is_finite() || !(-48.0..=48.0).contains(&semitones) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_pitch_semitones(tid, rid, semitones))
    }

    /// Upserts a VariAudio-style pitch/formant segment on an audio region.
    /// Segment edits stay non-destructive and are consumed by the native
    /// renderer on the next published region snapshot.
    pub fn set_region_audio_note_segment(
        &self,
        tid: u32,
        rid: u32,
        start_seconds: f64,
        end_seconds: f64,
        pitch_offset_cents: f64,
        formant_offset_cents: f64,
    ) -> bool {
        if !start_seconds.is_finite()
            || !end_seconds.is_finite()
            || end_seconds <= start_seconds
            || end_seconds - start_seconds > 24.0 * 60.0
            || !pitch_offset_cents.is_finite()
            || !formant_offset_cents.is_finite()
            || pitch_offset_cents.abs() > 4800.0
            || formant_offset_cents.abs() > 2400.0
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_region_audio_note_segment(
                tid,
                rid,
                start_seconds,
                end_seconds,
                pitch_offset_cents,
                formant_offset_cents,
            )
        })
    }

    pub fn set_region_audio_note_segment_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        start_seconds: f64,
        end_seconds: f64,
        pitch_offset_cents: f64,
        formant_offset_cents: f64,
    ) -> String {
        let valid = start_seconds.is_finite()
            && end_seconds.is_finite()
            && end_seconds > start_seconds
            && end_seconds - start_seconds <= 24.0 * 60.0
            && pitch_offset_cents.is_finite()
            && pitch_offset_cents.abs() <= 4800.0
            && formant_offset_cents.is_finite()
            && formant_offset_cents.abs() <= 2400.0;
        if !valid {
            return "{\"code\":\"invalid_audio_note_segment\",\"retryable\":false}".to_owned();
        }
        if self.set_region_audio_note_segment(
            tid,
            rid,
            start_seconds,
            end_seconds,
            pitch_offset_cents,
            formant_offset_cents,
        ) {
            format!(
                "{{\"ok\":true,\"operation\":\"set_region_audio_note_segment\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_segment_rejected\",\"retryable\":false}".to_owned()
        }
    }

    pub fn set_region_audio_note_anchor(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        position_seconds: f64,
        pitch_cents: f64,
        formant_cents: f64,
    ) -> bool {
        if !segment_start_seconds.is_finite()
            || segment_start_seconds < 0.0
            || !position_seconds.is_finite()
            || position_seconds < 0.0
            || !pitch_cents.is_finite()
            || pitch_cents.abs() > 4800.0
            || !formant_cents.is_finite()
            || formant_cents.abs() > 2400.0
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.set_region_audio_note_anchor(
                tid,
                rid,
                segment_start_seconds,
                position_seconds,
                pitch_cents,
                formant_cents,
            )
        })
    }

    pub fn set_region_audio_note_anchor_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        segment_start_seconds: f64,
        position_seconds: f64,
        pitch_cents: f64,
        formant_cents: f64,
    ) -> String {
        if self.set_region_audio_note_anchor(
            tid,
            rid,
            segment_start_seconds,
            position_seconds,
            pitch_cents,
            formant_cents,
        ) {
            format!(
                "{{\"ok\":true,\"operation\":\"set_region_audio_note_anchor\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_anchor_rejected\",\"retryable\":false}".to_owned()
        }
    }

    pub fn clear_region_audio_note_segments(&self, tid: u32, rid: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.clear_region_audio_note_segments(tid, rid))
    }

    pub fn warp_region_audio_note_segment(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        new_start: f64,
        new_end: f64,
    ) -> bool {
        if !segment_start.is_finite()
            || segment_start < 0.0
            || !new_start.is_finite()
            || new_start < 0.0
            || !new_end.is_finite()
            || new_end <= new_start
            || new_end - new_start > 24.0 * 60.0
        {
            return false;
        }
        self.engine.as_ref().is_some_and(|engine| {
            engine.warp_region_audio_note_segment(tid, rid, segment_start, new_start, new_end)
        })
    }

    pub fn remove_region_audio_note_segment(&self, tid: u32, rid: u32, segment_start: f64) -> bool {
        if !segment_start.is_finite() || segment_start < 0.0 {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.remove_region_audio_note_segment(tid, rid, segment_start))
    }

    pub fn warp_region_audio_note_segment_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
        new_start: f64,
        new_end: f64,
    ) -> String {
        if self.warp_region_audio_note_segment(tid, rid, segment_start, new_start, new_end) {
            format!("{{\"ok\":true,\"operation\":\"warp_region_audio_note_segment\",\"track_id\":{},\"region_id\":{}}}", tid, rid)
        } else {
            "{\"code\":\"audio_note_segment_warp_rejected\",\"retryable\":false}".to_owned()
        }
    }

    pub fn remove_region_audio_note_segment_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        segment_start: f64,
    ) -> String {
        if self.remove_region_audio_note_segment(tid, rid, segment_start) {
            format!("{{\"ok\":true,\"operation\":\"remove_region_audio_note_segment\",\"track_id\":{},\"region_id\":{}}}", tid, rid)
        } else {
            "{\"code\":\"audio_note_segment_not_found\",\"retryable\":false}".to_owned()
        }
    }

    pub fn clear_region_audio_note_segments_diagnostic_json(&self, tid: u32, rid: u32) -> String {
        if self.clear_region_audio_note_segments(tid, rid) {
            format!(
                "{{\"ok\":true,\"operation\":\"clear_region_audio_note_segments\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_segments_not_found\",\"retryable\":false}".to_owned()
        }
    }

    pub fn analyze_region_audio_note_segments(&self, tid: u32, rid: u32, sample_rate: f64) -> bool {
        if !sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.analyze_region_audio_note_segments(tid, rid, sample_rate))
    }

    pub fn analyze_region_audio_note_segments_diagnostic_json(
        &self,
        tid: u32,
        rid: u32,
        sample_rate: f64,
    ) -> String {
        if self.analyze_region_audio_note_segments(tid, rid, sample_rate) {
            format!(
                "{{\"ok\":true,\"operation\":\"analyze_region_audio_note_segments\",\"track_id\":{},\"region_id\":{}}}",
                tid, rid
            )
        } else {
            "{\"code\":\"audio_note_analysis_failed\",\"retryable\":true}".to_owned()
        }
    }

    pub fn set_region_loop_count(&self, tid: u32, rid: u32, count: u32) -> bool {
        if !(1..=1024).contains(&count) {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_region_loop_count(tid, rid, count))
    }

    pub fn clear_midi_notes(&self) {
        if let Some(e) = self.engine.as_ref() {
            e.clear_midi_notes();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.clear();
        }
    }

    pub fn clear_midi_notes_diagnostic_json(&self) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.clear_midi_notes();
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.clear();
        }
        "{\"ok\":true,\"operation\":\"clear_midi_notes\"}".to_owned()
    }

    pub fn remove_midi_notes_range_diagnostic_json(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if start_sample >= end_sample {
            return "{\"ok\":false,\"code\":\"invalid_midi_range\",\"retryable\":false}".to_owned();
        }
        if !engine.remove_midi_notes_range(track_id, start_sample, end_sample) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_notes_not_found",
                "retryable": false,
            })
            .to_string();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.retain(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                !(note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample)
            });
        }
        serde_json::json!({
            "ok": true,
            "operation": "remove_midi_notes_range",
            "track_id": track_id,
            "start_sample": start_sample,
            "end_sample": end_sample,
        })
        .to_string()
    }

    pub fn transpose_midi_notes_range_diagnostic_json(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        semitones: i32,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if track_id == 0 || start_sample >= end_sample || !(-127..=127).contains(&semitones) {
            return "{\"ok\":false,\"code\":\"invalid_midi_transpose\",\"retryable\":false}"
                .to_owned();
        }
        let notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes,
            Err(_) => {
                return "{\"ok\":false,\"code\":\"midi_state_unavailable\",\"retryable\":true}"
                    .to_owned();
            }
        };
        {
            let out_of_range = notes.iter().any(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample
                    && !(0..=127).contains(&(i32::from(note.pitch) + semitones))
            });
            if out_of_range {
                return serde_json::json!({
                    "ok": false,
                    "code": "midi_transpose_out_of_range",
                    "retryable": false,
                })
                .to_string();
            }
        }
        if !engine.transpose_midi_notes_range(track_id, start_sample, end_sample, semitones) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_transpose_rejected",
                "retryable": false,
            })
            .to_string();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            for note in notes.iter_mut().filter(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample
            }) {
                note.pitch = (i32::from(note.pitch) + semitones) as u8;
            }
        }
        serde_json::json!({
            "ok": true,
            "operation": "transpose_midi_notes_range",
            "track_id": track_id,
            "start_sample": start_sample,
            "end_sample": end_sample,
            "semitones": semitones,
        })
        .to_string()
    }

    pub fn move_midi_notes_range_diagnostic_json(
        &self,
        track_id: u32,
        start_sample: u64,
        end_sample: u64,
        delta_samples: i64,
    ) -> String {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if track_id == 0 || start_sample >= end_sample {
            return "{\"ok\":false,\"code\":\"invalid_midi_move_range\",\"retryable\":false}"
                .to_owned();
        }
        let notes = match self.scheduled_midi_notes.lock() {
            Ok(notes) => notes,
            Err(_) => {
                return "{\"ok\":false,\"code\":\"midi_state_unavailable\",\"retryable\":true}"
                    .to_owned();
            }
        };
        {
            let moves_before_zero = delta_samples < 0
                && notes.iter().any(|note| {
                    let note_end = note.start_sample.saturating_add(note.length_samples);
                    note.track_id == track_id
                        && note.start_sample < end_sample
                        && note_end > start_sample
                        && note.start_sample < delta_samples.unsigned_abs()
                });
            if moves_before_zero {
                return serde_json::json!({
                    "ok": false,
                    "code": "midi_move_before_zero",
                    "retryable": false,
                })
                .to_string();
            }
        }
        if !engine.move_midi_notes_range(track_id, start_sample, end_sample, delta_samples) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_move_rejected",
                "retryable": false,
            })
            .to_string();
        }
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            for note in notes.iter_mut().filter(|note| {
                let note_end = note.start_sample.saturating_add(note.length_samples);
                note.track_id == track_id
                    && note.start_sample < end_sample
                    && note_end > start_sample
            }) {
                note.start_sample = if delta_samples >= 0 {
                    note.start_sample.saturating_add(delta_samples as u64)
                } else {
                    note.start_sample
                        .saturating_sub(delta_samples.unsigned_abs())
                };
            }
        }
        serde_json::json!({
            "ok": true,
            "operation": "move_midi_notes_range",
            "track_id": track_id,
            "start_sample": start_sample,
            "end_sample": end_sample,
            "delta_samples": delta_samples,
        })
        .to_string()
    }

    pub fn set_midi_note(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
    ) {
        if let Some(e) = self.engine.as_ref() {
            e.set_midi_note(track_id, pitch, velocity, start_sample, length_samples);
        }
        if pitch <= 127
            && velocity != 0
            && length_samples != 0
            && start_sample.checked_add(length_samples).is_some()
        {
            if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
                let lyric = notes
                    .iter()
                    .find(|existing| {
                        existing.track_id == track_id
                            && existing.pitch == pitch
                            && existing.start_sample == start_sample
                    })
                    .map(|existing| existing.lyric.clone())
                    .unwrap_or_default();
                notes.retain(|existing| {
                    !(existing.track_id == track_id
                        && existing.pitch == pitch
                        && existing.start_sample == start_sample)
                });
                let note = crate::project_contracts::MidiNoteContract {
                    track_id,
                    pitch,
                    velocity,
                    start_sample,
                    length_samples,
                    lyric,
                    phoneme: String::new(),
                    pitch_curve_cents: Vec::new(),
                    vibrato_depth_cents: 0,
                    portamento_samples: 0,
                    probability: 100,
                    repeat_count: 1,
                };
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
    }

    pub fn set_midi_note_diagnostic_json(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
    ) -> String {
        if pitch > 127
            || velocity == 0
            || length_samples == 0
            || start_sample.checked_add(length_samples).is_none()
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_note",
                "retryable": false,
            })
            .to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        engine.set_midi_note(track_id, pitch, velocity, start_sample, length_samples);
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            let lyric = notes
                .iter()
                .find(|existing| {
                    existing.track_id == track_id
                        && existing.pitch == pitch
                        && existing.start_sample == start_sample
                })
                .map(|existing| existing.lyric.clone())
                .unwrap_or_default();
            notes.retain(|existing| {
                !(existing.track_id == track_id
                    && existing.pitch == pitch
                    && existing.start_sample == start_sample)
            });
            let note = crate::project_contracts::MidiNoteContract {
                track_id,
                pitch,
                velocity,
                start_sample,
                length_samples,
                lyric,
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            };
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        serde_json::json!({
            "ok": true,
            "operation": "set_midi_note",
            "track_id": track_id,
            "pitch": pitch,
            "velocity": velocity,
            "start_sample": start_sample,
            "length_samples": length_samples,
        })
        .to_string()
    }

    pub fn set_midi_note_lyric_diagnostic_json(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        lyric: &str,
    ) -> String {
        if pitch > 127
            || velocity == 0
            || length_samples == 0
            || start_sample.checked_add(length_samples).is_none()
            || lyric.len() > 1_024
            || lyric.contains('\0')
        {
            return serde_json::json!({
                "ok": false,
                "code": "invalid_midi_note_or_lyric",
                "retryable": false,
            })
            .to_string();
        }
        if self.engine.is_null() {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        }
        if !self.set_midi_note_lyric(
            track_id,
            pitch,
            velocity,
            start_sample,
            length_samples,
            lyric,
        ) {
            return serde_json::json!({
                "ok": false,
                "code": "midi_note_rejected",
                "retryable": true,
            })
            .to_string();
        }
        serde_json::json!({
            "ok": true,
            "operation": "set_midi_note",
            "track_id": track_id,
            "pitch": pitch,
            "velocity": velocity,
            "start_sample": start_sample,
            "length_samples": length_samples,
            "lyric": lyric,
        })
        .to_string()
    }

    /// Schedule a note and keep its vocal lyric in the canonical control
    /// plane. The native realtime note remains allocation-free; the lyric is
    /// intentionally persisted only in the project-side model.
    pub fn set_midi_note_lyric(
        &self,
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        lyric: &str,
    ) -> bool {
        if pitch > 127
            || velocity == 0
            || length_samples == 0
            || start_sample.checked_add(length_samples).is_none()
            || lyric.len() > 1_024
            || lyric.contains('\0')
        {
            return false;
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let before_metadata = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        let undo_before = engine.get_undo_count();
        engine.set_midi_note(track_id, pitch, velocity, start_sample, length_samples);
        let Ok(mut notes) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        // The native note is upserted by (track, pitch, start). Mirror the
        // same identity here so changing length/velocity while entering a
        // lyric cannot leave stale duplicate authoring records.
        notes.retain(|note| {
            !(note.track_id == track_id && note.pitch == pitch && note.start_sample == start_sample)
        });
        notes.push(crate::project_contracts::MidiNoteContract {
            track_id,
            pitch,
            velocity,
            start_sample,
            length_samples,
            lyric: lyric.to_owned(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        });
        let after_metadata = notes.clone();
        drop(notes);
        let depth_after = engine.get_undo_count();
        if depth_after > undo_before {
            if let Ok(mut history) = self.midi_lyric_history.lock() {
                history.retain(|entry| entry.depth_after <= undo_before);
                history.push(crate::MidiLyricHistoryEntry {
                    depth_after,
                    before: before_metadata,
                    after: after_metadata,
                });
            }
        }
        true
    }

    /// Update non-destructive vocal articulation without touching the
    /// realtime MIDI event. This keeps pitch curves and pronunciation safe
    /// for an editor while playback remains allocation-free.
    pub fn set_midi_note_articulation(
        &self,
        track_id: u32,
        pitch: u8,
        start_sample: u64,
        phoneme: &str,
        pitch_curve_cents: &[i16],
        vibrato_depth_cents: u16,
        portamento_samples: u32,
    ) -> bool {
        if pitch > 127
            || phoneme.len() > 128
            || phoneme.contains('\0')
            || pitch_curve_cents.len() > 256
        {
            return false;
        }
        let Ok(mut notes) = self.scheduled_midi_notes.lock() else {
            return false;
        };
        let Some(note) = notes.iter_mut().find(|note| {
            note.track_id == track_id && note.pitch == pitch && note.start_sample == start_sample
        }) else {
            return false;
        };
        note.phoneme = phoneme.to_owned();
        note.pitch_curve_cents = pitch_curve_cents.to_vec();
        note.vibrato_depth_cents = vibrato_depth_cents;
        note.portamento_samples =
            portamento_samples.min(note.length_samples.min(u64::from(u32::MAX)) as u32);
        let snapshot = notes.clone();
        drop(notes);
        // `set_midi_note` creates the native undo point immediately before
        // this metadata refinement. Extend that same entry so one user edit
        // undoes both the realtime note and its vocal articulation.
        if let Some(engine) = self.engine.as_ref() {
            let depth = engine.get_undo_count();
            if let Ok(mut history) = self.midi_lyric_history.lock() {
                if let Some(entry) = history
                    .iter_mut()
                    .rev()
                    .find(|entry| entry.depth_after == depth)
                {
                    entry.after = snapshot;
                }
            }
        }
        true
    }

    /// Atomically replace the realtime MIDI snapshot. The UI keeps lyrics and
    /// other authoring metadata in the canonical project model; this packed
    /// form is only the bounded, allocation-safe playback snapshot.
    pub fn replace_midi_notes(&self, packed: Vec<u64>, record_undo: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.replace_midi_notes(packed, record_undo))
    }

    /// Replace realtime notes and their authoring metadata as one undoable
    /// operation. This is the bridge used by data-driven MIDI editors.
    pub fn replace_midi_note_contracts(
        &self,
        notes: Vec<crate::project_contracts::MidiNoteContract>,
        record_undo: bool,
    ) -> bool {
        let packed = notes
            .iter()
            .flat_map(|note| {
                [
                    u64::from(note.track_id),
                    u64::from(note.pitch),
                    u64::from(note.velocity),
                    note.start_sample,
                    note.length_samples,
                ]
            })
            .collect::<Vec<_>>();
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        if !engine.replace_midi_notes(packed, record_undo) {
            return false;
        }
        if let Ok(mut metadata) = self.scheduled_midi_notes.lock() {
            *metadata = notes;
        }
        true
    }

    pub fn midi_notes_snapshot(&self) -> Vec<u64> {
        self.engine
            .as_ref()
            .map(|engine| engine.midi_notes_snapshot())
            .unwrap_or_default()
    }

    /// Reconcile the authoring mirror after native MIDI undo/redo. Native
    /// history owns the realtime snapshot; this keeps project saves and
    /// bounce extent calculations on the same note set without discarding
    /// lyrics attached to unchanged notes.
    fn sync_midi_note_metadata_from_engine(&self) {
        let packed = self.midi_notes_snapshot();
        let Ok(mut metadata) = self.scheduled_midi_notes.lock() else {
            return;
        };
        let old = std::mem::take(&mut *metadata);
        let mut synced = Vec::with_capacity(packed.len() / 5);
        for chunk in packed.chunks_exact(5) {
            let previous = old
                .iter()
                .find(|note| {
                    note.track_id as u64 == chunk[0]
                        && note.pitch as u64 == chunk[1]
                        && note.velocity as u64 == chunk[2]
                        && note.start_sample == chunk[3]
                        && note.length_samples == chunk[4]
                })
                .or_else(|| {
                    old.iter().find(|note| {
                        note.track_id as u64 == chunk[0]
                            && note.pitch as u64 == chunk[1]
                            && note.start_sample == chunk[3]
                    })
                });
            synced.push(crate::project_contracts::MidiNoteContract {
                track_id: chunk[0] as u32,
                pitch: chunk[1] as u8,
                velocity: chunk[2] as u8,
                start_sample: chunk[3],
                length_samples: chunk[4],
                lyric: previous.map(|note| note.lyric.clone()).unwrap_or_default(),
                phoneme: previous
                    .map(|note| note.phoneme.clone())
                    .unwrap_or_default(),
                pitch_curve_cents: previous
                    .map(|note| note.pitch_curve_cents.clone())
                    .unwrap_or_default(),
                vibrato_depth_cents: previous
                    .map(|note| note.vibrato_depth_cents)
                    .unwrap_or_default(),
                portamento_samples: previous
                    .map(|note| note.portamento_samples)
                    .unwrap_or_default(),
                probability: previous.map(|note| note.probability).unwrap_or(100),
                repeat_count: previous.map(|note| note.repeat_count).unwrap_or(1),
            });
        }
        *metadata = synced;
    }

    pub fn clear_midi_note_metadata(&self) {
        if let Ok(mut notes) = self.scheduled_midi_notes.lock() {
            notes.clear();
        }
        if let Ok(mut rates) = self.midi_vibrato_rates.lock() {
            rates.clear();
        }
    }

    /// Canonical authoring view for piano-roll and vocal editors. Unlike the
    /// realtime packed snapshot, this preserves lyric metadata and is safe
    /// for UI/JSON consumers to inspect.
    pub fn midi_notes_json(&self) -> String {
        let mut notes = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        notes.sort_by_key(|note| (note.track_id, note.start_sample, note.pitch));
        let vibrato_rates = self
            .midi_vibrato_rates
            .lock()
            .map(|rates| rates.clone())
            .unwrap_or_default();
        let enriched = notes
            .into_iter()
            .map(|note| {
                let vibrato_rate = vibrato_rates
                    .get(&(note.track_id, note.pitch, note.start_sample))
                    .copied()
                    .unwrap_or(5_000);
                let mut value =
                    serde_json::to_value(&note).unwrap_or_else(|_| serde_json::json!({}));
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "vibrato_rate_millihz".to_owned(),
                        serde_json::Value::from(vibrato_rate),
                    );
                    object.insert(
                        "drum_lane".to_owned(),
                        serde_json::Value::String(
                            crate::piano_roll_editor::drum_lane_label(note.pitch).to_owned(),
                        ),
                    );
                }
                value
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&enriched).unwrap_or_else(|_| "[]".to_owned())
    }
    pub fn set_spatial_position(&self, tid: u32, x: f32, y: f32, z: f32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_spatial_position(tid, x, y, z))
    }

    /// Installs a bounded measured HRTF impulse-response pair on one track.
    /// The copy happens on the control plane; the realtime panner only reads
    /// its fixed-size kernel. Empty/mismatched/non-finite data is rejected by
    /// the native kernel contract.
    pub fn set_hrtf_kernel(&self, tid: u32, left: Vec<f32>, right: Vec<f32>) -> bool {
        if left.is_empty() || left.len() != right.len() || left.len() > 128
            || left.iter().chain(right.iter()).any(|sample| !sample.is_finite())
        {
            return false;
        }
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_hrtf_kernel(tid, left, right))
    }

    pub fn clear_hrtf_kernel(&self, tid: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.clear_hrtf_kernel(tid))
    }

    /// Loads one measured HRTF pair from a provider payload. The payload is
    /// intentionally simple (`{"left":[...],"right":[...]}`) so an app can
    /// adapt SOFA/database lookups without coupling the core to a file format.
    pub fn set_hrtf_kernel_json(&self, tid: u32, payload: &str) -> String {
        let value = match serde_json::from_str::<serde_json::Value>(payload) {
            Ok(value) => value,
            Err(_) => return r#"{"ok":false,"code":"invalid_hrtf_payload","retryable":false}"#.into(),
        };
        let to_samples = |name: &str| -> Option<Vec<f32>> {
            value.get(name)?.as_array()?.iter().map(|sample| {
                let value = sample.as_f64()? as f32;
                value.is_finite().then_some(value)
            }).collect()
        };
        let Some(left) = to_samples("left") else {
            return r#"{"ok":false,"code":"invalid_hrtf_left","retryable":false}"#.into();
        };
        let Some(right) = to_samples("right") else {
            return r#"{"ok":false,"code":"invalid_hrtf_right","retryable":false}"#.into();
        };
        if self.set_hrtf_kernel(tid, left.clone(), right.clone()) {
            serde_json::json!({"ok":true,"track":tid,"taps":left.len()}).to_string()
        } else {
            r#"{"ok":false,"code":"hrtf_kernel_rejected","retryable":false}"#.into()
        }
    }
    pub fn set_track_armed(&self, tid: u32, armed: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_track_armed(tid, armed))
    }

    pub fn set_track_input_monitor(&self, tid: u32, enabled: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.set_track_input_monitor(tid, enabled))
    }

    pub fn set_mute(&self, tid: u32, m: bool) {
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.set_track_mute(tid, m);
        }
    }
    pub fn set_mute_diagnostic_json(&self, tid: u32, m: bool) -> String {
        self.set_track_toggle_diagnostic_json("mute", tid, |engine, id| {
            engine.set_track_mute(id, m)
        })
    }
    pub fn set_solo(&self, tid: u32, s: bool) {
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.set_track_solo(tid, s);
        }
    }
    pub fn set_solo_diagnostic_json(&self, tid: u32, s: bool) -> String {
        self.set_track_toggle_diagnostic_json("solo", tid, |engine, id| {
            engine.set_track_solo(id, s)
        })
    }

    fn set_track_toggle_diagnostic_json<F>(&self, field: &str, tid: u32, apply: F) -> String
    where
        F: FnOnce(&crate::ffi::AudioEngine, u32) -> bool,
    {
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if apply(engine, tid) {
            return format!("{{\"ok\":true,\"field\":\"{field}\",\"track_id\":{tid}}}");
        } else {
            crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                format!("track {tid} rejected {field} update"),
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result).unwrap_or_else(|_| {
            "{\"code\":\"diagnostic_serialization_failed\",\"retryable\":false}".to_owned()
        })
    }

    pub fn set_phase_invert(&self, tid: u32, inverted: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_phase_invert(tid, inverted))
    }

    pub fn execute_vocal_remover(&self, tid: u32) {
        if let Some(e) = self.engine.as_ref() {
            let _ = e.execute_vocal_remover(tid);
        }
    }

    pub fn execute_vocal_remover_diagnostic_json(&self, tid: u32) -> String {
        if tid == 0 {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_track_id",
                    "track id must be non-zero",
                )
                .object(format!("track:{tid}")),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.execute_vocal_remover(tid) {
            return format!(
                "{{\"ok\":true,\"operation\":\"execute_vocal_remover\",\"track_id\":{tid}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "vocal remover requires an existing stereo track",
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_articulation_map(&self, tid: u32, map: String) {
        // The native engine stores the map identity, not its display name.
        let mut hash = 2166136261u32;
        for byte in map.as_bytes() {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(16777619);
        }
        if let Some(e) = self.engine.as_ref() {
            let _ = e.set_articulation_map(tid, hash);
        }
    }

    pub fn set_articulation_map_diagnostic_json(&self, tid: u32, map: &str) -> String {
        if tid == 0 || map.trim().is_empty() || map.contains('\0') {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_articulation_map",
                    "track id or articulation map is invalid",
                )
                .object(format!("track:{tid}")),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let mut hash = 2166136261u32;
        for byte in map.as_bytes() {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(16777619);
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.set_articulation_map(tid, hash) {
            return format!(
                "{{\"ok\":true,\"operation\":\"set_articulation_map\",\"track_id\":{tid}}}"
            );
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "track_not_found_or_rejected",
                "articulation map target was rejected",
            )
            .object(format!("track:{tid}"))
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn execute_mixing_advice(&self, _title: String) {
        self.execute_auto_mixing();
    }

    pub fn execute_mixing_advice_diagnostic_json(&self, title: String) -> String {
        let title = title.trim();
        if title.is_empty() {
            let result = crate::bridge_error::BridgeError::new(
                "invalid_mixing_advice_title",
                "mixing advice title must not be empty",
            )
            .at_generation(self.project_generation());
            return serde_json::to_string(&result)
                .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        let result = if engine.execute_auto_mixing() {
            return serde_json::json!({
                "ok": true,
                "operation": "execute_mixing_advice",
                "title": title,
                "generation": self.project_generation(),
            })
            .to_string();
        } else {
            crate::bridge_error::BridgeError::new(
                "project_empty_or_rejected",
                "mixing advice requires at least one track",
            )
            .at_generation(self.project_generation())
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    pub fn set_track_eq(&self, tid: u32, lb: f32, lc: f32, hb: f32, hc: f32) {
        if let Some(e) = self.engine.as_ref() {
            e.set_track_eq(tid, lb, lc, hb, hc);
        }
    }

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

    pub fn set_project_scale_diagnostic_json(&self, root: i32, scale_type: i32) -> String {
        if !(0..=11).contains(&root) || !(0..=32).contains(&scale_type) {
            return serde_json::to_string(
                &crate::bridge_error::BridgeError::new(
                    "invalid_project_scale",
                    "root or scale type is outside the supported range",
                )
                .at_generation(self.project_generation()),
            )
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned());
        }
        let Some(engine) = self.engine.as_ref() else {
            return "{\"code\":\"engine_unavailable\",\"retryable\":true}".to_owned();
        };
        if engine.set_project_scale(root, scale_type) {
            return format!("{{\"ok\":true,\"operation\":\"set_project_scale\",\"root\":{root},\"scale_type\":{scale_type}}}");
        }
        serde_json::to_string(
            &crate::bridge_error::BridgeError::new(
                "project_scale_rejected",
                "project scale was rejected by the native engine",
            )
            .at_generation(self.project_generation()),
        )
        .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }

    // Telemetry
    pub fn get_all_peaks_l(&self, out: &mut Vec<f32>) {
        if let Some(e) = self.engine.as_ref() {
            let core = ffi::get_unified_engine(e);
            let pks = ffi::get_track_peaks_l_owned(core);
            out.clear();
            out.extend(pks);
        }
    }

    pub fn get_all_peaks_r(&self, out: &mut Vec<f32>) {
        if let Some(e) = self.engine.as_ref() {
            let core = ffi::get_unified_engine(e);
            let pks = ffi::get_track_peaks_r_owned(core);
            out.clear();
            out.extend(pks);
        }
    }

    pub fn get_cpu_total(&self) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |e| e.get_cpu_total_v() * 100.0)
    }
    pub fn get_buffer_size(&self) -> u32 {
        self.engine.as_ref().map_or(0, |e| e.get_block_size())
    }
    pub fn get_latency_ms(&self) -> f32 {
        self.engine.as_ref().map_or(0.0, |e| e.get_latency_ms())
    }
    pub fn get_track_latency_ms(&self, track_id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |engine| engine.get_track_latency_ms(track_id))
    }

    pub fn get_track_pdc_compensation_ms(&self, track_id: u32) -> f32 {
        self.engine
            .as_ref()
            .map_or(0.0, |engine| engine.get_track_pdc_compensation_ms(track_id))
    }

    pub fn set_low_latency_mode(&self, active: bool) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_low_latency_mode(active))
    }

    pub fn low_latency_mode(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.low_latency_mode())
    }

    pub fn set_tonal_scale(&self, root: i32, scale_type: u32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.set_tonal_scale(root, scale_type))
    }

    pub fn is_note_in_tonal_scale(&self, midi_note: i32) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.is_note_in_tonal_scale(midi_note))
    }

    pub fn generate_chord_notes(&self, root: i32, octave: i32, quality: u32) -> Vec<u8> {
        let quality = match quality {
            0 => crate::tonal::ChordQuality::Major,
            1 => crate::tonal::ChordQuality::Minor,
            2 => crate::tonal::ChordQuality::Diminished,
            3 => crate::tonal::ChordQuality::Dominant7,
            4 => crate::tonal::ChordQuality::Major7,
            5 => crate::tonal::ChordQuality::Minor7,
            _ => return Vec::new(),
        };
        crate::tonal::chord_notes(root, octave, quality)
    }

    pub fn suggest_next_chords(&self, last_chord_name: &str) -> Vec<String> {
        crate::harmonic::HarmonicOrchestrator::default().suggest_next_chords(last_chord_name)
    }

    pub fn generate_arpeggio(
        &self,
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    ) -> Vec<(u8, u8)> {
        if pitches.is_empty() || pitches.len() != velocities.len() || steps == 0 {
            return Vec::new();
        }
        let pattern = match pattern {
            0 => crate::arpeggio::ArpPattern::Up,
            1 => crate::arpeggio::ArpPattern::Down,
            2 => crate::arpeggio::ArpPattern::UpDown,
            3 => crate::arpeggio::ArpPattern::Random,
            _ => return Vec::new(),
        };
        let mut arp = crate::arpeggio::ArpeggioOrchestrator::new();
        arp.pattern = pattern;
        arp.octaves = octaves.clamp(1, 4);
        arp.update_notes(
            pitches
                .into_iter()
                .zip(velocities)
                .map(|(pitch, velocity)| crate::arpeggio::HeldNote { pitch, velocity })
                .collect(),
        );
        (0..steps).filter_map(|_| arp.get_next_note()).collect()
    }

    pub fn place_arpeggio(
        &self,
        track_id: u32,
        start_sample: u64,
        step_samples: u64,
        gate_samples: u64,
        pitches: Vec<u8>,
        velocities: Vec<u8>,
        pattern: u32,
        octaves: u32,
        steps: u32,
    ) -> usize {
        if step_samples == 0 || gate_samples == 0 || gate_samples > step_samples {
            return 0;
        }
        let generated = self.generate_arpeggio(pitches, velocities, pattern, octaves, steps);
        let Some(engine) = self.engine.as_ref() else {
            return 0;
        };
        let mut notes = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        for (index, (pitch, velocity)) in generated.iter().enumerate() {
            let position = start_sample.saturating_add(step_samples.saturating_mul(index as u64));
            let lyric = notes
                .iter()
                .find(|note| {
                    note.track_id == track_id
                        && note.pitch == *pitch
                        && note.start_sample == position
                })
                .map(|note| note.lyric.clone())
                .unwrap_or_default();
            notes.retain(|note| {
                !(note.track_id == track_id
                    && note.pitch == *pitch
                    && note.start_sample == position)
            });
            notes.push(crate::project_contracts::MidiNoteContract {
                track_id,
                pitch: *pitch,
                velocity: *velocity,
                start_sample: position,
                length_samples: gate_samples,
                lyric,
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            });
        }
        let mut packed = Vec::with_capacity(notes.len() * 5);
        for note in &notes {
            packed.extend([
                note.track_id as u64,
                note.pitch as u64,
                note.velocity as u64,
                note.start_sample,
                note.length_samples,
            ]);
        }
        if !engine.replace_midi_notes(packed, true) {
            return 0;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = notes;
        }
        generated.len()
    }

    /// Places a generated chord voicing into the canonical piano-roll model.
    pub fn place_generated_chord(
        &self,
        track_id: u32,
        start_sample: u64,
        length_samples: u64,
        velocity: u8,
        root: i32,
        octave: i32,
        quality: u32,
    ) -> usize {
        if velocity == 0 || length_samples == 0 {
            return 0;
        }
        let pitches = self.generate_chord_notes(root, octave, quality);
        if pitches.is_empty() {
            return 0;
        }
        let Some(engine) = self.engine.as_ref() else {
            return 0;
        };
        let mut notes = self
            .scheduled_midi_notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default();
        for pitch in &pitches {
            let lyric = notes
                .iter()
                .find(|note| {
                    note.track_id == track_id
                        && note.pitch == *pitch
                        && note.start_sample == start_sample
                })
                .map(|note| note.lyric.clone())
                .unwrap_or_default();
            notes.retain(|note| {
                !(note.track_id == track_id
                    && note.pitch == *pitch
                    && note.start_sample == start_sample)
            });
            notes.push(crate::project_contracts::MidiNoteContract {
                track_id,
                pitch: *pitch,
                velocity,
                start_sample,
                length_samples,
                lyric,
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
                probability: 100,
                repeat_count: 1,
            });
        }
        let mut packed = Vec::with_capacity(notes.len() * 5);
        for note in &notes {
            packed.extend([
                note.track_id as u64,
                note.pitch as u64,
                note.velocity as u64,
                note.start_sample,
                note.length_samples,
            ]);
        }
        if !engine.replace_midi_notes(packed, true) {
            return 0;
        }
        if let Ok(mut current) = self.scheduled_midi_notes.lock() {
            *current = notes;
        }
        pitches.len()
    }

    pub fn drum_lane_label(&self, pitch: u8) -> String {
        crate::piano_roll_editor::drum_lane_label(pitch).to_owned()
    }
    pub fn get_fft_bands(&self) -> Vec<f32> {
        self.get_spectral_data_v()
    }
    pub fn get_synesthesia_colors(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_synesthesia_colors_v()
    }
    pub fn get_motion_energy(&self) -> f32 {
        let Some(a) = self.analysis() else {
            return 0.0;
        };
        a.get_motion_energy()
    }
    pub fn get_spectral_partials_v(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_spectral_partials_v()
    }
    pub fn get_video_frame(&self) -> Vec<u8> {
        self.engine
            .as_ref()
            .map_or_else(Vec::new, |e| e.get_video_frame().into_iter().collect())
    }

    pub fn get_video_frame_revision(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |e| e.get_video_frame_revision())
    }

    pub fn request_video_frame(&self, seconds: f64) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.request_video_frame(seconds))
    }

    pub fn load_video(&self, path: &str) -> bool {
        self.engine.as_ref().is_some_and(|e| e.load_video(path))
    }

    pub fn get_song_structure_json(&self) -> String {
        let Some(a) = self.analysis() else {
            return String::new();
        };
        ffi::get_song_structure_json_ffi(a)
    }

    pub fn get_ai_advice(&self) -> Vec<String> {
        let mut results = Vec::new();
        let Some(a) = self.analysis() else {
            return results;
        };
        results.push(a.get_creative_advice());
        results.push(a.get_arrangement_advice(0));
        results
    }

    // --- COMPATIBILITY PASS-THROUGHS FOR UI ---
    pub fn get_mixer_levels_v(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_mixer_levels_v()
    }
    pub fn get_spectral_data_v(&self) -> Vec<f32> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        a.get_spectral_data_v()
    }
    pub fn get_master_loudness(&self) -> LoudnessData {
        let Some(a) = self.analysis() else {
            return LoudnessData::default();
        };
        let v = ffi::get_master_loudness_v_ffi(a);
        LoudnessData {
            integrated: v.integrated,
            short_term: v.short_term,
            true_peak_l: v.true_peak_l,
            true_peak_r: v.true_peak_r,
            correlation: v.correlation,
        }
    }
    pub fn get_intelligence_dashboard_json(&self) -> String {
        let Some(a) = self.analysis() else {
            return String::new();
        };
        a.get_intelligence_dashboard_json()
    }
    pub fn get_clashing_frequencies(&self) -> Vec<ClashData> {
        let Some(a) = self.analysis() else {
            return Vec::new();
        };
        ffi::get_spectral_clash_v_ffi(a)
            .into_iter()
            .map(|c| ClashData {
                frequency: c.frequency,
                severity: c.severity,
            })
            .collect()
    }
    pub fn save_project(&self, path: &str) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        if path.trim().is_empty() {
            return false;
        }
        // Native mutations use their own engine lock and may arrive while
        // sidecars are being serialized. Reject a stale publication instead
        // of letting an older snapshot overwrite a newer edit.
        let save_generation = self.project_generation();
        let had_primary = std::path::Path::new(path).is_file();
        let sidecars = [
            Self::comping_sidecar_path(path),
            Self::midi_sidecar_path(path),
        ];
        let mut sidecar_backups = Vec::with_capacity(sidecars.len());
        for sidecar in &sidecars {
            let backup = std::path::PathBuf::from(format!("{}.bak.1", sidecar.display()));
            let existed = sidecar.is_file();
            if existed && !copy_file_atomic_replace(sidecar, &backup) {
                return false;
            }
            sidecar_backups.push((backup, existed));
        }
        // The native serializer predates the Rust persistence layer. Preserve
        // the current primary before delegating to it so the regular Save
        // button gets the same recovery guarantee as save_project_v2.
        if crate::persistence::PersistenceOrchestrator::new(10)
            .rotate_existing_backup(path)
            .is_err()
        {
            return false;
        }
        let saved = self.engine.as_ref().is_some_and(|e| e.save_project(path))
            && self.save_comping_sidecar(path)
            && self.save_midi_sidecar(path);
        let verified = saved
            && self.project_generation() == save_generation
            && std::fs::metadata(path)
                .map(|meta| meta.is_file() && meta.len() > 0)
                .unwrap_or(false)
            && std::fs::read_to_string(Self::comping_sidecar_path(path))
                .ok()
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
                .is_some()
            && std::fs::read_to_string(Self::midi_sidecar_path(path))
                .ok()
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
                .is_some();
        if verified {
            return true;
        }

        // A native writer or either sidecar can fail after the primary has
        // already been replaced. Restore the previous generation so a failed
        // save is observationally atomic for both disk and the live engine.
        if had_primary {
            let backup = format!("{path}.bak.1");
            if copy_file_atomic_replace(std::path::Path::new(&backup), std::path::Path::new(path)) {
                let _ = self
                    .engine
                    .as_ref()
                    .is_some_and(|e| e.load_project(&backup));
            }
        } else {
            let _ = std::fs::remove_file(path);
        }
        for (backup, existed) in sidecar_backups {
            let sidecar = backup
                .to_string_lossy()
                .strip_suffix(".bak.1")
                .map(std::path::PathBuf::from);
            if let Some(sidecar) = sidecar {
                if existed {
                    let _ = copy_file_atomic_replace(&backup, &sidecar);
                } else {
                    let _ = std::fs::remove_file(sidecar);
                }
            }
        }
        false
    }

    /// Split an audio region at both sides of each sufficiently long silent run.
    /// Boundaries are applied from right to left so the original region remains a
    /// stable left-hand target while native IDs are allocated for the right side.
    pub fn split_region_at_silence(
        &self,
        tid: u32,
        rid: u32,
        samples: &[f32],
        threshold: f32,
        min_length: usize,
    ) -> u32 {
        if tid == 0
            || rid == 0
            || !threshold.is_finite()
            || !(0.0..=1.0).contains(&threshold)
            || min_length == 0
        {
            return 0;
        }
        let Ok(layout) = serde_json::from_str::<serde_json::Value>(&self.get_project_layout_json())
        else {
            return 0;
        };
        let Some(region) = layout.as_array().and_then(|tracks| {
            tracks.iter().find_map(|track| {
                track
                    .get("regions")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|regions| {
                        regions.iter().find(|region| {
                            region.get("id").and_then(serde_json::Value::as_u64) == Some(rid as u64)
                        })
                    })
            })
        }) else {
            return 0;
        };
        let region_start = region
            .get("start")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let region_length = region
            .get("length")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(samples.len() as u64);
        let mut boundaries =
            crate::silence_detector::detect_silence(samples, threshold, min_length)
                .into_iter()
                .flat_map(|range| {
                    let start = range.start as u64;
                    let end = start.saturating_add(range.length as u64);
                    [start, end]
                })
                .filter(|offset| *offset > 0 && *offset < region_length)
                .map(|offset| region_start.saturating_add(offset))
                .collect::<Vec<_>>();
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut applied = 0;
        for sample in boundaries.into_iter().rev() {
            if self.split_region(tid, rid, self.samples_to_beats(sample)) {
                applied += 1;
            }
        }
        applied
    }

    /// Splits a region and applies a symmetric non-destructive crossfade at
    /// the new boundary by resolving the native-created right region ID.
    pub fn split_region_with_auto_crossfade(
        &self,
        tid: u32,
        rid: u32,
        beat: f64,
        ratio: f32,
    ) -> bool {
        if tid == 0
            || rid == 0
            || !beat.is_finite()
            || beat <= 0.0
            || !ratio.is_finite()
            || !(0.0..=1.0).contains(&ratio)
        {
            return false;
        }
        let split_samples = self.beats_to_samples(beat);
        if !self.split_region(tid, rid, beat) {
            return false;
        }
        let Ok(layout) = serde_json::from_str::<serde_json::Value>(&self.get_project_layout_json())
        else {
            return false;
        };
        let Some(track) = layout.as_array().and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(tid as u64)
            })
        }) else {
            return false;
        };
        let Some(regions) = track.get("regions").and_then(serde_json::Value::as_array) else {
            return false;
        };
        let right_id = regions
            .iter()
            .find(|region| {
                region.get("start").and_then(serde_json::Value::as_u64) == Some(split_samples)
            })
            .and_then(|region| region.get("id").and_then(serde_json::Value::as_u64))
            .unwrap_or(0) as u32;
        right_id != 0
            && self.set_region_fades(tid, right_id, ratio, 0.0)
            && self.set_region_fades(tid, rid, 0.0, ratio)
    }
    /// Structured counterpart to the legacy bool API. This keeps existing UI
    /// callers compatible while giving CLI/automation callers a stable error
    /// code instead of an information-losing false value.
    pub fn save_project_diagnostic_json(&self, path: &str) -> String {
        let result = if path.trim().is_empty() {
            Err(crate::bridge_error::BridgeError::new(
                "invalid_path",
                "project path must not be empty",
            ))
        } else if self.save_project(path) {
            Ok(serde_json::json!({
                "ok": true,
                "path": path,
                "generation": self.project_generation(),
            }))
        } else {
            Err(crate::bridge_error::BridgeError::new(
                "project_save_failed",
                "project or sidecar publication failed",
            )
            .retryable(true)
            .at_generation(self.project_generation()))
        };
        match result {
            Ok(value) => value.to_string(),
            Err(error) => serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"serialization_error\"}".to_owned()),
        }
    }
    pub fn load_project(&self, path: &str) -> bool {
        let Ok(_project_transaction) = self.project_transaction.lock() else {
            return false;
        };
        if path.trim().is_empty()
            || !std::fs::metadata(path)
                .map(|meta| meta.is_file() && meta.len() > 0)
                .unwrap_or(false)
        {
            return false;
        }
        // Validate optional sidecars before mutating the native graph. Missing
        // sidecars remain backward-compatible and mean an empty collection;
        // malformed sidecars must never leave the project half-loaded.
        for sidecar in [
            Self::comping_sidecar_path(path),
            Self::midi_sidecar_path(path),
        ] {
            if !sidecar.is_file() {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&sidecar) else {
                return false;
            };
            if serde_json::from_str::<serde_json::Value>(&contents).is_err() {
                return false;
            }
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let rollback_path = std::env::temp_dir().join(format!(
            "aura-load-rollback-{}-{}.native",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        let rollback_path_text = rollback_path.to_string_lossy().into_owned();
        if !engine.save_project(&rollback_path_text) {
            return false;
        }
        let previous_comping = self.comping_snapshot_json();
        let previous_midi = self.midi_events_json();
        let loaded = engine.load_project(path)
            && self.load_comping_sidecar(path)
            && self.load_midi_sidecar(path);
        if loaded {
            if let Ok(mut history) = self.midi_lyric_history.lock() {
                history.clear();
            }
            let _ = std::fs::remove_file(&rollback_path);
            return true;
        }

        // A sidecar failure must not leave the native graph or either
        // auxiliary state store half-hydrated. Restore all three snapshots
        // before exposing the failed load to callers.
        let native_restored = engine.load_project(&rollback_path_text);
        let comping_restored = self.restore_comping_snapshot_json(&previous_comping);
        let midi_restored = self.set_midi_events_json(&previous_midi);
        let _ = std::fs::remove_file(&rollback_path);
        let _rollback_succeeded = native_restored && comping_restored && midi_restored;
        false
    }

    pub fn load_project_diagnostic_json(&self, path: &str) -> String {
        let result = if path.trim().is_empty() {
            Err(crate::bridge_error::BridgeError::new(
                "invalid_path",
                "project path must not be empty",
            ))
        } else if !std::path::Path::new(path).is_file() {
            Err(crate::bridge_error::BridgeError::new(
                "project_not_found",
                "project file does not exist",
            ))
        } else if self.load_project(path) {
            Ok(serde_json::json!({
                "ok": true,
                "path": path,
                "generation": self.project_generation(),
            }))
        } else {
            Err(crate::bridge_error::BridgeError::new(
                "project_load_failed",
                "project or sidecar hydration failed",
            )
            .retryable(false)
            .at_generation(self.project_generation()))
        };
        match result {
            Ok(value) => value.to_string(),
            Err(error) => serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"serialization_error\"}".to_owned()),
        }
    }

    /// Returns valid on-disk recovery generations as JSON so the UI can show
    /// recoverable snapshots without duplicating filesystem scanning logic.
    pub fn recovery_candidates_json(&self, path: &str) -> String {
        if path.trim().is_empty() {
            return "[]".to_owned();
        }
        crate::persistence::PersistenceOrchestrator::recovery_candidates(path)
            .ok()
            .and_then(|candidates| serde_json::to_string(&candidates).ok())
            .unwrap_or_else(|| "[]".to_owned())
    }

    /// Loads one validated backup generation into the native engine. The
    /// generation is numeric, so callers cannot make this API escape the
    /// project's own `.bak.N` recovery set.
    pub fn restore_project_backup(&self, path: &str, generation: u32) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        let Ok(candidates) = crate::persistence::PersistenceOrchestrator::recovery_candidates(path)
        else {
            return false;
        };
        let Some(candidate) = candidates
            .into_iter()
            .find(|candidate| candidate.generation == generation)
        else {
            return false;
        };
        let current_path = std::path::Path::new(path);
        let rollback_path = std::path::PathBuf::from(format!("{path}.before-restore"));
        let rollback_ready =
            current_path.is_file() && std::fs::copy(current_path, &rollback_path).is_ok();
        let candidate_valid = std::fs::read(&candidate.path)
            .ok()
            .is_some_and(|bytes| bytes.len() >= 8 && bytes.starts_with(b"ARUA"));
        if !candidate_valid {
            return false;
        }
        let restored = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.load_project(candidate.path.to_string_lossy().as_ref()));
        if restored {
            return true;
        }
        if rollback_ready {
            let _ = self.engine.as_ref().is_some_and(|engine| {
                engine.load_project(rollback_path.to_string_lossy().as_ref())
            });
        }
        false
    }
}
