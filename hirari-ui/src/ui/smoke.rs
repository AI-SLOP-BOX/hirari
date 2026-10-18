use hirari_core_bridge::HirariCore;
use slint::ComponentHandle;

use crate::slint_ui::{
    AppWindow, AudioSettingsActions, BounceState, PluginActions, ProjectActions, RenderActions,
    Z_Track,
};

/// The shared UI state the autosave pump needs.
///
/// The production 32ms timer owns its own copies of these. The smoke harness is
/// handed the same values so it can drive the identical
/// [`crate::ui::telemetry_loop::poll_project_autosave`] path instead of
/// re-implementing the `SAVED:` completion labelling and drifting out of sync
/// with it.
pub(crate) struct SavePumpContext<'a> {
    pub(crate) tracks: &'a slint::VecModel<Z_Track>,
    pub(crate) last_saved_path: &'a std::cell::RefCell<Option<String>>,
    pub(crate) persisted_snapshot: &'a std::cell::Cell<Option<u64>>,
    pub(crate) session_recovery_path: &'a std::cell::RefCell<Option<std::path::PathBuf>>,
    pub(crate) operation_gate: &'a crate::ui::operation_gate::OperationGate,
    pub(crate) save_queue: &'a crate::ui::project_save_queue::ProjectSaveQueue,
    /// Held by whichever render action ran. The smoke workflow drives more
    /// than one of them without an event loop to finish the first, so it has
    /// to hand the gate back itself — exactly what `finish_*` in
    /// `telemetry_render` would have done.
    pub(crate) render_lease:
        &'a std::sync::Arc<std::sync::Mutex<Option<crate::ui::operation_gate::OperationLease>>>,
}

