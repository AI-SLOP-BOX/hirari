fn plugin_editor_parent_handle(ui: &AppWindow) -> Option<u64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let window = ui.window();
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    let value = match raw {
        RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr() as usize,
        RawWindowHandle::Win32(handle) => handle.hwnd.get() as usize,
        RawWindowHandle::Xlib(handle) => handle.window as usize,
        RawWindowHandle::Xcb(handle) => handle.window.get() as usize,
        // VST3's Linux embedding contract currently expects X11's
        // X11EmbedWindowID. Passing a Wayland wl_surface as that ID is invalid.
        _ => return None,
    };
    (value != 0).then_some(value as u64)
}

pub fn install(ui: &AppWindow, core: Rc<HirariCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    ui.global::<PluginActions>().on_catalog_query_changed({
        let weak = weak.clone();
        move |query| {
            if let Some(ui) = weak.upgrade() {
                filter_plugin_catalog(&ui, query.as_str());
            }
        }
    });
    ui.global::<PluginActions>().on_refresh_extensions({
        let weak = ui.as_weak();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let count = refresh_extension_catalog(&ui, &core);
                ui.set_bot_view(16);
                ui.set_mx_open(true);
                ui.set_last_action(format!("EXTENSIONS: {} COMMANDS REGISTERED", count).into());
            }
        }
    });
    ui.global::<PluginActions>().on_install_extension({
        let weak = ui.as_weak();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                install_extension_from_ui(&core, &ui);
            }
        }
    });
    ui.global::<PluginActions>().on_run_extension({
        let weak = ui.as_weak();
        let core = core.clone();
        move |extension_id, command_id, payload| {
            if let Some(ui) = weak.upgrade() {
                run_extension_from_ui(
                    extension_id.as_str(),
                    command_id.as_str(),
                    payload.as_str(),
                    &core,
                    &ui,
                );
            }
        }
    });
    ui.global::<PluginActions>().on_toggle_extension({
        let weak = ui.as_weak();
        let core = core.clone();
        move |extension_id, enabled| {
            if let Some(ui) = weak.upgrade() {
                toggle_extension_from_ui(extension_id.as_str(), enabled, &core, &ui);
            }
        }
    });
    ui.global::<PluginActions>().on_plugin_param_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, parameter_id, value| {
            let applied =
                apply_plugin_parameter(&core, track_id, plugin_index, parameter_id, value);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!(
                        "PLUGIN PARAM: TRACK {} FX {} P{}",
                        track_id, plugin_index, parameter_id
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "parameter rejected by host").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_automate_plugin_parameter({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, plugin_index, parameter_id| {
            if track_id < 0 || plugin_index < 0 || parameter_id < 0 {
                return;
            }
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
            let mut lanes: Vec<Z_AutomationLane> = track.auto_lanes.iter().collect();
            if lanes
                .iter()
                .any(|lane| lane.plugin_index == plugin_index && lane.parameter_id == parameter_id)
            {
                track.show_automation = true;
                tracks.set_row_data(row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("PLUGIN AUTOMATION ALREADY EXISTS".into());
                }
                return;
            }

            let sample = core.get_playhead();
            let beat = core.samples_to_beats(sample) as f32;
            let value = core
                .get_plugin_parameter(track_id as u32, plugin_index as u32, parameter_id as u32)
                .clamp(0.0, 1.0);
            let points = vec![sample as f64, value as f64, 0.0];
            core.begin_undo_transaction("Create Plugin Automation");
            let accepted = core.set_plugin_automation(
                track_id as u32,
                plugin_index as u32,
                parameter_id as u32,
                points,
            );
            let accepted = if accepted {
                core.end_undo_transaction()
            } else {
                let _ = core.abort_undo_transaction();
                false
            };
            if !accepted {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Plugin, "automation lane creation failed")
                            .into(),
                    );
                }
                return;
            }

            let parameter_name = core.get_plugin_parameter_name(
                track_id as u32,
                plugin_index as u32,
                parameter_id as u32,
            );
            let name = if parameter_name.trim().is_empty() {
                format!("Plugin {} Parameter {}", plugin_index + 1, parameter_id)
            } else {
                format!("Plugin {} · {}", plugin_index + 1, parameter_name)
            };
            let hue = (plugin_index
                .wrapping_mul(67)
                .wrapping_add(parameter_id.wrapping_mul(29))
                % 156) as u8;
            lanes.push(Z_AutomationLane {
                name: name.into(),
                color: slint::Color::from_rgb_u8(80 + hue, 180, 120 + hue / 2),
                active: true,
                plugin_index,
                parameter_id,
                points: slint::ModelRc::new(VecModel::from(vec![Z_AutomationPoint {
                    beat,
                    value,
                    curve: 0.0,
                }])),
            });
            track.auto_lanes = slint::ModelRc::new(VecModel::from(lanes));
            track.show_automation = true;
            tracks.set_row_data(row, track);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!(
                        "PLUGIN AUTOMATION CREATED: TRACK {} P{}",
                        track_id, parameter_id
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<PluginActions>().on_set_plugin_bypass({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, bypassed| {
            let applied = track_id >= 0
                && plugin_index >= 0
                && core.set_plugin_bypass(track_id as u32, plugin_index as u32, bypassed);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!(
                        "PLUGIN {} · {}",
                        plugin_index,
                        if bypassed { "BYPASSED" } else { "ENABLED" }
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "plugin bypass rejected by host").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_reset_sandboxed_plugin({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index| {
            let applied = track_id >= 0
                && plugin_index >= 0
                && core.reset_sandboxed_plugin(track_id as u32, plugin_index as u32);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!("PLUGIN {} · HOST RESET REQUESTED", plugin_index).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "plugin host reset rejected").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_open_native_editor({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index| {
            if let Some(ui) = weak.upgrade() {
                let result = plugin_editor_parent_handle(&ui).map_or_else(
                    || {
                        r#"{"ok":false,"code":"native_editor_parent_unavailable","retryable":true}"#
                            .to_owned()
                    },
                    |parent| {
                        core.open_plugin_native_editor_json(
                            track_id.max(0) as u32,
                            plugin_index.max(0) as u32,
                            parent,
                        )
                    },
                );
                let status = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .map(|value| {
                        if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                            format!("PLUGIN {} · NATIVE EDITOR OPENED", plugin_index)
                        } else {
                            match value.get("code").and_then(serde_json::Value::as_str) {
                                Some("native_editor_open_failed") => {
                                    "NATIVE EDITOR FAILED · PLUGIN COULD NOT ATTACH TO THIS WINDOW"
                                        .to_string()
                                }
                                Some("native_editor_parent_unavailable") => {
                                    "NATIVE EDITOR UNAVAILABLE · PLATFORM WINDOW HANDLE MISSING"
                                        .to_string()
                                }
                                Some("native_editor_unavailable") => {
                                    "NATIVE EDITOR UNAVAILABLE FOR THIS PLUGIN".to_string()
                                }
                                Some(code) => format!("NATIVE EDITOR FAILED · {}", code),
                                None => "NATIVE EDITOR FAILED · INVALID HOST RESPONSE".to_string(),
                            }
                        }
                    })
                    .unwrap_or_else(|| "NATIVE EDITOR FAILED · INVALID HOST RESPONSE".to_string());
                ui.set_last_action(status.into());
            }
        }
    });
    ui.global::<PluginActions>().on_close_native_editor({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index| {
            let result = core.close_plugin_native_editor_json(
                track_id.max(0) as u32,
                plugin_index.max(0) as u32,
            );
            if let Some(ui) = weak.upgrade() {
                let status = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    == Some(true);
                ui.set_last_action(if status {
                    format!("PLUGIN {} · NATIVE EDITOR CLOSED", plugin_index).into()
                } else {
                    "NATIVE EDITOR CLOSE FAILED".into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_apply_dynamics_suggestion({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index| {
            if track_id < 0 || plugin_index < 0 {
                return;
            }
            let result = core.apply_dynamics_suggestion_diagnostic_json(
                track_id as u32,
                plugin_index as u32,
                Vec::new(),
            );
            if let Some(ui) = weak.upgrade() {
                let ok = serde_json::from_str::<serde_json::Value>(&result)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false);
                ui.set_last_action(if ok {
                    "DYNAMICS ASSISTANT · DEFAULT GLUE APPLIED · UNDO READY".into()
                } else {
                    format!("DYNAMICS ASSISTANT FAILED · {}", result).into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_filter({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |row, name, value| {
            if row < 0 || !value.is_finite() {
                return;
            }
            let Some(track) = tracks.row_data(row as usize) else {
                return;
            };
            let Some(parameter_text) = name.strip_prefix("PARAM_") else {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("FILTER PARAMETER REJECTED".into());
                }
                return;
            };
            let Ok(parameter) = parameter_text.parse::<u32>() else {
                return;
            };
            if parameter == 0 {
                return;
            }
            let applied = core.set_plugin_parameter(
                track.id.max(0) as u32,
                0,
                parameter - 1,
                value.clamp(0.0, 1.0),
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if applied {
                    format!("FILTER PARAM {}: APPLIED", parameter).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "filter parameter rejected by host")
                        .into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_macro_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |macro_index, value| {
            if macro_index < 0 || !value.is_finite() {
                return;
            }
            let normalized = value.clamp(0.0, 1.0);
            core.set_macro_value(macro_index as u32, normalized);
            if let Some(ui) = weak.upgrade() {
                let mut values: Vec<f32> = ui.get_track_macros().iter().collect();
                let index = macro_index as usize;
                if index >= values.len() {
                    return;
                }
                values[index] = normalized;
                ui.set_track_macros(slint::ModelRc::new(VecModel::from(values)));
                ui.set_last_action(format!("MACRO {}: {:.3}", macro_index, normalized).into());
            }
        }
    });
    ui.global::<PluginActions>().on_set_synth_engine({
        let weak = weak.clone();
        let core = core.clone();
        move |engine| {
            if !(0..=2).contains(&engine) {
                return;
            }
            core.set_preview_synth_engine(engine as u32);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!(
                        "SYNTH ENGINE: {}",
                        ["SYNTH", "WAVETABLE", "SAMPLER"][engine as usize]
                    )
                    .into(),
                );
            }
        }
    });
    ui.global::<PluginActions>().on_set_route({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, enabled| {
            if source_id < 0 || dest_id < 0 || source_id == dest_id {
                return;
            }
            let ok = core.set_route(source_id as u32, dest_id as u32, enabled);
            if let Some(ui) = weak.upgrade() {
                ui.set_sidechain_result(ok);
                ui.set_last_action(if ok {
                    format!(
                        "ROUTE: {} -> {} {}",
                        source_id,
                        dest_id,
                        if enabled { "CONNECTED" } else { "DISCONNECTED" }
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "route rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_route_gain({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, gain, enabled| {
            if source_id < 0 || dest_id < 0 || source_id == dest_id || !gain.is_finite() {
                return;
            }
            let ok = core.set_route_gain(
                source_id as u32,
                dest_id as u32,
                gain.clamp(0.0, 2.0),
                enabled,
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("ROUTE GAIN: {} -> {} = {:.2}", source_id, dest_id, gain).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "route gain rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_send_route({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, gain, pre_fader, enabled| {
            if source_id < 0 || dest_id < 0 || source_id == dest_id || !gain.is_finite() {
                return;
            }
            let ok = core.set_send_route(
                source_id as u32,
                dest_id as u32,
                gain.clamp(0.0, 2.0),
                pre_fader,
                enabled,
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!(
                        "SEND {} → {} · {:.2} · {}FADER · {}",
                        source_id,
                        dest_id,
                        gain,
                        if pre_fader { "PRE-" } else { "POST-" },
                        if enabled { "ON" } else { "OFF" }
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "send route rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_save_preset({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, path| {
            let ok = track_id >= 0
                && plugin_index >= 0
                && !path.trim().is_empty()
                && core.save_plugin_preset(track_id as u32, plugin_index as u32, path.as_str());
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("PLUGIN PRESET SAVED: {}", path).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "preset save rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_load_preset({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id, plugin_index, path| {
            let ok = track_id >= 0
                && plugin_index >= 0
                && !path.trim().is_empty()
                && core.load_plugin_preset(track_id as u32, plugin_index as u32, path.as_str());
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("PLUGIN PRESET LOADED: {}", path).into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "preset load rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_move_plugin({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |plugin_index, delta| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let selected =
                crate::slint_ui::clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "move failed: no track selected").into(),
                );
                return;
            };
            if plugin_index < 0 || !(-1..=1).contains(&delta) {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "move failed: invalid plugin index")
                        .into(),
                );
                return;
            }
            let destination = plugin_index + delta;
            let ok = destination >= 0
                && core.move_plugin(
                    track.id.max(0) as u32,
                    plugin_index as u32,
                    destination as u32,
                );
            if ok {
                sync_tracks_from_engine(&tracks, &core);
                ui.set_fx_active_id(destination);
                ui.set_last_action(
                    format!("PLUGIN MOVED {} → {}", plugin_index, destination).into(),
                );
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "plugin reorder rejected by host").into(),
                );
            }
        }
    });
    ui.global::<PluginActions>().on_browse_preset({
        let weak = weak.clone();
        move |for_save| {
            let dialog = rfd::FileDialog::new()
                .add_filter("Hirari Plugin Preset", &["aupreset"])
                .set_title(if for_save {
                    "Save Plugin Preset"
                } else {
                    "Load Plugin Preset"
                });
            let path = if for_save {
                dialog.set_file_name("PluginPreset.aupreset").save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(path) = path {
                if let Some(ui) = weak.upgrade() {
                    ui.set_plugin_preset_path(path.to_string_lossy().into_owned().into());
                    ui.set_last_action(format!("PLUGIN PRESET PATH: {}", path.display()).into());
                }
            }
        }
    });
    let plugin_scan_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    ui.global::<PluginActions>().on_rescan_plugins({
        let weak = weak.clone();
        let plugin_scan_in_flight = plugin_scan_in_flight.clone();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if plugin_scan_in_flight.swap(true, std::sync::atomic::Ordering::AcqRel) {
                ui.set_last_action("PLUGIN SCAN ALREADY RUNNING".into());
                return;
            }
            ui.set_plugin_catalog_status("SCANNING INSTALLED PLUGINS…".into());
            ui.set_last_action("SCANNING INSTALLED PLUGINS…".into());

            let weak = weak.clone();
            let in_flight_for_worker = plugin_scan_in_flight.clone();
            let spawn = std::thread::Builder::new()
                .name("hirari-plugin-catalog-scan".to_owned())
                .spawn(move || {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let result = std::panic::catch_unwind(|| {
                        (
                            hirari_core_bridge::plugin_catalog::json(),
                            scan_installed_plugin_count(),
                        )
                    });
                    let in_flight_for_ui = in_flight_for_worker.clone();
                    let callback = move || {
                        in_flight_for_ui.store(false, std::sync::atomic::Ordering::Release);
                        let Some(ui) = weak.upgrade() else {
                            return;
                        };
                        match result {
                            Ok((catalog_json, (discovered, rejected))) => {
                                let catalog = crate::ui::plugin_commands::
                                    apply_plugin_catalog_json_to_ui(&catalog_json, &ui);
                                ui.set_last_action(
                                    format!(
                                        "PLUGIN SCAN COMPLETE: {} AVAILABLE · {} DISCOVERED · {} REJECTED",
                                        catalog, discovered, rejected
                                    )
                                    .into(),
                                );
                            }
                            Err(_) => {
                                ui.set_plugin_catalog_status("PLUGIN SCAN FAILED".into());
                                ui.set_last_action("PLUGIN SCAN FAILED · RETRY FROM THE PLUGIN BROWSER".into());
                            }
                        }
                    };
                    let in_flight_on_dispatch_failure = in_flight_for_worker;
                    if slint::invoke_from_event_loop(callback).is_err() {
                        in_flight_on_dispatch_failure
                            .store(false, std::sync::atomic::Ordering::Release);
                    }
                });
            if let Err(error) = spawn {
                plugin_scan_in_flight.store(false, std::sync::atomic::Ordering::Release);
                ui.set_plugin_catalog_status("PLUGIN SCAN COULD NOT START".into());
                ui.set_last_action(format!("PLUGIN SCAN COULD NOT START · {error}").into());
            }
        }
    });
    ui.global::<PluginActions>().on_insert_plugin({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |plugin_id| {
            if let Some(ui) = weak.upgrade() {
                crate::ui::plugin_commands::insert_catalog_plugin(&plugin_id, &core, &ui, &tracks);
            }
        }
    });
    ui.global::<PluginActions>().on_set_sidechain({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, plugin_index, tap_point, enabled| {
            let ok = source_id >= 0
                && dest_id >= 0
                && plugin_index >= 0
                && tap_point >= 0
                && core.set_sidechain_link(
                    source_id as u32,
                    dest_id as u32,
                    plugin_index as u32,
                    tap_point as u32,
                    enabled,
                );
            if let Some(ui) = weak.upgrade() {
                if ok {
                    ui.set_sidechain_result(enabled);
                }
                ui.set_last_action(if ok {
                    format!(
                        "SIDECHAIN {}: {} -> {} FX {}",
                        if enabled { "CONNECTED" } else { "DISCONNECTED" },
                        source_id,
                        dest_id,
                        plugin_index
                    )
                    .into()
                } else {
                    ui_error_message(UiErrorKind::Plugin, "sidechain rejected by Core").into()
                });
            }
        }
    });
    ui.global::<PluginActions>().on_set_sidechain_tap_point({
        let weak = weak.clone();
        let core = core.clone();
        move |source_id, dest_id, plugin_index, tap_point| {
            let valid =
                source_id >= 0 && dest_id >= 0 && plugin_index >= 0 && (0..=2).contains(&tap_point);
            let ok = valid
                && core.set_sidechain_link(
                    source_id as u32,
                    dest_id as u32,
                    plugin_index as u32,
                    tap_point as u32,
                    true,
                );
            if let Some(ui) = weak.upgrade() {
                if ok {
                    ui.set_sidechain_tap_point(tap_point);
                    ui.set_sidechain_result(true);
                    ui.set_last_action(format!("SIDECHAIN TAP UPDATED: {}", tap_point).into());
                } else if valid {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Plugin,
                            "sidechain tap update rejected by Core",
                        )
                        .into(),
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::apply_plugin_parameter;
    use hirari_core_bridge::HirariCore;

    #[test]
    fn plugin_parameter_binding_validates_and_normalizes_before_core_apply() {
        let core = HirariCore::new().expect("core must initialize");
        let track_id = core.add_track(0);
        assert!(core.add_plugin(track_id, 0));

        assert!(!apply_plugin_parameter(&core, -1, 0, 0, 50.0));
        assert!(!apply_plugin_parameter(
            &core,
            track_id as i32,
            0,
            0,
            f32::NAN
        ));
        assert!(apply_plugin_parameter(&core, track_id as i32, 0, 0, 150.0));
        assert!(apply_plugin_parameter(&core, track_id as i32, 0, 0, -20.0));
    }
}
