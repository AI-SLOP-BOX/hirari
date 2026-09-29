use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::Cell;
use std::rc::Rc;

use crate::slint_ui::{
    sync_tracks_from_engine, ui_error_message, AppWindow, MixerActions, UiErrorKind, Z_Fx, Z_Track,
};
use crate::ui::sync::replace_track;

/// Channel-strip mutations. All mixer actions update the engine first where
/// required, then publish the accepted value to the Slint model.
pub fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    peak_reset_generation: Rc<Cell<u64>>,
    moufu_publisher: Option<crate::moufu::MoufuPublisher>,
) {
    let weak = ui.as_weak();
    ui.global::<MixerActions>().on_open_send_routing({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_mx_open(false);
                ui.set_bot_view(4);
                ui.set_send_routing_mode(true);
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_fx({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, fx_index| {
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != track_id || fx_index < 0 {
                    continue;
                }
                let mut effects: Vec<Z_Fx> = track.fx.iter().collect();
                let Some(effect) = effects.get_mut(fx_index as usize) else {
                    return;
                };
                let requested_active = !effect.active;
                let name = effect.name.clone();
                if !core.set_plugin_bypass(track_id as u32, fx_index as u32, !requested_active) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Plugin,
                                &format!("FX {} {} could not be bypassed", track_id, name),
                            )
                            .into(),
                        );
                    }
                    return;
                }
                effect.active = requested_active;
                let active = effect.active;
                track.fx = slint::ModelRc::new(VecModel::from(effects));
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "FX {} {}: {}",
                            track_id,
                            name,
                            if active { "ON" } else { "OFF" }
                        )
                        .into(),
                    );
                }
                break;
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_saturation({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id| {
            let Some(row) = (0..tracks.row_count()).find(|row| {
                tracks
                    .row_data(*row)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            let mut effects: Vec<Z_Fx> = track.fx.iter().collect();
            if let Some((plugin_index, effect)) = effects
                .iter_mut()
                .enumerate()
                .find(|(_, effect)| effect.name == "Tube Saturation")
            {
                let requested_active = !effect.active;
                core.begin_undo_transaction("Toggle Tube Saturation");
                let accepted = core.set_plugin_bypass(
                    track_id.max(0) as u32,
                    plugin_index as u32,
                    !requested_active,
                );
                let accepted = if accepted {
                    core.end_undo_transaction()
                } else {
                    let _ = core.abort_undo_transaction();
                    false
                };
                if !accepted {
                    let _ = sync_tracks_from_engine(&tracks, &core);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Plugin,
                                "Tube Saturation could not be updated",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                effect.active = requested_active;
                track.saturate_active = requested_active;
                track.fx = slint::ModelRc::new(VecModel::from(effects));
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if requested_active {
                        format!("TUBE SATURATION ON · TRACK {track_id}").into()
                    } else {
                        format!("TUBE SATURATION BYPASSED · TRACK {track_id}").into()
                    });
                }
                return;
            }

            core.begin_undo_transaction("Insert Tube Saturation");
            let accepted = core.add_plugin(track_id.max(0) as u32, 3);
            let accepted = if accepted {
                core.end_undo_transaction()
            } else {
                let _ = core.abort_undo_transaction();
                false
            };
            if !accepted {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Plugin,
                            "Tube Saturation could not be inserted",
                        )
                        .into(),
                    );
                }
                return;
            }
            sync_tracks_from_engine(&tracks, &core);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(format!("TUBE SATURATION INSERTED · TRACK {track_id}").into());
            }
        }
    });
    ui.global::<MixerActions>().on_reset_peaks({
        let weak = ui.as_weak();
        let peak_reset_generation = peak_reset_generation.clone();
        move || {
            peak_reset_generation.set(peak_reset_generation.get().wrapping_add(1));
            if let Some(ui) = weak.upgrade() {
                let peak_count = ui.get_pks().row_count();
                ui.set_pks(slint::ModelRc::new(VecModel::from(vec![0.0; peak_count])));
                let hold_count = ui.get_pks_h().row_count();
                ui.set_pks_h(slint::ModelRc::new(VecModel::from(vec![0.0; hold_count])));
                ui.set_last_action("PEAK METERS RESET".into());
            }
        }
    });

    ui.global::<MixerActions>().on_fader_changed({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, val| {
            if !val.is_finite() {
                return;
            }
            let bounded = val.clamp(0.0, 2.0);
            if id == 999 {
                if core.set_master_gain(bounded) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_master_output_gain(bounded);
                        ui.set_last_action(format!("MASTER LEVEL {:.0}%", bounded * 100.0).into());
                    }
                }
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.volume = bounded;
                    core.set_track_fader(id as u32, bounded);
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_pan_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, val| {
            if !val.is_finite() {
                return;
            }
            let bounded = val.clamp(-1.0, 1.0);
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    core.set_track_pan(id as u32, bounded);
                    track.pan = bounded;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_monitor({
        let weak = ui.as_weak();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            let Some(target_row) = (0..tracks.row_count())
                .find(|row| tracks.row_data(*row).is_some_and(|track| track.id == id))
            else {
                return;
            };
            let currently_enabled = tracks
                .row_data(target_row)
                .is_some_and(|track| track.monitor);
            let enabled = !currently_enabled;
            let Some(track) = tracks.row_data(target_row) else {
                return;
            };
            let Some(input_channels) =
                crate::ui::track_model::parse_recording_input_channels(track.input.as_str())
            else {
                return;
            };
            let left = input_channels[0] as u32;
            let right = input_channels.get(1).copied().unwrap_or(input_channels[0]) as u32;
            if !core.set_track_input_monitor_channels(id as u32, enabled, left, right) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Engine,
                            &format!("Track {} input monitor could not be changed", id),
                        )
                        .into(),
                    );
                }
                return;
            }
            // The engine has one physical input monitor focus. Reflect that
            // exclusivity in the model when enabling a different track.
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                let next = enabled && track.id == id;
                if track.monitor != next {
                    track.monitor = next;
                    replace_track(&tracks, row, track);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!("MONITOR {}: {}", id, if enabled { "ON" } else { "OFF" }).into(),
                );
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_mute({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.mute = !track.mute;
                    core.set_mute(id as u32, track.mute);
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_solo({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.solo = !track.solo;
                    core.set_solo(id as u32, track.solo);
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_phase({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id != id {
                    continue;
                }
                let inverted = !track.phase_invert;
                if !core.set_phase_invert(id as u32, inverted) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Engine,
                                "phase invert failed: track not found",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                track.phase_invert = inverted;
                replace_track(&tracks, i, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if inverted {
                        "PHASE: INVERTED".into()
                    } else {
                        "PHASE: NORMAL".into()
                    });
                }
                break;
            }
        }
    });
    ui.global::<MixerActions>().on_reset_all_mute_solo({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move || {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.mute || track.solo {
                    track.mute = false;
                    track.solo = false;
                    core.set_mute(track.id as u32, false);
                    core.set_solo(track.id as u32, false);
                    replace_track(&tracks, i, track);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action("RESET MUTE/SOLO".into());
            }
        }
    });
    ui.global::<MixerActions>().on_delay_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, milliseconds| {
            if !milliseconds.is_finite() {
                return;
            }
            let bounded = milliseconds.clamp(0.0, 200.0);
            let sample_rate = core.get_sample_rate();
            if !sample_rate.is_finite() || sample_rate <= 0.0 {
                return;
            }
            let samples = ((bounded as f64 * sample_rate / 1000.0).round() as u32).min(8192);
            if !core.set_track_delay_samples(id as u32, samples) {
                return;
            }
            let accepted_ms = samples as f32 * 1000.0 / sample_rate as f32;
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.delay_ms = accepted_ms;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_step_input_channel({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let moufu_publisher = moufu_publisher.clone();
        move |id, direction| {
            if !matches!(direction, -1 | 1) {
                return;
            }
            let Some(row) = (0..tracks.row_count())
                .find(|row| tracks.row_data(*row).is_some_and(|track| track.id == id))
            else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            if track.r#type != "Audio" && track.r#type != "Vocal" {
                return;
            }
            let Some(current) =
                crate::ui::track_model::parse_recording_input_channels(track.input.as_str())
            else {
                return;
            };
            let input_count = core.get_audio_input_channel_count().min(32);
            let available = if input_count == 0 { 32 } else { input_count };
            let width = current.len() as u16;
            if available < width {
                return;
            }
            let last_start = available - width;
            let next = (current[0] as i32 + direction).clamp(0, last_start as i32) as u16;
            let channels = (next..next + width).collect::<Vec<_>>();
            let Some(value) = crate::ui::track_model::format_recording_input_channels(&channels)
            else {
                return;
            };
            if track.monitor {
                let left = channels[0] as u32;
                let right = channels.get(1).copied().unwrap_or(channels[0]) as u32;
                if !core.set_track_input_monitor_channels(id.max(0) as u32, true, left, right) {
                    return;
                }
            }
            let input_changed = track.input.as_str() != value;
            track.input = value.clone().into();
            track.input_first_channel = next as i32;
            if input_changed {
                crate::ui::project_state::mark_ui_routing_changed();
            }
            let endpoint_uid = if track.input_endpoint_uid.is_empty() {
                core.audio_input_device_uid()
            } else {
                track.input_endpoint_uid.to_string()
            };
            let channel_names = core.audio_device_input_channel_names(&endpoint_uid);
            track.input_bus_name = crate::ui::track_model::recording_input_bus_label_with_names(
                &channels,
                &channel_names,
            )
            .or_else(|| crate::ui::track_model::recording_input_bus_label(&channels))
            .unwrap_or_else(|| "Input".to_owned())
            .into();
            replace_track(&tracks, row, track);
            if let Some(publisher) = &moufu_publisher {
                crate::moufu::publish_layout(publisher, &core, &tracks, true);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(format!("TRACK INPUT: {value}").into());
            }
        }
    });
    ui.global::<MixerActions>().on_select_input_channel({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let moufu_publisher = moufu_publisher.clone();
        move |id, selected_name| {
            let Some(row) = (0..tracks.row_count())
                .find(|row| tracks.row_data(*row).is_some_and(|track| track.id == id))
            else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            if track.r#type != "Audio" && track.r#type != "Vocal" {
                return;
            }
            let endpoint_uid = if track.input_endpoint_uid.is_empty() {
                core.audio_input_device_uid()
            } else {
                track.input_endpoint_uid.to_string()
            };
            let names = core.audio_device_input_channel_names(&endpoint_uid);
            let Some(channel) = names
                .iter()
                .position(|name| name == selected_name.as_str())
                .and_then(|channel| u16::try_from(channel).ok())
            else {
                return;
            };
            let available = core.get_audio_input_channel_count().min(32);
            if available == 0 || channel >= available {
                return;
            }
            let width =
                crate::ui::track_model::parse_recording_input_channels(track.input.as_str())
                    .map_or(2, |channels| channels.len().clamp(1, 32) as u16)
                    .min(available);
            let start = channel.min(available - width);
            let channels = (start..start + width).collect::<Vec<_>>();
            let Some(value) = crate::ui::track_model::format_recording_input_channels(&channels)
            else {
                return;
            };
            if track.monitor {
                let right = channels.get(1).copied().unwrap_or(channels[0]);
                if !core.set_track_input_monitor_channels(
                    id.max(0) as u32,
                    true,
                    channels[0] as u32,
                    right as u32,
                ) {
                    return;
                }
            }
            if track.input.as_str() != value {
                crate::ui::project_state::mark_ui_routing_changed();
            }
            track.input = value.clone().into();
            track.input_first_channel = start as i32;
            track.input_bus_name =
                crate::ui::track_model::recording_input_bus_label_with_names(&channels, &names)
                    .or_else(|| crate::ui::track_model::recording_input_bus_label(&channels))
                    .unwrap_or_else(|| "Input".to_owned())
                    .into();
            replace_track(&tracks, row, track);
            if let Some(publisher) = &moufu_publisher {
                crate::moufu::publish_layout(publisher, &core, &tracks, true);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(format!("TRACK INPUT: {} → {value}", selected_name).into());
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_input_width({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let moufu_publisher = moufu_publisher.clone();
        move |id| {
            let Some(row) = (0..tracks.row_count())
                .find(|row| tracks.row_data(*row).is_some_and(|track| track.id == id))
            else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            if track.r#type != "Audio" && track.r#type != "Vocal" {
                return;
            }
            let Some(current) =
                crate::ui::track_model::parse_recording_input_channels(track.input.as_str())
            else {
                return;
            };
            let input_count = core.get_audio_input_channel_count().min(32);
            let available = if input_count == 0 { 32 } else { input_count };
            let mut widths = [1u16, 2, 4, 8, 16, 32]
                .into_iter()
                .filter(|width| *width <= available)
                .collect::<Vec<_>>();
            if !widths.contains(&available) {
                widths.push(available);
            }
            if widths.is_empty() {
                return;
            }
            let current_width = current.len() as u16;
            let next_width = widths
                .iter()
                .position(|width| *width == current_width)
                .map(|index| widths[(index + 1) % widths.len()])
                .unwrap_or(widths[0]);
            let Some(last_start) = available.checked_sub(next_width) else {
                return;
            };
            let start = current[0].min(last_start);
            let channels = (start..start + next_width).collect::<Vec<_>>();
            let Some(value) = crate::ui::track_model::format_recording_input_channels(&channels)
            else {
                return;
            };
            if track.monitor {
                let left = channels[0] as u32;
                let right = channels.get(1).copied().unwrap_or(channels[0]) as u32;
                if !core.set_track_input_monitor_channels(id.max(0) as u32, true, left, right) {
                    return;
                }
            }
            let input_changed = track.input.as_str() != value;
            track.input = value.clone().into();
            track.input_first_channel = start as i32;
            if input_changed {
                crate::ui::project_state::mark_ui_routing_changed();
            }
            let endpoint_uid = if track.input_endpoint_uid.is_empty() {
                core.audio_input_device_uid()
            } else {
                track.input_endpoint_uid.to_string()
            };
            let channel_names = core.audio_device_input_channel_names(&endpoint_uid);
            track.input_bus_name = crate::ui::track_model::recording_input_bus_label_with_names(
                &channels,
                &channel_names,
            )
            .or_else(|| crate::ui::track_model::recording_input_bus_label(&channels))
            .unwrap_or_else(|| "Input".to_owned())
            .into();
            replace_track(&tracks, row, track);
            if let Some(publisher) = &moufu_publisher {
                crate::moufu::publish_layout(publisher, &core, &tracks, true);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!("TRACK INPUT WIDTH {} CH: {value}", channels.len()).into(),
                );
            }
        }
    });
}