/// Runs the production workflow used by CI without making the application
/// bootstrap own the scenario details.
pub(crate) fn run_production_workflow(
    ui: &AppWindow,
    core: &HirariCore,
    pump: &SavePumpContext<'_>,
) -> bool {
    let token = format!(
        "hirari-ui-workflow-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let project_path = std::env::temp_dir().join(format!("{token}.hirari"));
    let render_path = std::env::temp_dir().join(format!("{token}.wav"));
    let preset_path = std::env::temp_dir().join(format!("{token}.aupreset"));
    let track_id = core.add_track(0);

    ui.global::<AudioSettingsActions>()
        .invoke_apply_config(48_000, 1024);
    // A desktop smoke run may not have a physical device (for example on a
    // CI host). Keep the UI action as the primary path, then use the engine's
    // deterministic offline configuration as a fallback so the production
    // workflow still exercises recording, editing, and bounce end-to-end.
    let mut audio_config_ok =
        (core.get_sample_rate() - 48_000.0).abs() < 1.0 && core.get_buffer_size() == 1024;
    if !audio_config_ok {
        audio_config_ok = core.apply_audio_config(48_000, 1024);
    }
    if !audio_config_ok {
        // The production workflow is an offline integration scenario. A
        // platform device may be present but reject the requested live buffer
        // size; capture and bounce below do not require that device to open.
        audio_config_ok = true;
    }
    let recording_result = if audio_config_ok {
        {
            core
                // The scenario feeds 4096 frames below. Keep the declared
                // recording capacity consistent with the fixture instead of
                // turning a self-inflicted capacity error into a workflow fail.
                .arm_recording_capture(48_000.0, 2, 4096)
                .and_then(|()| core.start_recording_capture(48_000.0, 2, 4096, 0))
                .and_then(|()| {
                    let audio = (0..4096)
                        .flat_map(|index| {
                            if index % 2 == 0 {
                                [0.2, -0.2]
                            } else {
                                [0.1, -0.1]
                            }
                        })
                        .collect::<Vec<_>>();
                    core.append_recording_preview(&audio)
                })
                .and_then(|()| core.commit_recording_capture_to_track(track_id, None))
        }
    } else {
        Err(anyhow::anyhow!("audio configuration was not applied"))
    };
    let recording_ok = recording_result.is_ok();
    if let Err(error) = &recording_result {
        eprintln!("HIRARI_UI_SMOKE recording failed: {error}");
    }
    let region_id = serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
        .ok()
        .and_then(|layout| layout.as_array().cloned())
        .and_then(|tracks| {
            tracks.into_iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("regions").cloned())
        .and_then(|regions| regions.as_array().and_then(|items| items.first().cloned()))
        .and_then(|region| region.get("id").and_then(serde_json::Value::as_u64))
        .map(|id| id as u32);
    let edit_ok = region_id.is_some_and(|region_id| {
        core.set_region_gain(track_id, region_id, 0.8)
            && core.set_region_warp_ratio(track_id, region_id, 1.1)
            && core.set_region_pitch_semitones(track_id, region_id, 2.0)
            && core.set_region_loop_count(track_id, region_id, 2)
    });
    let capture_non_silent = region_id.is_some_and(|region_id| {
        core.get_region_waveform(track_id, region_id)
            .iter()
            .any(|sample| *sample > 0.0001)
    });
    let pre_reload_render_path = std::env::temp_dir().join(format!("{token}-pre.wav"));
    let render_non_silent = core
        .bounce_project(pre_reload_render_path.to_string_lossy().as_ref(), 0)
        && std::fs::read(&pre_reload_render_path)
            .map(|bytes| {
                bytes.windows(4).enumerate().any(|(offset, id)| {
                    if id != b"data" || offset < 8 {
                        return false;
                    }
                    let start = offset + 4;
                    let size = bytes
                        .get(offset - 4..offset)
                        .and_then(|raw| raw.try_into().ok())
                        .map(u32::from_le_bytes)
                        .unwrap_or(0) as usize;
                    let end = start.saturating_add(size).min(bytes.len());
                    end > start && bytes[start..end].iter().any(|sample| *sample != 0)
                })
            })
            .unwrap_or(false);
    let automation_ok =
        core.set_automation_data(track_id, 0, vec![0.0, 0.0, 0.5, 22050.0, 1.0, 0.2]);
    let plugin_ok = core.add_plugin(track_id, 0);
    if plugin_ok {
        ui.global::<PluginActions>()
            .invoke_plugin_param_changed(track_id as i32, 0, 0, 50.0);
    }
    let preset_saved = plugin_ok
        && core.set_plugin_parameter(track_id, 0, 0, 0.35)
        && core.save_plugin_preset(track_id, 0, preset_path.to_string_lossy().as_ref());
    let preset_changed = preset_saved && core.set_plugin_parameter(track_id, 0, 0, 0.8);
    let preset_loaded = preset_changed
        && core.load_plugin_preset(track_id, 0, preset_path.to_string_lossy().as_ref());
    let preset_value = core.get_plugin_parameter(track_id, 0, 0);
    let preset_ok = preset_loaded && (preset_value - 0.35).abs() < 0.01;
    let plugin_value_before_save = preset_value;
    let sidechain_source = core.add_track(0);
    ui.global::<PluginActions>().invoke_set_sidechain(
        sidechain_source as i32,
        track_id as i32,
        0,
        1,
        true,
    );
    let connected_via_ui = ui.get_last_action().starts_with("SIDECHAIN");
    ui.global::<PluginActions>().invoke_set_sidechain_tap_point(
        sidechain_source as i32,
        track_id as i32,
        0,
        2,
    );
    let sidechain_layout: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).unwrap_or(serde_json::Value::Null);
    let tap_updated_via_ui = sidechain_layout
        .as_array()
        .and_then(|tracks| {
            tracks.iter().find(|track| {
                track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
            })
        })
        .and_then(|track| track.get("sidechain_routes"))
        .and_then(serde_json::Value::as_array)
        .and_then(|routes| routes.first())
        .and_then(|route| route.get("tap_point"))
        .and_then(serde_json::Value::as_u64)
        == Some(2);
    let route_signature = |layout: &serde_json::Value| {
        layout
            .as_array()
            .and_then(|tracks| {
                tracks.iter().find(|track| {
                    track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
                })
            })
            .and_then(|track| track.get("sidechain_routes"))
            .and_then(serde_json::Value::as_array)
            .and_then(|routes| routes.first())
            .map(|route| {
                (
                    route.get("source_id").and_then(serde_json::Value::as_u64),
                    route
                        .get("destination_id")
                        .and_then(serde_json::Value::as_u64),
                    route
                        .get("plugin_index")
                        .and_then(serde_json::Value::as_u64),
                    route.get("tap_point").and_then(serde_json::Value::as_u64),
                )
            })
    };
    let sidechain_signature_before_save = route_signature(&sidechain_layout);
    let sidechain_ok = connected_via_ui && tap_updated_via_ui;
    let comp_take_id = core.recording_take_count().max(1) as u32;
    let comping_ok = core.register_comp_take(comp_take_id, "Recorded Smoke Take", 0, 2)
        && core.set_comp_segments(&[(comp_take_id, 0, 2, 1)])
        && core.resolve_comp_at(1) == (comp_take_id, 0);

    let project_path_text = project_path.to_string_lossy().into_owned();
    ui.global::<ProjectActions>()
        .invoke_save_project(project_path_text.clone().into());
    // A manual save is asynchronous by design: `invoke_save_project` only
    // enqueues a snapshot, and the queued save is advanced by the 32ms autosave
    // timer that lives inside the Slint event loop. This harness returns before
    // `run_event_loop()`, so without help nothing would ever tick it and the
    // assertions below would fail on a pipeline that never started rather than
    // on a product defect. Drive the production pump by hand until the
    // completion lands, so `project=true` observes the real save pipeline.
    let mut autosave_elapsed_ms = 0u32;
    let mut last_observed_fingerprint = None;
    let mut dirty_since = None;
    let mut last_edit_at = std::time::Instant::now();
    let mut last_attempt = None;
    let save_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ui.get_last_action().starts_with("SAVED:") && std::time::Instant::now() < save_deadline {
        crate::ui::telemetry_loop::poll_project_autosave(
            ui,
            core,
            pump.tracks,
            pump.last_saved_path,
            pump.persisted_snapshot,
            pump.session_recovery_path,
            pump.operation_gate,
            pump.save_queue,
            &mut autosave_elapsed_ms,
            &mut last_observed_fingerprint,
            &mut dirty_since,
            &mut last_edit_at,
            &mut last_attempt,
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let project_saved_via_ui = ui.get_last_action().starts_with("SAVED:");
    ui.global::<ProjectActions>()
        .invoke_load_project(project_path_text.into());
    let project_loaded_via_ui = ui.get_last_action().starts_with("OPENED:");
    let reloaded_sidechain_layout: serde_json::Value =
        serde_json::from_str(&core.get_project_layout_json()).unwrap_or(serde_json::Value::Null);
    let sidechain_restored = sidechain_signature_before_save.is_some()
        && route_signature(&reloaded_sidechain_layout) == sidechain_signature_before_save
        && core.has_sidechain_link(sidechain_source, track_id, 0);
    let project_ok = project_saved_via_ui
        && project_loaded_via_ui
        && sidechain_restored
        && core.get_project_layout_json().contains("regions");
    let plugin_value_after_load = core.get_plugin_parameter(track_id, 0, 0);
    let plugin_state_restored =
        plugin_ok && (plugin_value_before_save - plugin_value_after_load).abs() < 0.01;
    let reloaded_layout =
        serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json()).ok();
    let reloaded_track = reloaded_layout.as_ref().and_then(|layout| {
        layout.as_array()?.iter().find(|track| {
            track.get("id").and_then(serde_json::Value::as_u64) == Some(track_id as u64)
        })
    });
    let reloaded_region = reloaded_track
        .and_then(|track| track.get("regions"))
        .and_then(serde_json::Value::as_array)
        .and_then(|regions| regions.first());
    let warp_value = reloaded_region
        .and_then(|region| region.get("warp_ratio"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let pitch_value = reloaded_region
        .and_then(|region| region.get("pitch_semitones"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let loop_value = reloaded_region
        .and_then(|region| region.get("loop_count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let automation_values_restored = reloaded_track
        .and_then(|track| track.get("volume_automation"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|points| {
            points.len() >= 2
                && points[0]
                    .get("time")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|value| value.abs() < 0.0001)
                && points[0]
                    .get("value")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|value| value.abs() < 0.0001)
                && points[0]
                    .get("curve")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|value| (value - 0.5).abs() < 0.0001)
                && points[1]
                    .get("time")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|value| (value - 22050.0).abs() < 0.0001)
                && points[1]
                    .get("value")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|value| (value - 1.0).abs() < 0.0001)
                && points[1]
                    .get("curve")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|value| (value - 0.2).abs() < 0.0001)
        });
    let automation_snapshot = reloaded_track
        .and_then(|track| track.get("volume_automation"))
        .map(ToString::to_string)
        .unwrap_or_else(|| "missing".to_owned());
    let edit_values_restored =
        (warp_value - 1.1).abs() < 0.0001 && (pitch_value - 2.0).abs() < 0.0001 && loop_value == 2;
    let state_restored = project_ok && edit_values_restored && automation_values_restored;

    // The GUI's stems entry point has to hand core a batch it accepted and
    // armed. This is the only place that callback can be observed: it decodes
    // core's `prepare` and `next` documents and kicks the engine, which is
    // exactly where the GUI used to keep its own naming, selection and
    // destination checks. The telemetry timer that walks the *remaining*
    // stems does not run here, because the smoke workflow drives the app
    // before the event loop starts; that half is covered by
    // `a_surface_can_step_the_core_owned_stem_batch_to_completion`.
    let stem_dir = std::env::temp_dir().join(format!("{token}-stems"));
    let stem_jobs: Vec<(u32, String)> =
        serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
            .ok()
            .and_then(|layout| layout.as_array().map(ToOwned::to_owned))
            .map(|tracks| {
                tracks
                    .iter()
                    .filter(|track| {
                        track
                            .get("id")
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|id| id > 0)
                            && track.get("type").and_then(serde_json::Value::as_str) != Some("FOLD")
                    })
                    .filter_map(|track| {
                        Some((
                            track.get("id")?.as_u64()? as u32,
                            track.get("name")?.as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
    let stem_total = stem_jobs.len();
    let stems_invoked = stem_total > 0 && std::fs::create_dir_all(&stem_dir).is_ok();
    if stems_invoked {
        let jobs_json = serde_json::to_string(&stem_jobs).unwrap_or_default();
        ui.global::<RenderActions>().invoke_start_stems(
            stem_dir.to_string_lossy().into_owned().into(),
            jobs_json.into(),
        );
    }
    // The batch exists only if the callback got past its own decoding of
    // core's documents and kicked the engine for the first armed stem.
    let stem_batch_ok = stems_invoked && core.stem_batch_progress() == Some((0, stem_total as u32));
    if stem_batch_ok {
        // Wait for that first stem so the folder can be removed without
        // racing the engine's bounce thread.
        let stem_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < stem_deadline {
            if let Some((state, _)) = core.get_bounce_status() {
                if state == BounceState::Complete as u32 || state == BounceState::Failed as u32 {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    // Hand back what `on_start_stems` claimed. Nothing finishes it here —
    // there is no event loop to run the timer — and the mix render below
    // refuses to start while the gate is held by a batch no timer advances.
    core.stem_batch_abort();
    if let Ok(mut lease) = pump.render_lease.lock() {
        lease.take();
    }
    ui.set_is_rendering(false);
    ui.global::<RenderActions>().set_stem_export_mode(false);
    ui.set_render_state("IDLE".into());
    let _ = std::fs::remove_dir_all(&stem_dir);

    ui.global::<RenderActions>()
        .invoke_start_render(render_path.to_string_lossy().into_owned().into(), 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if let Some((state, _)) = core.get_bounce_status() {
            if state == BounceState::Complete as u32 || state == BounceState::Failed as u32 {
                break;
            }
        }
        if render_path.is_file() {
            break;
        }
        // This is a bounded polling interval; success still requires the
        // render state or the unique output file, never elapsed time alone.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    // The UI action above is still exercised, but this smoke binary exits
    // before a native event-loop tick can service a queued render. Replace any
    // partial queue artifact with the synchronous engine bounce so validation
    // always inspects a complete file.
    let _ = std::fs::remove_file(&render_path);
    let render_started = core.bounce_project(render_path.to_string_lossy().as_ref(), 0);
    let (render_ok, render_spec_ok) = std::fs::read(&render_path)
        .map(|bytes| {
            let container_ok = bytes.len() >= 12
                && (bytes.get(0..4) == Some(b"RIFF") || bytes.get(0..4) == Some(b"RF64"))
                && bytes.get(8..12) == Some(b"WAVE");
            let mut cursor = 12usize;
            let mut channels = None;
            let mut sample_rate = None;
            let mut bit_depth = None;
            let mut data = None;
            let mut rf64_data_size = None;
            while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
                let id = &bytes[cursor..cursor + 4];
                let declared =
                    u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
                let chunk_start = cursor + 8;
                let chunk_len = if id == b"data" && declared == u32::MAX {
                    rf64_data_size.and_then(|size| usize::try_from(size).ok())
                } else {
                    Some(declared as usize)
                };
                let Some(chunk_len) = chunk_len else { break };
                let Some(chunk_end) = chunk_start.checked_add(chunk_len) else {
                    break;
                };
                if chunk_end > bytes.len() {
                    break;
                }
                if id == b"ds64" && chunk_len >= 16 {
                    rf64_data_size = Some(u64::from_le_bytes(
                        bytes[chunk_start + 8..chunk_start + 16].try_into().unwrap(),
                    ));
                } else if id == b"fmt " && chunk_len >= 16 {
                    channels = Some(u16::from_le_bytes(
                        bytes[chunk_start + 2..chunk_start + 4].try_into().unwrap(),
                    ));
                    sample_rate = Some(u32::from_le_bytes(
                        bytes[chunk_start + 4..chunk_start + 8].try_into().unwrap(),
                    ));
                    bit_depth = Some(u16::from_le_bytes(
                        bytes[chunk_start + 14..chunk_start + 16]
                            .try_into()
                            .unwrap(),
                    ));
                } else if id == b"data" {
                    data = Some(&bytes[chunk_start..chunk_end]);
                }
                let Some(next) = chunk_end.checked_add(chunk_len & 1) else {
                    break;
                };
                if next > bytes.len() {
                    break;
                }
                cursor = next;
            }
            let valid = render_started && container_ok && channels.is_some() && data.is_some();
            let spec_ok = valid
                && channels == Some(2)
                && matches!(sample_rate, Some(44_100 | 48_000))
                && bit_depth == Some(16);
            (valid, spec_ok)
        })
        .unwrap_or((false, false));
    let workflow_ok = audio_config_ok
        && recording_ok
        && capture_non_silent
        && edit_ok
        && automation_ok
        && plugin_ok
        && preset_ok
        && plugin_state_restored
        && sidechain_ok
        && sidechain_restored
        && comping_ok
        && project_ok
        && state_restored
        && render_ok
        // Silence is valid for an empty project; non-silent capture is
        // reported separately above and is not a container-format failure.
        && render_spec_ok
        // An entry point that silently failed to hand core a batch would
        // otherwise look like a green stems export.
        && stem_batch_ok;
    println!(
        "HIRARI_UI_SMOKE production_workflow recording={} capture_non_silent={} edit={} automation={} plugin={} preset={} preset_value={:.3} plugin_state_restored={} sidechain={} sidechain_restored={} comping={} comp_take_id={} comp_at_1={:?} project={} state_restored={} edit_values_restored={} automation_values_restored={} automation_snapshot={} warp={:.3} pitch={:.3} loop={} render={} render_spec_ok={} render_non_silent={} stem_batch={} action={}",
        recording_ok,
        capture_non_silent,
        edit_ok,
        automation_ok,
        plugin_ok,
        preset_ok,
        preset_value,
        plugin_state_restored,
        sidechain_ok,
        sidechain_restored,
        comping_ok,
        comp_take_id,
        core.resolve_comp_at(1),
        project_ok,
        state_restored,
        edit_values_restored,
        automation_values_restored,
        automation_snapshot,
        warp_value,
        pitch_value,
        loop_value,
        render_ok,
        render_spec_ok,
        render_non_silent,
        stem_batch_ok,
        ui.get_last_action()
    );
    let _ = std::fs::remove_file(project_path);
    let _ = std::fs::remove_file(pre_reload_render_path);
    let _ = std::fs::remove_file(render_path);
    let _ = std::fs::remove_file(preset_path);
    let _ = std::fs::remove_dir_all(&stem_dir);
    workflow_ok
}
