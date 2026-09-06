impl AuraCore {
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
