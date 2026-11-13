impl HirariCore {
    /// Render tail bounds live here; the engine only stores milliseconds.
    fn bounded_render_tail_seconds(seconds: f32) -> f32 {
        if seconds.is_finite() {
            seconds.clamp(0.0, 60.0)
        } else {
            0.0
        }
    }

    pub fn set_offline_render_target(&self, track_id: u32) -> bool {
        self.engine_opt()
            .is_some_and(|engine| engine.set_offline_render_target(track_id))
    }

    pub fn clear_offline_render_target(&self) {
        if let Some(engine) = self.engine_opt() {
            engine.clear_offline_render_target();
        }
    }

    pub fn set_offline_render_tail_seconds(&self, seconds: f32) {
        if let Some(engine) = self.engine_opt() {
            engine.set_offline_render_tail_seconds(Self::bounded_render_tail_seconds(seconds));
        }
    }

    pub fn set_offline_render_options(&self, pre_fader: bool, include_inserts: bool) {
        if let Some(engine) = self.engine_opt() {
            engine.set_offline_render_options(pre_fader, include_inserts);
        }
    }

    pub fn plugin_latency_snapshot_json(&self) -> String {
        let Some(engine) = self.engine_opt() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\"}".to_owned();
        };
        let tracks: serde_json::Value = serde_json::from_str(&engine.project_layout_json())
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

}

#[path = "hirari_core_project_plugins_export.rs"]
mod export_;
#[path = "hirari_core_project_plugins_stems.rs"]
mod stems_;


/// Request validation shared by the diagnostic bounce entry point. Returns
/// the rejection document when the render cannot be attempted; the chosen
/// container and the engine failure stay separate codes.
pub(crate) fn bounce_diagnostic_rejection(path: &str, format: u32) -> Option<&'static str> {
    if !is_wav_output_path(path) {
        return Some("{\"code\":\"invalid_render_path\",\"retryable\":false}");
    }
    if format > 3 {
        return Some("{\"code\":\"unsupported_render_format\",\"retryable\":false}");
    }
    None
}

/// `stem_batch_*` reports success as `"ok": true` and every failure as the
/// bare `{"code":…}` document this module has always returned, so the
/// blocking driver can tell them apart without a second error code path.
fn stem_batch_succeeded(document: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(document)
        .map(|value| value.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
        .unwrap_or(false)
}

fn stem_batch_unavailable() -> String {
    "{\"code\":\"stem_batch_unavailable\",\"retryable\":true}".to_owned()
}

fn stem_batch_unreadable() -> String {
    "{\"code\":\"stem_batch_unreadable\",\"retryable\":true}".to_owned()
}

/// Rebuilds the success document `render stems` has always printed from the
/// queue's own outputs, so only the state machine knows what was published.
fn stem_batch_success_document(
    document: &serde_json::Value,
    format: u32,
    tail_seconds: f32,
    pre_fader: bool,
    include_inserts: bool,
) -> String {
    let outputs = document
        .get("outputs")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
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
