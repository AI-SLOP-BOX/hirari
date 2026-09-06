//! Plugin parameter and PDC telemetry.

use crate::slint_ui::*;
use crate::ui::plugin_polling;
use aura_core_bridge::AuraCore;
use slint::Model;

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_plugin_telemetry(
    ui: &AppWindow,
    core: &AuraCore,
    tracks: &slint::VecModel<Z_Track>,
    plugin_elapsed_ms: &mut u32,
    pdc_elapsed_ms: &mut u32,
    last_plugin_context: &mut (i32, i32),
    last_pdc_context: &mut (i32, i32),
    last_plugin_values: &mut Vec<f32>,
) {
    let plugin_index = ui.get_fx_active_id();
    let selected_row = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
    let selected_track = tracks.row_data(selected_row);
    let track_id = selected_track.as_ref().map(|track| track.id).unwrap_or(-1);
    let plugin_context = (track_id, plugin_index);
    if plugin_context != *last_plugin_context || *plugin_elapsed_ms >= 64 {
        let bypassed = if track_id >= 0 && plugin_index >= 0 {
            core.get_plugin_bypass(track_id as u32, plugin_index as u32)
        } else {
            false
        };
        ui.set_plugin_bypassed(bypassed);
    }
    if plugin_context != *last_plugin_context || *plugin_elapsed_ms >= 64 {
        ui.set_plugin_editor_capability(
            plugin_polling::editor_capability_label(core, track_id, plugin_index).into(),
        );
    }
    let pdc_due = *pdc_elapsed_ms >= 256;
    let pdc_context_changed = plugin_context != *last_pdc_context;
    if pdc_due {
        *pdc_elapsed_ms -= 256;
    }
    let pdc_status_pending = ui.get_plugin_pdc_status() == "CALCULATING";
    if pdc_due || pdc_context_changed || pdc_status_pending {
        if selected_track
            .as_ref()
            .is_some_and(|track| track.fx.row_count() == 0)
        {
            ui.set_plugin_pdc_status("NO_PLUGIN".into());
        }
        let (latency, compensation) = selected_track
            .as_ref()
            .map(|track| {
                let id = track.id.max(0) as u32;
                (
                    core.get_track_latency_ms(id),
                    core.get_track_pdc_compensation_ms(id),
                )
            })
            .unwrap_or((0.0, 0.0));
        let status = if selected_track
            .as_ref()
            .is_some_and(|track| track.fx.row_count() == 0)
        {
            "NO_PLUGIN"
        } else if selected_track.is_some()
            && latency.is_finite()
            && latency >= 0.0
            && compensation.is_finite()
            && compensation >= 0.0
        {
            "READY"
        } else if selected_track.is_some() {
            "ERROR"
        } else {
            "UNAVAILABLE"
        };
        ui.set_plugin_pdc_status(status.into());
        ui.set_plugin_track_latency_ms(plugin_polling::display_latency_ms(latency));
        ui.set_plugin_pdc_compensation_ms(plugin_polling::display_latency_ms(compensation));
        *last_pdc_context = plugin_context;
    }
    let context_changed = plugin_context != *last_plugin_context;
    let plugin_poll_due = *plugin_elapsed_ms >= 64;
    let plugin_event_due = !core.drain_plugin_parameter_events().is_empty();
    if plugin_poll_due {
        *plugin_elapsed_ms -= 64;
    }
    let plugin_snapshot = if context_changed || plugin_poll_due || plugin_event_due {
        Some(plugin_polling::snapshot(
            core,
            tracks,
            selected_row,
            plugin_index,
        ))
    } else {
        None
    };
    if context_changed {
        *last_plugin_context = plugin_context;
        last_plugin_values.clear();
        let names = plugin_snapshot
            .as_ref()
            .map(|snapshot| snapshot.names.clone())
            .unwrap_or_default()
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>();
        ui.set_plugin_parameter_names(slint::ModelRc::new(slint::VecModel::from(names)));
    }
    if let Some(snapshot) = plugin_snapshot {
        ui.set_plugin_parameter_error(!snapshot.available);
        let values = snapshot.values;
        let values_changed = plugin_polling::values_changed(&values, last_plugin_values);
        if values_changed {
            *last_plugin_values = values.clone();
            ui.set_plugin_parameter_values(slint::ModelRc::new(slint::VecModel::from(values)));
        }
    }
}
