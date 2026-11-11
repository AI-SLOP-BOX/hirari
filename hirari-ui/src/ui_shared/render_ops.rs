//! Offline-render helper surface shared by the Slint dock and the AmeUI
//! export card. Core owns the queue and the naming; this only translates
//! documents between core and each surface.

use std::path::PathBuf;

use hirari_core_bridge::HirariCore;

/// A stems export is owned by core (`stem_batch_*`), which does the track
/// selection, the naming, the destination check, the per-stem WAV validation
/// and the rollback of a partly written batch. Each surface only renders the
/// stem core arms and reports the result back, so these variants are the
/// whole of what a surface has to understand.
#[derive(Debug)]
pub(crate) enum StemStep {
    /// A stem is armed: render `output` into `format`.
    Ready {
        output: PathBuf,
        // total/format are only consumed by the Slint render telemetry;
        // keep them for a single source of truth for both surfaces.
        #[allow(dead_code)]
        total: u32,
        format: u32,
    },
    /// One stem published; `stem_batch_next` now reports the following target.
    StemDone { rendered: u32, total: u32 },
    /// The whole batch published.
    Complete {
        outputs: Vec<String>,
        #[allow(dead_code)]
        format: u32,
    },
    /// Core rejected the batch or rolled it back; this is its reason.
    Failed(String),
}

/// Decodes one `stem_batch_*` document. Core owns every code; this only maps
/// a document onto what a surface has to do next.
pub(crate) fn stem_step(document: &str) -> StemStep {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(document) else {
        return StemStep::Failed("engine returned an unreadable stem export status".to_owned());
    };
    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return StemStep::Failed(stem_failure_reason(&value));
    }
    let number = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default() as u32
    };
    match value.get("state").and_then(serde_json::Value::as_str) {
        Some("ready") => StemStep::Ready {
            output: value
                .get("output")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default(),
            total: number("total"),
            format: number("format"),
        },
        Some("stem_done") => StemStep::StemDone {
            rendered: number("index"),
            total: number("total"),
        },
        Some("complete") => StemStep::Complete {
            outputs: value
                .get("outputs")
                .and_then(serde_json::Value::as_array)
                .map(|outputs| {
                    outputs
                        .iter()
                        .filter_map(|output| output.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            format: number("format"),
        },
        _ => StemStep::Failed("stem export reported an unexpected state".to_owned()),
    }
}

/// Turns core's bare error document into the sentence the action log shows.
/// The codes stay core's; this is only the translation for a human.
pub(crate) fn stem_failure_reason(value: &serde_json::Value) -> String {
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let message = value.get("message").and_then(serde_json::Value::as_str);
    match (code, message) {
        ("stem_render_failed", Some(message)) => message.to_owned(),
        ("stem_output_exists", _) => value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("stem output already exists: {path}"))
            .unwrap_or_else(|| "stem output already exists".to_owned()),
        ("invalid_stem_track_selection", _) => "invalid stem track selection".to_owned(),
        ("stem_track_not_found", _) => "one of the selected tracks no longer exists".to_owned(),
        ("no_tracks_for_stem_export", _) => "there are no tracks to export".to_owned(),
        ("invalid_stem_output_directory", _) => "stem output folder is unavailable".to_owned(),
        ("unsupported_render_format", _) => "unsupported render format".to_owned(),
        ("engine_unavailable", _) => "engine is unavailable".to_owned(),
        ("project_transaction_busy", _) => "project is busy".to_owned(),
        (code, _) => format!("stem export could not be prepared ({code})"),
    }
}

/// `stem_batch_prepare` answers with `{"ok":true,...}` rather than a step, so
/// this is what tells a surface whether core accepted the batch, and produces
/// the sentence to show when it did not.
pub(crate) fn prepare_rejected(document: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(document) {
        Ok(value) if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) => None,
        Ok(value) => Some(stem_failure_reason(&value)),
        Err(_) => Some("engine returned an unreadable stem export status".to_owned()),
    }
}

/// Kicks the engine's own bounce thread for an armed stem. Arming already
/// happened inside `stem_batch_next`, which is the only thing that differs
/// from the CLI: `render stems` blocks on the engine's synchronous bounce
/// instead of letting its timer drive the queue.
pub(crate) fn kick_stem(core: &HirariCore, output: &PathBuf, format: u32) -> bool {
    core.start_render_async_to_format(output.to_string_lossy().as_ref(), format)
}

pub(crate) fn render_format_label(format: u32) -> &'static str {
    match format {
        1 => "WAVE64 Float32",
        2 => "Float32 WAV",
        3 => "PCM24 WAV",
        _ => "PCM16 WAV",
    }
}

pub(crate) fn render_tail_seconds(index: i32) -> f32 {
    [0.0, 1.0, 2.0, 5.0, 10.0, 30.0]
        .get(index.clamp(0, 5) as usize)
        .copied()
        .unwrap_or(2.0)
}

/// Human sentences for a cancel request, used by both surfaces.
pub(crate) fn cancellation_status(accepted: bool) -> (&'static str, &'static str, &'static str) {
    if accepted {
        (
            "CANCEL_REQUESTED",
            "CANCELLATION REQUESTED",
            "RENDER CANCELLATION REQUESTED",
        )
    } else {
        (
            "CANCEL_REJECTED",
            "CANCELLATION REJECTED · RENDER CONTINUES",
            "RENDER CANCELLATION REJECTED",
        )
    }
}
