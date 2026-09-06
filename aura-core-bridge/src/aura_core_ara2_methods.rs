impl AuraCore {
    /// Random-access audio block for an ARA adapter. The native engine owns
    /// decoding; this method only bounds the already decoded interleaved data
    /// and returns the requested frame window.
    pub fn ara2_read_region_audio_json(
        &self,
        track_id: u32,
        region_id: u32,
        start_frame: u64,
        frames: u32,
        channels: u32,
    ) -> String {
        if track_id == 0 || region_id == 0 || channels == 0 || channels > 32 {
            return serde_json::json!({"ok": false, "code": "invalid_ara2_audio_request"}).to_string();
        }
        let Some(engine) = self.engine.as_ref() else {
            return serde_json::json!({"ok": false, "code": "engine_unavailable"}).to_string();
        };
        let audio = engine.get_region_audio_interleaved(track_id, region_id);
        let start = match usize::try_from(start_frame).ok().and_then(|value| value.checked_mul(channels as usize)) {
            Some(value) => value,
            None => return serde_json::json!({"ok": false, "code": "ara2_range_overflow"}).to_string(),
        };
        let length = match usize::try_from(frames).ok().and_then(|value| value.checked_mul(channels as usize)) {
            Some(value) => value,
            None => return serde_json::json!({"ok": false, "code": "ara2_range_overflow"}).to_string(),
        };
        let Some(end) = start.checked_add(length) else {
            return serde_json::json!({"ok": false, "code": "ara2_range_overflow"}).to_string();
        };
        if end > audio.len() {
            return serde_json::json!({"ok": false, "code": "ara2_range_out_of_bounds", "available_samples": audio.len()}).to_string();
        }
        serde_json::json!({
            "ok": true,
            "track_id": track_id,
            "region_id": region_id,
            "start_frame": start_frame,
            "frames": frames,
            "channels": channels,
            "samples": &audio[start..end],
        }).to_string()
    }

    pub fn ara2_bind_document_json(
        &self,
        plugin_id: &str,
        region_id: &str,
        sample_rate: f64,
        channels: u32,
        sample_count: u64,
    ) -> String {
        let result = self.ara2_protocol.lock().map_err(|_| "ARA2 state unavailable".to_owned()).and_then(|mut endpoint| {
            endpoint.bind_document(plugin_id, region_id, sample_rate, channels, sample_count)
        });
        match result {
            Ok(()) => serde_json::json!({"ok": true, "operation": "ara2_bind_document"}).to_string(),
            Err(error) => serde_json::json!({"ok": false, "code": "ara2_bind_failed", "message": error}).to_string(),
        }
    }

    pub fn ara2_unbind_document_json(&self) -> String {
        match self.ara2_protocol.lock() {
            Ok(mut endpoint) => {
                endpoint.unbind_document();
                serde_json::json!({"ok": true, "operation": "ara2_unbind_document"}).to_string()
            }
            Err(_) => serde_json::json!({"ok": false, "code": "ara2_state_unavailable"}).to_string(),
        }
    }

    pub fn ara2_request_analysis_json(&self, request_json: &str) -> String {
        let result = serde_json::from_str::<crate::ara2_protocol::Ara2AnalysisRequest>(request_json)
            .map_err(|error| format!("invalid analysis request: {error}"))
            .and_then(|request| self.ara2_protocol.lock().map_err(|_| "ARA2 state unavailable".to_owned()).and_then(|mut endpoint| endpoint.request_analysis(request)));
        match result {
            Ok(()) => serde_json::json!({"ok": true, "operation": "ara2_request_analysis"}).to_string(),
            Err(error) => serde_json::json!({"ok": false, "code": "ara2_analysis_failed", "message": error}).to_string(),
        }
    }

    pub fn ara2_set_analysis_state_json(&self, id: &str, state_json: &str) -> String {
        let result = serde_json::from_str::<crate::ara2_protocol::Ara2AnalysisState>(state_json)
            .map_err(|error| format!("invalid analysis state: {error}"))
            .and_then(|state| self.ara2_protocol.lock().map_err(|_| "ARA2 state unavailable".to_owned()).and_then(|mut endpoint| endpoint.set_analysis_state(id, state)));
        match result {
            Ok(()) => serde_json::json!({"ok": true, "operation": "ara2_set_analysis_state"}).to_string(),
            Err(error) => serde_json::json!({"ok": false, "code": "ara2_analysis_state_failed", "message": error}).to_string(),
        }
    }

    pub fn ara2_set_note_segments_json(&self, segments_json: &str) -> String {
        let result = serde_json::from_str::<Vec<crate::ara2_protocol::Ara2NoteSegment>>(segments_json)
            .map_err(|error| format!("invalid note segments: {error}"))
            .and_then(|segments| self.ara2_protocol.lock().map_err(|_| "ARA2 state unavailable".to_owned()).and_then(|mut endpoint| endpoint.set_note_segments(segments)));
        match result {
            Ok(()) => serde_json::json!({"ok": true, "operation": "ara2_set_note_segments"}).to_string(),
            Err(error) => serde_json::json!({"ok": false, "code": "ara2_note_segments_failed", "message": error}).to_string(),
        }
    }

    pub fn ara2_document_snapshot_json(&self) -> String {
        match self.ara2_protocol.lock() {
            Ok(endpoint) => serde_json::json!({"ok": true, "document": endpoint.snapshot()}).to_string(),
            Err(_) => serde_json::json!({"ok": false, "code": "ara2_state_unavailable"}).to_string(),
        }
    }
}
