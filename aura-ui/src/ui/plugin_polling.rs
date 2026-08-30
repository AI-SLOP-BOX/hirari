use aura_core_bridge::AuraCore;
use slint::Model;

use crate::slint_ui::Z_Track;

pub struct PluginParameterSnapshot {
    pub names: Vec<String>,
    pub values: Vec<f32>,
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
        };
    }
    let Some(track) = tracks.row_data(selected_row) else {
        return PluginParameterSnapshot {
            names: Vec::new(),
            values: Vec::new(),
        };
    };
    let track_id = track.id.max(0) as u32;
    let plugin_index = plugin_index as u32;
    let count = core.get_plugin_parameter_count(track_id, plugin_index);
    let mut names = Vec::with_capacity(count as usize);
    let mut values = Vec::with_capacity(count as usize);
    for parameter in 0..count {
        let name = core.get_plugin_parameter_name(track_id, plugin_index, parameter);
        names.push(if name.is_empty() {
            format!("PARAM {parameter}")
        } else {
            name
        });
        values.push(core.get_plugin_parameter(track_id, plugin_index, parameter));
    }
    PluginParameterSnapshot { names, values }
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
    use super::{display_latency_ms, values_changed};

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
}
