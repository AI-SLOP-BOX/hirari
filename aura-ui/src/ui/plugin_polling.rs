use aura_core_bridge::AuraCore;
use slint::Model;

use crate::slint_ui::Z_Track;

pub struct PluginParameterSnapshot {
    pub names: Vec<String>,
    pub values: Vec<f32>,
    pub available: bool,
}

/// Converts the Core capability diagnostic into a compact, user-facing label.
/// Native editor embedding is platform-dependent, so the fallback remains
/// explicit instead of pretending the parameter panel is the vendor UI.
pub fn editor_capability_label(core: &AuraCore, track_id: i32, plugin_index: i32) -> String {
    if track_id < 0 || plugin_index < 0 {
        return "NO PLUGIN SELECTED".to_string();
    }
    editor_capability_label_from_json(
        &core.plugin_editor_capability_diagnostic_json(track_id as u32, plugin_index as u32),
    )
}

fn editor_capability_label_from_json(json: &str) -> String {
    let value = serde_json::from_str::<serde_json::Value>(json).unwrap_or_default();
    let native = value
        .get("native_editor")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let embedding = value
        .get("embedding")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("parameter_only");
    let embedded = value
        .get("embedded")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if native && embedded {
        "NATIVE EDITOR · EMBEDDED".to_string()
    } else if native {
        let host_state = value
            .get("host_state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(embedding);
        if host_state == "available_not_embedded" || embedding == "ui_thread_required" {
            "NATIVE EDITOR AVAILABLE · UI HOST PENDING".to_string()
        } else {
            format!("NATIVE EDITOR · {host_state}")
        }
    } else {
        "PARAMETER EDITOR · NATIVE UI UNAVAILABLE".to_string()
    }
}

fn parse_parameter_snapshot(json: &str) -> PluginParameterSnapshot {
    let snapshot = serde_json::from_str::<serde_json::Value>(json).unwrap_or_default();
    let mut available = snapshot.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    let mut names = Vec::new();
    let mut values = Vec::new();
    if let Some(parameters) = snapshot
        .get("parameters")
        .and_then(serde_json::Value::as_array)
    {
        names.reserve(parameters.len());
        values.reserve(parameters.len());
        for (index, parameter) in parameters.iter().enumerate() {
            let name = parameter
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or_default();
            names.push(if name.is_empty() {
                format!("PARAM {index}")
            } else {
                name.to_string()
            });
            let value = parameter
                .get("normalized")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value));
            if value.is_none() {
                available = false;
            }
            values.push(value.unwrap_or(0.0) as f32);
        }
    }
    PluginParameterSnapshot {
        names,
        values,
        available,
    }
}

/// Reads one immutable plugin snapshot from Core for the UI timer.
///
/// Keeping this boundary separate makes the current polling implementation
/// replaceable with engine-pushed parameter events without changing the view
/// or the action bindings.
pub fn snapshot(
    core: &AuraCore,
    tracks: &slint::VecModel<Z_Track>,
    selected_row: usize,
    plugin_index: i32,
) -> PluginParameterSnapshot {
    if plugin_index < 0 {
        return PluginParameterSnapshot {
            names: Vec::new(),
            values: Vec::new(),
            available: false,
        };
    }
    let Some(track) = tracks.row_data(selected_row) else {
        return PluginParameterSnapshot {
            names: Vec::new(),
            values: Vec::new(),
            available: false,
        };
    };
    let track_id = track.id.max(0) as u32;
    let plugin_index = plugin_index as u32;
    parse_parameter_snapshot(&core.plugin_parameter_snapshot_json(track_id, plugin_index))
}

pub fn values_changed(current: &[f32], previous: &[f32]) -> bool {
    current.len() != previous.len()
        || current.iter().zip(previous).any(|(current, previous)| {
            !current.is_finite() || !previous.is_finite() || (current - previous).abs() > 0.0005
        })
}

/// Converts an engine latency value into a safe UI value.  The Core clamps
/// PDC in samples, but the UI boundary also rejects malformed values so a
/// stale or faulty plugin cannot surface a negative/NaN label.
pub fn display_latency_ms(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        display_latency_ms, editor_capability_label_from_json, parse_parameter_snapshot,
        values_changed,
    };

    #[test]
    fn parameter_value_diff_detects_shape_nan_and_threshold_changes() {
        assert!(!values_changed(&[0.1, 0.2], &[0.1, 0.2]));
        assert!(values_changed(&[0.1], &[0.1, 0.2]));
        assert!(values_changed(&[f32::NAN], &[0.1]));
        assert!(values_changed(&[0.2], &[0.1]));
    }

    #[test]
    fn latency_display_rejects_negative_and_non_finite_values() {
        assert_eq!(display_latency_ms(-1.0), 0.0);
        assert_eq!(display_latency_ms(f32::NAN), 0.0);
        assert_eq!(display_latency_ms(f32::INFINITY), 0.0);
        assert_eq!(display_latency_ms(12.5), 12.5);
    }

    #[test]
    fn editor_capability_label_distinguishes_native_and_fallback() {
        assert_eq!(
            editor_capability_label_from_json(
                r#"{"native_editor":true,"embedded":false,"host_state":"available_not_embedded","embedding":"ui_thread_required"}"#
            ),
            "NATIVE EDITOR AVAILABLE · UI HOST PENDING"
        );
        assert_eq!(
            editor_capability_label_from_json(
                r#"{"native_editor":true,"embedded":true,"host_state":"embedded"}"#
            ),
            "NATIVE EDITOR · EMBEDDED"
        );
        assert_eq!(
            editor_capability_label_from_json(r#"{"native_editor":false}"#),
            "PARAMETER EDITOR · NATIVE UI UNAVAILABLE"
        );
    }

    #[test]
    fn malformed_parameter_snapshot_is_unavailable_and_has_no_controls() {
        let snapshot = parse_parameter_snapshot("not-json");
        assert!(!snapshot.available);
        assert!(snapshot.names.is_empty());
        assert!(snapshot.values.is_empty());
    }

    #[test]
    fn parameter_snapshot_preserves_valid_names_and_normalized_values() {
        let snapshot = parse_parameter_snapshot(
            r#"{"ok":true,"parameters":[{"name":"Gain","normalized":0.75},{"name":"","normalized":0.5}]}"#,
        );
        assert!(snapshot.available);
        assert_eq!(snapshot.names, vec!["Gain", "PARAM 1"]);
        assert_eq!(snapshot.values, vec![0.75, 0.5]);
    }
}
