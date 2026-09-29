//! Main-thread engine telemetry and UI synchronization loop.

use crate::slint_ui::*;
use hirari_core_bridge::HirariCore;
use slint::{Model, SharedString};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn commit_recording_targets(
    core: &HirariCore,
    tracks: &slint::VecModel<Z_Track>,
    targets: &[(u32, Vec<u16>)],
    project_path: Option<&str>,
) -> Option<(usize, String)> {
    if targets.is_empty() {
        return None;
    }
    let names = targets
        .iter()
        .map(|(id, _)| {
            (0..tracks.row_count())
                .filter_map(|row| tracks.row_data(row))
                .find(|track| track.id.max(0) as u32 == *id)
                .map(|track| track.name.to_string())
        })
        .collect::<Option<Vec<_>>>()?
        .join(", ");
    let frames = core
        .commit_recording_capture_to_tracks(targets, project_path)
        .ok()?;
    Some((frames, names))
}

fn active_route_matrix(core: &HirariCore, tracks: &slint::VecModel<Z_Track>) -> Vec<bool> {
    let mut matrix = vec![false; 100];
    let Ok(routes) = serde_json::from_str::<serde_json::Value>(&core.audio_routes_json()) else {
        return matrix;
    };
    let Some(routes) = routes.as_array() else {
        return matrix;
    };
    for route in routes {
        let Some(source) = route.get("source_id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(destination) = route
            .get("destination_id")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        let source_row = (0..tracks.row_count()).find(|&row| {
            tracks
                .row_data(row)
                .is_some_and(|track| track.id.max(0) as u64 == source)
        });
        let destination_row = (0..tracks.row_count()).find(|&row| {
            tracks
                .row_data(row)
                .is_some_and(|track| track.id.max(0) as u64 == destination)
        });
        if route
            .get("send")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        if let (Some(source_row), Some(destination_row)) = (source_row, destination_row) {
            if source_row < 10 && destination_row < 10 {
                matrix[source_row * 10 + destination_row] = true;
            }
        }
    }
    matrix
}

fn active_send_state(
    core: &HirariCore,
    tracks: &slint::VecModel<Z_Track>,
) -> (Vec<bool>, Vec<f32>, Vec<bool>) {
    let mut active = vec![false; 100];
    let mut gains = vec![0.0; 100];
    let mut pre_fader = vec![false; 100];
    let Ok(routes) = serde_json::from_str::<serde_json::Value>(&core.audio_routes_json()) else {
        return (active, gains, pre_fader);
    };
    let Some(routes) = routes.as_array() else {
        return (active, gains, pre_fader);
    };
    for route in routes {
        if !route
            .get("send")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(source) = route.get("source_id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(destination) = route
            .get("destination_id")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        let source_row = (0..tracks.row_count()).find(|&row| {
            tracks
                .row_data(row)
                .is_some_and(|track| track.id.max(0) as u64 == source)
        });
        let destination_row = (0..tracks.row_count()).find(|&row| {
            tracks
                .row_data(row)
                .is_some_and(|track| track.id.max(0) as u64 == destination && track.r#type == "Bus")
        });
        if let (Some(source_row), Some(destination_row)) = (source_row, destination_row) {
            if source_row < 10 && destination_row < 10 {
                let index = source_row * 10 + destination_row;
                active[index] = true;
                gains[index] = route
                    .get("gain")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(1.0)
                    .clamp(0.0, 2.0) as f32;
                pre_fader[index] = route
                    .get("pre_fader")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
            }
        }
    }
    (active, gains, pre_fader)
}

fn valid_time_signatures(events: &[f64]) -> Vec<(f64, u8, u8)> {
    let mut meters = events
        .chunks_exact(3)
        .filter_map(|event| {
            let beat = event[0];
            let numerator = event[1];
            let denominator = event[2];
            if !beat.is_finite()
                || beat < 0.0
                || !numerator.is_finite()
                || numerator.fract() != 0.0
                || !(1.0..=32.0).contains(&numerator)
                || !denominator.is_finite()
                || denominator.fract() != 0.0
                || ![1.0, 2.0, 4.0, 8.0, 16.0, 32.0].contains(&denominator)
            {
                return None;
            }
            Some((beat, numerator as u8, denominator as u8))
        })
        .collect::<Vec<_>>();
    meters.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut unique_meters = Vec::with_capacity(meters.len());
    for meter in meters {
        if let Some(previous) = unique_meters.last_mut() {
            let previous: &mut (f64, u8, u8) = previous;
            if (previous.0 - meter.0).abs() <= f64::EPSILON {
                *previous = meter;
                continue;
            }
        }
        unique_meters.push(meter);
    }
    unique_meters
}

fn time_signature_at(events: &[(f64, u8, u8)], beat: f32) -> (u8, u8) {
    events
        .iter()
        .take_while(|event| event.0 <= f64::from(beat.max(0.0)) + 1e-6)
        .last()
        .map(|event| (event.1, event.2))
        .unwrap_or((4, 4))
}

fn arrangement_timeline_end(last_clip_end: f32, events: &[(f64, u8, u8)]) -> f32 {
    const EPSILON_BEATS: f64 = 1e-6;
    let last_clip_end = f64::from(last_clip_end.max(0.0));
    let active_meter = events
        .iter()
        .take_while(|event| event.0 <= last_clip_end + EPSILON_BEATS)
        .last()
        .copied()
        .unwrap_or((0.0, 4, 4));
    let bar_length = f64::from(active_meter.1) * 4.0 / f64::from(active_meter.2);
    let bars_from_anchor = ((last_clip_end - active_meter.0).max(0.0) / bar_length).ceil();
    let mut boundary = active_meter.0 + bars_from_anchor * bar_length;
    if boundary + EPSILON_BEATS < last_clip_end {
        boundary += bar_length;
    }
    if let Some(next_meter) = events.iter().find(|event| {
        event.0 > active_meter.0 + EPSILON_BEATS && event.0 <= boundary + EPSILON_BEATS
    }) {
        boundary = next_meter.0;
    }

    for _ in 0..4 {
        let meter = events
            .iter()
            .take_while(|event| event.0 <= boundary + EPSILON_BEATS)
            .last()
            .copied()
            .unwrap_or((0.0, 4, 4));
        let next_bar = boundary + f64::from(meter.1) * 4.0 / f64::from(meter.2);
        boundary = events
            .iter()
            .find(|event| event.0 > boundary + EPSILON_BEATS && event.0 < next_bar - EPSILON_BEATS)
            .map_or(next_bar, |event| event.0);
    }

    let timeline_end = boundary as f32;
    if timeline_end.is_finite() {
        timeline_end.max(128.0)
    } else {
        128.0
    }
}

fn arrangement_measure_starts(events: &[f64], end_beat: f32) -> Vec<Z_MeasureStart> {
    const MAX_MEASURE_MARKERS: usize = 16_384;
    let mut unique_meters = valid_time_signatures(events);
    if unique_meters
        .first()
        .is_none_or(|meter| meter.0 > f64::EPSILON)
    {
        unique_meters.insert(0, (0.0, 4, 4));
    }

    let end_beat = f64::from(end_beat.max(0.0));
    let mut starts = Vec::with_capacity(128);
    starts.push(Z_MeasureStart {
        beat: 0.0,
        number: 1,
    });
    for (index, (segment_start, numerator, denominator)) in
        unique_meters.iter().copied().enumerate()
    {
        if segment_start > end_beat {
            break;
        }
        if segment_start > starts.last().map_or(0.0, |marker| f64::from(marker.beat)) + 1e-6 {
            starts.push(Z_MeasureStart {
                beat: segment_start as f32,
                number: starts.len() as i32 + 1,
            });
        }
        let segment_end = unique_meters
            .get(index + 1)
            .map_or(end_beat, |next| next.0.min(end_beat));
        let beats_per_measure = f64::from(numerator) * 4.0 / f64::from(denominator);
        let mut next_start = segment_start + beats_per_measure;
        while next_start < segment_end - 1e-6
            && next_start <= end_beat
            && starts.len() < MAX_MEASURE_MARKERS
        {
            starts.push(Z_MeasureStart {
                beat: next_start as f32,
                number: starts.len() as i32 + 1,
            });
            next_start += beats_per_measure;
        }
        if starts.len() >= MAX_MEASURE_MARKERS {
            break;
        }
    }
    starts
}

#[allow(clippy::too_many_arguments)]
fn poll_project_autosave(
    ui: &AppWindow,
    core: &HirariCore,
    tracks: &slint::VecModel<Z_Track>,
    last_saved_path: &RefCell<Option<String>>,
    persisted_snapshot: &Cell<Option<u64>>,
    session_recovery_path: &RefCell<Option<std::path::PathBuf>>,
    operation_gate: &crate::ui::operation_gate::OperationGate,
    save_queue: &crate::ui::project_save_queue::ProjectSaveQueue,
    elapsed_ms: &mut u32,
    last_observed_fingerprint: &mut Option<u64>,
    dirty_since: &mut Option<Instant>,
    last_edit_at: &mut Instant,
    last_attempt: &mut Option<Instant>,
) {
    if let Some(mut completion) = save_queue.poll_completion(core) {
        if let Some(after_completion) = completion.after_completion.take() {
            after_completion(completion.success);
        }
        let current = crate::ui::project_state::save_fingerprint(
            core,
            tracks,
            ui.get_sequencer_patterns_json().as_str(),
        );
        match (&completion.intent, completion.success) {
            (crate::ui::project_save_queue::SaveIntent::Auto, true) => {
                persisted_snapshot.set(Some(completion.fingerprint));
                if current == Some(completion.fingerprint) {
                    *last_observed_fingerprint = current;
                    *dirty_since = None;
                    ui.set_project_save_status(if last_saved_path.borrow().is_some() {
                        "Auto-saved".into()
                    } else {
                        "Recovery copy saved".into()
                    });
                    ui.set_last_action(if last_saved_path.borrow().is_some() {
                        "PROJECT AUTO-SAVED · RECOVERY BACKUP UPDATED".into()
                    } else {
                        "UNSAVED SESSION RECOVERY COPY UPDATED".into()
                    });
                } else {
                    // New edits landed during disk I/O; preserve dirty state.
                    ui.set_project_save_status("Unsaved changes · Auto-save pending".into());
                }
            }
            (crate::ui::project_save_queue::SaveIntent::Auto, false) => {
                ui.set_project_save_status(if last_saved_path.borrow().is_some() {
                    "Auto-save failed · Save manually".into()
                } else {
                    "Auto-recovery failed · Save manually".into()
                });
                ui.set_last_action(
                    format!(
                        "PROJECT AUTO-SAVE FAILED · CHECK DISK AND SAVE MANUALLY ({})",
                        completion.path
                    )
                    .into(),
                );
                log::warn!("Hirari project auto-save failed for {}", completion.path);
            }
            (crate::ui::project_save_queue::SaveIntent::Manual { success_prefix, .. }, true) => {
                *last_saved_path.borrow_mut() = Some(completion.path.clone());
                crate::slint_ui::store_project_path(&completion.path);
                if let Some(recovery_path) = session_recovery_path.borrow_mut().take() {
                    let _ = crate::slint_ui::discard_session_recovery(&recovery_path);
                }
                persisted_snapshot.set(Some(completion.fingerprint));
                ui.set_project_path(completion.path.clone().into());
                ui.set_project_save_status(if current == Some(completion.fingerprint) {
                    "Saved".into()
                } else {
                    "Unsaved changes · Auto-save pending".into()
                });
                ui.set_last_action(
                    format!(
                        "{}: {}",
                        success_prefix,
                        crate::slint_ui::display_path(&completion.path)
                    )
                    .into(),
                );
                if current == Some(completion.fingerprint) {
                    *last_observed_fingerprint = current;
                    *dirty_since = None;
                } else {
                    *dirty_since = Some(Instant::now());
                }
            }
            (crate::ui::project_save_queue::SaveIntent::Manual { failure_label, .. }, false) => {
                ui.set_last_action(
                    crate::slint_ui::ui_error_message(
                        crate::slint_ui::UiErrorKind::Project,
                        &format!(
                            "{}: {}",
                            failure_label,
                            crate::slint_ui::display_path(&completion.path)
                        ),
                    )
                    .into(),
                );
                ui.set_project_save_status("Save failed · Previous project remains active".into());
            }
            (crate::ui::project_save_queue::SaveIntent::RecoveryAs { success_prefix }, true) => {
                *last_saved_path.borrow_mut() = Some(completion.path.clone());
                crate::slint_ui::store_project_path(&completion.path);
                if let Some(recovery_path) = session_recovery_path.borrow_mut().take() {
                    let _ = crate::slint_ui::discard_session_recovery(&recovery_path);
                }
                persisted_snapshot.set(Some(completion.fingerprint));
                ui.set_project_path(completion.path.clone().into());
                ui.set_project_save_status(if current == Some(completion.fingerprint) {
                    "Saved".into()
                } else {
                    "Unsaved changes · Auto-save pending".into()
                });
                ui.set_last_action(
                    format!(
                        "{}: {}",
                        success_prefix,
                        crate::slint_ui::display_path(&completion.path)
                    )
                    .into(),
                );
                if current == Some(completion.fingerprint) {
                    *last_observed_fingerprint = current;
                    *dirty_since = None;
                } else {
                    *dirty_since = Some(Instant::now());
                }
            }
            (crate::ui::project_save_queue::SaveIntent::RecoveryAs { .. }, false) => {}
        }
    }

    if save_queue.is_busy() {
        return;
    }
    *elapsed_ms = elapsed_ms.saturating_add(32);
    if *elapsed_ms < 5_000 {
        return;
    }
    *elapsed_ms -= 5_000;
    let now = Instant::now();
    let current = crate::ui::project_state::save_fingerprint(
        core,
        tracks,
        ui.get_sequencer_patterns_json().as_str(),
    );
    if current != *last_observed_fingerprint {
        *last_observed_fingerprint = current;
        *last_edit_at = now;
    }

    let Some(current) = current else {
        ui.set_project_save_status("Save state unavailable · Save manually".into());
        return;
    };
    let named_path = last_saved_path.borrow().clone();
    let recovery_path = session_recovery_path
        .borrow()
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    let target_path = named_path.clone().or(recovery_path);
    if persisted_snapshot.get() == Some(current) {
        *dirty_since = None;
        if ui.get_project_save_status().starts_with("Unsaved changes")
            || ui.get_project_save_status().starts_with("Auto-save failed")
        {
            ui.set_project_save_status(if named_path.is_some() {
                "Saved".into()
            } else {
                "Recovery copy saved".into()
            });
        }
        return;
    }

    let dirty_start = *dirty_since.get_or_insert(now);
    if named_path.is_none() {
        ui.set_project_save_status(if target_path.is_some() {
            "Unsaved project · Auto-recovery pending".into()
        } else {
            "Unsaved project · Save manually".into()
        });
    } else if !ui.get_project_save_status().starts_with("Auto-save failed") {
        ui.set_project_save_status("Unsaved changes · Auto-save pending".into());
    }

    let quiet_long_enough = now.duration_since(*last_edit_at) >= Duration::from_secs(10);
    let dirty_too_long = now.duration_since(dirty_start) >= Duration::from_secs(120);
    let retry_ready =
        last_attempt.is_none_or(|attempt| now.duration_since(attempt) >= Duration::from_secs(30));
    let can_save = target_path.is_some()
        && !ui.get_unsaved_recovery_available()
        && !ui.get_is_rec()
        && !ui.get_is_rendering()
        && (quiet_long_enough || dirty_too_long)
        && retry_ready;
    if !can_save {
        return;
    }
    let Some(_lease) = operation_gate.try_enter(crate::ui::operation_gate::OperationKind::Save)
    else {
        return;
    };
    *last_attempt = Some(now);
    let Some(path) = target_path else {
        return;
    };
    match save_queue.enqueue(
        &path,
        tracks,
        core,
        ui.get_sequencer_patterns_json().as_str(),
        current,
        crate::ui::project_save_queue::SaveIntent::Auto,
        _lease,
    ) {
        Ok(()) => {
            ui.set_project_save_status("Saving project snapshot…".into());
        }
        Err(error) => {
            log::warn!("Hirari project auto-save could not be queued: {error}");
            ui.set_project_save_status("Auto-save busy · Save manually if needed".into());
        }
    }
}

fn poll_dawproject_import(
    ui: &AppWindow,
    core: &HirariCore,
    tracks: &slint::VecModel<Z_Track>,
    markers: &Rc<slint::VecModel<Z_Marker>>,
    last_saved_path: &RefCell<Option<String>>,
    persisted_snapshot: &Cell<Option<u64>>,
    import_queue: &crate::ui::dawproject_import_queue::DawProjectImportQueue,
) {
    let Some(completion) = import_queue.poll_completion() else {
        return;
    };
    let report = match completion.result {
        Ok(report) => report,
        Err(error) => {
            ui.set_project_save_status("DAWproject import failed".into());
            ui.set_last_action(
                crate::ui::telemetry::ui_error_message(
                    crate::ui::telemetry::UiErrorKind::Project,
                    &format!("DAWproject import failed: {error}"),
                )
                .into(),
            );
            return;
        }
    };
    let previous_path = last_saved_path.borrow().clone();
    let result = crate::slint_ui::load_project_transactionally(
        &completion.destination,
        previous_path.as_deref(),
        tracks,
        core,
        |core| core.load_project(&completion.destination),
    );
    crate::ui::project_commands::detach_save_target_after_rollback_failure(
        ui,
        last_saved_path,
        &result,
    );
    if let Err(error) = result {
        ui.set_project_save_status("DAWproject imported · project load failed".into());
        ui.set_last_action(
            crate::ui::telemetry::ui_error_message(
                crate::ui::telemetry::UiErrorKind::Project,
                &format!("imported project could not be activated ({error:?})"),
            )
            .into(),
        );
        return;
    }
    crate::ui::project_commands::reset_project_scoped_ui(ui);
    ui.set_master_output_gain(core.master_gain());
    crate::ui::project::sync_markers_from_core(markers, core);
    *last_saved_path.borrow_mut() = Some(completion.destination.clone());
    crate::slint_ui::store_project_path(&completion.destination);
    persisted_snapshot.set(crate::ui::project_state::save_fingerprint(
        core,
        tracks,
        ui.get_sequencer_patterns_json().as_str(),
    ));
    ui.set_project_path(completion.destination.clone().into());
    ui.set_project_save_status("Imported · Saved".into());
    ui.set_last_action(
        format!(
            "DAWPROJECT IMPORTED: {} · {} tracks · {} audio clips · {} MIDI notes · {} embedded WAV files · {} plug-ins omitted",
            crate::slint_ui::display_path(&completion.destination),
            report.track_count,
            report.audio_clip_count,
            report.midi_note_count,
            report.embedded_media_count,
            report.unsupported_plugin_count,
        )
        .into(),
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn install_telemetry_loop(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<slint::VecModel<Z_Track>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    persisted_snapshot: Rc<Cell<Option<u64>>>,
    session_recovery_path: Rc<RefCell<Option<std::path::PathBuf>>>,
    operation_gate: crate::ui::operation_gate::OperationGate,
    save_queue: Rc<crate::ui::project_save_queue::ProjectSaveQueue>,
    import_queue: std::sync::Arc<crate::ui::dawproject_import_queue::DawProjectImportQueue>,
    markers: Rc<slint::VecModel<Z_Marker>>,
    tone_test_started_ms: Arc<AtomicU64>,
    tone_test_baseline_callbacks: Arc<AtomicU64>,
    render_started_ms: Arc<AtomicU64>,
    render_output_path: Arc<Mutex<std::path::PathBuf>>,
    render_lease: Arc<Mutex<Option<crate::ui::operation_gate::OperationLease>>>,
    stem_batch: Arc<Mutex<Option<crate::ui::render::StemExportBatch>>>,
    recording_target: Rc<RefCell<Option<Vec<(u32, Vec<u16>)>>>>,
    peak_reset_generation: Rc<Cell<u64>>,
    gpu_plot_store: crate::ui::gpu_canvas::PlotFrameStore,
) {
    let ui_timer_val = Arc::new(AtomicU64::new(0));
    // --- 4. ENGINE TELEMETRY LOOP ---

    let ui_sync = ui.as_weak();
    let core_tele = core.clone();
    let tracks_tele = tracks.clone();
    let markers_tele = markers.clone();
    let last_saved_path_tele = last_saved_path.clone();
    let persisted_snapshot_tele = persisted_snapshot.clone();
    let session_recovery_path_tele = session_recovery_path.clone();
    let recording_target_tele = recording_target.clone();
    let tone_test_started_ms_tele = tone_test_started_ms.clone();
    let tone_test_baseline_callbacks_tele = tone_test_baseline_callbacks.clone();
    let render_started_ms_tele = render_started_ms.clone();
    let render_output_path_tele = render_output_path.clone();
    let stem_batch_tele = stem_batch.clone();
    let gpu_plot_store_tele = gpu_plot_store.clone();
    let operation_gate_tele = operation_gate.clone();
    let import_queue_tele = import_queue.clone();
    let recovery_summary_queue = crate::ui::recovery::RecoverySummaryQueue::start();
    let mut peak_holds: Vec<f32> = vec![0.0; 10];
    let mut last_peak_reset_generation = peak_reset_generation.get();
    let mut last_video_revision = 0u64;
    let mut ui_timer = 0;
    let mut auto_save_elapsed_ms = 0u32;
    let mut last_observed_save_fingerprint = crate::ui::project_state::save_fingerprint(
        &core,
        &tracks,
        ui.get_sequencer_patterns_json().as_str(),
    );
    let mut dirty_since: Option<Instant> = None;
    let mut last_edit_at = Instant::now();
    let mut last_auto_save_attempt: Option<Instant> = None;
    let mut analysis_elapsed_ms: u32 = 0;
    let mut waveform_elapsed_ms: u32 = 0;
    let mut tempo_elapsed_ms: u32 = 0;
    let mut environment_elapsed_ms: u32 = 0;
    let mut plugin_elapsed_ms: u32 = 0;
    let mut pdc_elapsed_ms: u32 = 0;
    let mut last_plugin_context = (-1_i32, -1_i32);
    let mut last_pdc_context = (-1_i32, -1_i32);
    let mut last_plugin_values: Vec<f32> = Vec::new();
    let mut watchdog_total: u32 = 0;
    let mut last_piano_row: i32 = -1;
    let mut last_tempo_beats: Vec<f32> = Vec::new();
    let mut last_tempo_bpms: Vec<f32> = Vec::new();
    let mut last_route_matrix: Vec<bool> = Vec::new();
    let mut last_send_state: (Vec<bool>, Vec<f32>, Vec<bool>) =
        (Vec::new(), Vec::new(), Vec::new());
    let mut last_arrangement_measure_starts: Vec<Z_MeasureStart> = Vec::new();
    let mut last_time_signature_label = String::new();
    let mut last_device_catalog = String::new();
    let mut last_input_endpoint_uid = String::new();
    let mut last_pks: Vec<f32> = Vec::new();
    let mut last_peak_holds: Vec<f32> = Vec::new();
    let mut ev_buffer = Vec::with_capacity(128);
    let timer = slint::Timer::default();

    timer.start(
        slint::TimerMode::Repeated,
        // UI telemetry is intentionally capped at 30 Hz. Audio processing
        // remains real-time on its own thread; polling the entire project,
        // plugins and meters at 60 Hz made switching views needlessly costly.
        std::time::Duration::from_millis(32),
        move || {
            if let Some(ui) = ui_sync.upgrade() {
                ui_timer += 32;
                poll_project_autosave(
                    &ui,
                    &core_tele,
                    &tracks_tele,
                    &last_saved_path_tele,
                    &persisted_snapshot_tele,
                    &session_recovery_path_tele,
                    &operation_gate_tele,
                    &save_queue,
                    &mut auto_save_elapsed_ms,
                    &mut last_observed_save_fingerprint,
                    &mut dirty_since,
                    &mut last_edit_at,
                    &mut last_auto_save_attempt,
                );
                poll_dawproject_import(
                    &ui,
                    &core_tele,
                    &tracks_tele,
                    &markers_tele,
                    &last_saved_path_tele,
                    &persisted_snapshot_tele,
                    &import_queue_tele,
                );
                // A blank project has no meter, transport or analysis state
                // to publish. Avoid touching Slint properties in that state:
                // every property write schedules a full scene redraw.
                if tracks_tele.row_count() == 0 {
                    return;
                }
                analysis_elapsed_ms = analysis_elapsed_ms.saturating_add(32);
                waveform_elapsed_ms = waveform_elapsed_ms.saturating_add(32);
                tempo_elapsed_ms = tempo_elapsed_ms.saturating_add(32);
                environment_elapsed_ms = environment_elapsed_ms.saturating_add(32);
                plugin_elapsed_ms = plugin_elapsed_ms.saturating_add(32);
                pdc_elapsed_ms = pdc_elapsed_ms.saturating_add(32);
                let analysis_due = if analysis_elapsed_ms >= 64 {
                    analysis_elapsed_ms -= 64;
                    true
                } else {
                    false
                };
                let waveform_due = if waveform_elapsed_ms >= 128 {
                    waveform_elapsed_ms -= 128;
                    true
                } else {
                    false
                };
                ui_timer_val.store(ui_timer, Ordering::SeqCst);
                if ui.get_is_ply() || ui.get_show_video() || ui.get_bot_view() == 5 {
                    ui.set_ui_timer(ui_timer as f32);
                }
                ui.set_undo_depth(core_tele.undo_depth() as i32);
                ui.set_redo_depth(core_tele.redo_depth() as i32);
                if let Ok(Some(generation)) = core_tele.poll_preview_audio_decode() {
                    ui.set_last_action(format!("PREVIEWING · generation {}", generation).into());
                }

                crate::ui::plugin_telemetry::update_plugin_telemetry(
                    &ui,
                    &core_tele,
                    &tracks_tele,
                    &mut plugin_elapsed_ms,
                    &mut pdc_elapsed_ms,
                    &mut last_plugin_context,
                    &mut last_pdc_context,
                    &mut last_plugin_values,
                );

                if ui_timer % 1000 < 32 {
                    if let Some((completed_path, recovery_summary)) = recovery_summary_queue.poll()
                    {
                        let active_path = last_saved_path_tele.borrow().clone();
                        if active_path.as_deref() == Some(completed_path.as_str()) {
                            ui.set_recovery_backup_count(recovery_summary.count);
                            ui.set_recovery_backup_generations(slint::ModelRc::new(
                                slint::VecModel::from(recovery_summary.generations),
                            ));
                            ui.set_recovery_backup_summary(recovery_summary.text.into());
                        }
                    }
                    if let Some(path) = last_saved_path_tele.borrow().clone() {
                        recovery_summary_queue.request(&path);
                    }
                }

                // --- REAL TELEMETRY ---
                // Read the native health snapshot once. This keeps CPU load,
                // device state, transport state and sandbox failures from
                // being sampled from different engine moments in one frame.
                let mut health = core_tele.runtime_health_snapshot();
                let watchdog_trips = core_tele.take_watchdog_trips();
                watchdog_total = watchdog_total.saturating_add(watchdog_trips);
                let mut high_priority_diagnostic = false;
                ui.set_cpu_usage((health.dsp_load * 100.0).max(0.0));
                ui.set_diagnostic_state(health.status_text().into());
                ui.set_diagnostic_load((health.dsp_load * 100.0).max(0.0));
                ui.set_diagnostic_watchdogs(watchdog_total.min(i32::MAX as u32) as i32);
                ui.set_diagnostic_sandbox_failures(
                    health.sandbox_failures.min(i32::MAX as u32) as i32
                );
                ui.set_diagnostic_range_overflow(health.audio_range_overflow);
                ui.set_recording_take_count(
                    core_tele.recording_take_count().min(i32::MAX as usize) as i32,
                );
                ui.set_active_recording_take(
                    core_tele.active_recording_take().min(i32::MAX as usize) as i32,
                );
                ui.set_sandbox_failures(health.sandbox_failures.min(i32::MAX as u32) as i32);
                // Reconnect is a control-rate operation. Refresh the health
                // snapshot immediately after it completes so the UI does not
                // spend one extra second advertising a stale offline state.
                let was_audio_ready = health.audio_device_ready;
                if !was_audio_ready && ui_timer % 1000 < 32 {
                    core_tele.try_reconnect_audio_device();
                    health = core_tele.runtime_health_snapshot();
                    if health.audio_device_ready {
                        ui.set_last_action("AUDIO DEVICE RECONNECTED · TRANSPORT READY".into());
                    }
                }
                let audio_ready = health.audio_device_ready;
                let output_peak = health.peak_left.max(health.peak_right);
                if ui_timer % 1000 < 32 && health.audio_driver_status != "running" {
                    ui.set_audio_diagnostic(health.status_text().into());
                }
                if matches!(
                    health.state(),
                    hirari_core_bridge::RuntimeHealthState::Fault
                        | hirari_core_bridge::RuntimeHealthState::Degraded
                ) && ui_timer % 1000 < 32
                {
                    ui.set_audio_diagnostic(health.status_text().into());
                }
                if health.sandbox_failures > 0 && ui_timer % 1000 < 32 {
                    let selected = clamp_selection_index(ui.get_sel_idx(), tracks_tele.row_count());
                    let detail = tracks_tele
                        .row_data(selected)
                        .map(|track| core_tele.last_sandbox_failure_text(track.id.max(0) as u32))
                        .unwrap_or_else(|| "track-not-selected".to_owned());
                    ui.set_last_action(
                        format!(
                            "PLUGIN SANDBOX: {} FAILURE(S) · {}",
                            health.sandbox_failures, detail
                        )
                        .into(),
                    );
                }
                if watchdog_trips > 0 {
                    ui.set_last_action(
                        format!("AU WATCHDOG: {} PLUGIN(S) BYPASSED", watchdog_trips).into(),
                    );
                    high_priority_diagnostic = true;
                }
                if ui_timer % 1000 < 32 {
                    let sandbox_snapshots = core_tele.sandbox_snapshots();
                    let mailbox_overruns: u32 = sandbox_snapshots
                        .iter()
                        .map(|snapshot| snapshot.mailbox_overruns)
                        .sum();
                    ui.set_diagnostic_overruns(mailbox_overruns.min(i32::MAX as u32) as i32);
                    ui.set_diagnostic_pdc(ui.get_plugin_pdc_status());
                }
                if !high_priority_diagnostic && ui_timer % 1000 < 32 {
                    let sandbox_snapshots = core_tele.sandbox_snapshots();
                    let quarantined = sandbox_snapshots
                        .iter()
                        .filter(|snapshot| snapshot.is_quarantined())
                        .count();
                    if quarantined > 0 {
                        ui.set_last_action(
                            format!(
                                "PLUGIN SANDBOX: {} QUARANTINED · RECOVERY REQUIRED",
                                quarantined
                            )
                            .into(),
                        );
                    }
                    let mailbox_overruns: u32 = sandbox_snapshots
                        .iter()
                        .map(|snapshot| snapshot.mailbox_overruns)
                        .sum();
                    if quarantined == 0 && mailbox_overruns > 0 {
                        ui.set_last_action(
                            format!("PLUGIN SANDBOX: {} MAILBOX OVERRUN(S)", mailbox_overruns)
                                .into(),
                        );
                    }
                }
                if ui.get_is_rec() && recording_target_tele.borrow().is_some() {
                    if let Err(error) = core_tele.poll_recording_capture() {
                        let targets = recording_target_tele.borrow().clone();
                        let recovered = targets.as_deref().and_then(|targets| {
                            commit_recording_targets(
                                &core_tele,
                                &tracks_tele,
                                targets,
                                last_saved_path_tele.borrow().as_deref(),
                            )
                        });
                        if recovered.is_some() {
                            sync_tracks_from_engine(&tracks_tele, &core_tele);
                        }
                        ui.set_is_rec(false);
                        *recording_target_tele.borrow_mut() = None;
                        ui.set_last_action(if let Some((frames, tracks)) = recovered {
                            format!(
                                "RECORD INPUT ERROR: {error}; recovered {frames} frames on {tracks}"
                            )
                            .into()
                        } else {
                            format!("RECORD INPUT ERROR: {error}; no recoverable take").into()
                        });
                    } else if waveform_due {
                        let waveform = core_tele.recording_capture_waveform(96);
                        if !waveform.is_empty() {
                            gpu_plot_store_tele.publish_waveform(&waveform, 512);
                            ui.set_recording_waveform(slint::ModelRc::new(slint::VecModel::from(
                                waveform,
                            )));
                        }
                    }
                    if core_tele.recording_capture_auto_stop_requested() {
                        let targets = recording_target_tele.borrow().clone();
                        let committed = targets.as_deref().and_then(|targets| {
                            commit_recording_targets(
                                &core_tele,
                                &tracks_tele,
                                targets,
                                last_saved_path_tele.borrow().as_deref(),
                            )
                        });
                        ui.set_is_rec(false);
                        *recording_target_tele.borrow_mut() = None;
                        if let Some((frames, tracks)) = committed {
                            sync_tracks_from_engine(&tracks_tele, &core_tele);
                            ui.set_last_action(
                                format!("PUNCH OUT: {} frames on {}", frames, tracks).into(),
                            );
                        } else {
                            ui.set_last_action("PUNCH OUT: TAKE COULD NOT BE COMMITTED".into());
                        }
                    }
                }
                ui.set_audio_device_ready(audio_ready);
                // Routing and device catalogs are configuration data, not
                // meters. Polling and replacing their models every frame was
                // needlessly invalidating the entire settings surface.
                if environment_elapsed_ms >= 256 {
                    environment_elapsed_ms -= 256;
                    let mut last_clip_end = 0.0f32;
                    for row in 0..tracks_tele.row_count() {
                        if let Some(track) = tracks_tele.row_data(row) {
                            for clip in track.clips.iter() {
                                let end = clip.start_beat + clip.length_beats;
                                if clip.start_beat.is_finite()
                                    && clip.length_beats.is_finite()
                                    && clip.length_beats > 0.0
                                    && end.is_finite()
                                {
                                    last_clip_end = last_clip_end.max(end);
                                }
                            }
                        }
                    }
                    let raw_time_signatures = core_tele.get_time_signature_events();
                    let meter_events = valid_time_signatures(&raw_time_signatures);
                    let timeline_end = arrangement_timeline_end(last_clip_end, &meter_events);
                    if (ui.get_project_len() - timeline_end).abs() > 0.25 {
                        ui.set_project_len(timeline_end);
                    }
                    let (numerator, denominator) = time_signature_at(&meter_events, ui.get_ph());
                    let time_signature_label = format!("{numerator}/{denominator}");
                    if time_signature_label != last_time_signature_label {
                        ui.set_time_sig(time_signature_label.clone().into());
                        last_time_signature_label = time_signature_label;
                    }
                    let measure_starts =
                        arrangement_measure_starts(&raw_time_signatures, timeline_end);
                    if measure_starts != last_arrangement_measure_starts {
                        ui.set_arrangement_measure_starts(slint::ModelRc::new(
                            slint::VecModel::from(measure_starts.clone()),
                        ));
                        last_arrangement_measure_starts = measure_starts;
                    }
                    let route_matrix = active_route_matrix(&core_tele, &tracks_tele);
                    if route_matrix != last_route_matrix {
                        ui.set_active_route_matrix(slint::ModelRc::new(slint::VecModel::from(
                            route_matrix.clone(),
                        )));
                        last_route_matrix = route_matrix;
                    }
                    let send_state = active_send_state(&core_tele, &tracks_tele);
                    if send_state != last_send_state {
                        ui.set_active_send_matrix(slint::ModelRc::new(slint::VecModel::from(
                            send_state.0.clone(),
                        )));
                        ui.set_active_send_gains(slint::ModelRc::new(slint::VecModel::from(
                            send_state.1.clone(),
                        )));
                        ui.set_active_send_pre_matrix(slint::ModelRc::new(slint::VecModel::from(
                            send_state.2.clone(),
                        )));
                        last_send_state = send_state;
                    }
                    let device_catalog = core_tele.list_audio_devices_json();
                    if device_catalog != last_device_catalog {
                        ui.set_audio_device_catalog(device_catalog.clone().into());
                        if let Ok(devices) =
                            serde_json::from_str::<serde_json::Value>(&device_catalog)
                        {
                            let names = devices
                                .as_array()
                                .map(|entries| {
                                    entries
                                        .iter()
                                        .map(|entry| {
                                            let name = entry
                                                .get("name")
                                                .and_then(|name| name.as_str())
                                                .unwrap_or("Unnamed audio device");
                                            let inputs = entry
                                                .get("input_channels")
                                                .and_then(|value| value.as_u64())
                                                .unwrap_or(0);
                                            let outputs = entry
                                                .get("output_channels")
                                                .and_then(|value| value.as_u64())
                                                .unwrap_or(0);
                                            format!("{name} · {inputs} in / {outputs} out")
                                        })
                                        .map(SharedString::from)
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            let query = ui
                                .get_audio_device_query()
                                .to_string()
                                .trim()
                                .to_lowercase();
                            let indices = devices
                                .as_array()
                                .into_iter()
                                .flatten()
                                .enumerate()
                                .filter_map(|(index, entry)| {
                                    let name = entry.get("name")?.as_str()?;
                                    (query.is_empty() || name.to_lowercase().contains(&query))
                                        .then_some(index.min(i32::MAX as usize) as i32)
                                })
                                .collect::<Vec<_>>();
                            ui.set_audio_device_names(slint::ModelRc::new(slint::VecModel::from(
                                names,
                            )));
                            ui.set_audio_device_indices(slint::ModelRc::new(
                                slint::VecModel::from(indices),
                            ));
                        }
                        last_device_catalog = device_catalog;
                    }
                }
                let sample_rate = core_tele.get_sample_rate();
                ui.set_audio_sample_rate(if sample_rate.is_finite() {
                    sample_rate.max(0.0) as f32
                } else {
                    0.0
                });
                ui.set_audio_input_channel_count(
                    core_tele
                        .get_audio_input_channel_count()
                        .min(i32::MAX as u16) as i32,
                );
                let input_uid = core_tele.audio_input_device_uid();
                if input_uid != last_input_endpoint_uid {
                    ui.set_audio_input_channel_names(slint::ModelRc::new(slint::VecModel::from(
                        core_tele
                            .audio_device_input_channel_names(&input_uid)
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<slint::SharedString>>(),
                    )));
                    last_input_endpoint_uid = input_uid;
                }
                let callback_count = core_tele.get_audio_callback_count();
                ui.set_input_dropped_blocks(
                    core_tele.get_dropped_input_blocks().min(i32::MAX as u64) as i32,
                );
                let test_started = tone_test_started_ms_tele.load(Ordering::Acquire);
                if test_started > 0 {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    let elapsed = now_ms.saturating_sub(test_started);
                    if elapsed >= 500 {
                        let baseline = tone_test_baseline_callbacks_tele.load(Ordering::Acquire);
                        let callback_delta = callback_count.saturating_sub(baseline);
                        if callback_delta >= 2 && output_peak.is_finite() && output_peak > 0.001 {
                            ui.set_audio_diagnostic(
                                format!("音声セルフテスト: PASS {:.3}", output_peak).into(),
                            );
                        } else if callback_delta == 0 {
                            ui.set_audio_diagnostic("音声セルフテスト: FAIL (callbackなし)".into());
                        } else {
                            ui.set_audio_diagnostic("音声セルフテスト: FAIL (無音)".into());
                        }
                        tone_test_started_ms_tele.store(u64::MAX, Ordering::Release);
                    }
                }
                if test_started != u64::MAX && ui.get_is_ply() && callback_count > 4 {
                    if output_peak.is_finite() && output_peak > 0.001 {
                        ui.set_audio_diagnostic(format!("音声出力: OK {:.3}", output_peak).into());
                    } else {
                        ui.set_audio_diagnostic("音声コールバック: 無音".into());
                    }
                } else if !audio_ready {
                    ui.set_audio_diagnostic("音声デバイス未接続".into());
                }
                if !audio_ready && ui.get_is_ply() {
                    // Device loss is a transport safety event: stop advancing
                    // the UI and engine until the user explicitly resumes.
                    core_tele.set_playing(false);
                    ui.set_is_ply(false);
                    ui.set_last_action("AUDIO DEVICE OFFLINE: PAUSED".into());
                }
                if !audio_ready && ui.get_is_rec() && recording_target_tele.borrow().is_some() {
                    // Never leave the UI in a recording state after the input
                    // device disappeared. Finalize and publish the last valid
                    // capture so a device fault does not strand a recoverable
                    // take in the temporary spool directory.
                    ui.set_is_rec(false);
                    let _ = core_tele.poll_recording_capture();
                    let targets = recording_target_tele.borrow().clone();
                    let recovered = targets.as_deref().and_then(|targets| {
                        commit_recording_targets(
                            &core_tele,
                            &tracks_tele,
                            targets,
                            last_saved_path_tele.borrow().as_deref(),
                        )
                    });
                    if recovered.is_some() {
                        sync_tracks_from_engine(&tracks_tele, &core_tele);
                    }
                    *recording_target_tele.borrow_mut() = None;
                    ui.set_last_action(if let Some((frames, tracks)) = recovered {
                        format!(
                            "AUDIO DEVICE OFFLINE: RECOVERED {} frames on {}",
                            frames, tracks
                        )
                        .into()
                    } else {
                        "AUDIO DEVICE OFFLINE: RECORDING STOPPED · NO TAKE RECOVERED".into()
                    });
                }
                ui.set_latency_ms(core_tele.get_latency_ms().max(0.0));
                ui.set_buffer_size(core_tele.get_buffer_size().min(i32::MAX as u32) as i32);

                crate::ui::telemetry_render::update_render_telemetry(
                    &ui,
                    &core_tele,
                    &render_started_ms_tele,
                    &render_output_path_tele,
                    &render_lease,
                    &stem_batch_tele,
                );

                // Transport state can change outside the UI callback (device
                // loss, engine stop, or an external host). Always reconcile
                // the UI with the Core state before drawing the playhead.
                let engine_playing = core_tele.is_playing();
                if ui.get_is_ply() != engine_playing {
                    ui.set_is_ply(engine_playing);
                }

                // Tempo and MIDI note models are control-rate data. Replacing
                // Slint models at audio-frame cadence causes a full reactive
                // invalidation even when nothing changed.
                if tempo_elapsed_ms >= 256 {
                    tempo_elapsed_ms -= 256;
                    let tempo_events = core_tele.get_tempo_events();
                    let mut tempo_beats = Vec::with_capacity(tempo_events.len() / 3);
                    let mut tempo_bpms = Vec::with_capacity(tempo_events.len() / 3);
                    for chunk in tempo_events.as_chunks::<3>().0 {
                        if chunk[0].is_finite() && chunk[1].is_finite() {
                            tempo_beats.push(chunk[0] as f32);
                            tempo_bpms.push(chunk[1] as f32);
                        }
                    }
                    if tempo_beats != last_tempo_beats {
                        ui.set_tempo_event_beats(slint::ModelRc::new(slint::VecModel::from(
                            tempo_beats.clone(),
                        )));
                        last_tempo_beats = tempo_beats;
                    }
                    if tempo_bpms != last_tempo_bpms {
                        ui.set_tempo_event_bpms(slint::ModelRc::new(slint::VecModel::from(
                            tempo_bpms.clone(),
                        )));
                        last_tempo_bpms = tempo_bpms;
                    }
                }

                // Keep the GPU piano-roll texture driven by the selected track's
                // actual MIDI notes, rather than an analyzer-shaped placeholder.
                let selected_row = ui.get_selected_row();
                if selected_row != last_piano_row {
                    last_piano_row = selected_row;
                    if let Some(track) = tracks_tele.row_data(selected_row as usize) {
                        let notes: Vec<_> = track.piano_roll_notes.iter().collect();
                        let max_end = notes
                            .iter()
                            .map(|note| note.start_beat + note.length_beats)
                            .fold(1.0_f32, f32::max);
                        let normalized: Vec<[f32; 4]> = notes
                            .iter()
                            .map(|note| {
                                [
                                    (note.start_beat / max_end).clamp(0.0, 1.0),
                                    (note.length_beats / max_end).clamp(0.002, 1.0),
                                    (note.pitch as f32 / 127.0).clamp(0.0, 1.0),
                                    (note.velocity as f32 / 127.0).clamp(0.0, 1.0),
                                ]
                            })
                            .collect();
                        gpu_plot_store_tele.publish_piano_notes(&normalized);
                    }
                }

                if engine_playing {
                    let ph = core_tele.samples_to_beats(core_tele.get_playhead());
                    if ph.is_finite() {
                        ui.set_ph(ph as f32);
                    }

                    if analysis_due {
                        let fft = core_tele.get_fft_bands();
                        if !fft.is_empty() {
                            gpu_plot_store_tele.publish_spectrum(&fft);
                            ui.set_fft_data(slint::ModelRc::new(slint::VecModel::from(fft)));
                        }

                        let colors = core_tele.get_synesthesia_colors();
                        if colors.len() >= 3 {
                            ui.set_synesthesia_color(slint::Color::from_rgb_f32(
                                colors[0], colors[1], colors[2],
                            ));
                        }

                        ui.set_motion_energy(core_tele.get_motion_energy());

                        let partials = core_tele.get_spectral_partials_v();
                        if !partials.is_empty() {
                            ui.set_partials_data(slint::ModelRc::new(slint::VecModel::from(
                                partials,
                            )));
                        }
                    }
                }

                crate::ui::telemetry_video::update_video_preview(
                    &ui,
                    &core_tele,
                    &mut last_video_revision,
                );

                // Read the master meter once and expose it as slot zero. Track
                // peaks follow in model order so mixer strips and the master
                // strip no longer compete for the same telemetry slot.
                let loudness = core_tele.get_master_loudness();
                let count = tracks_tele.row_count() + 1;
                let reset_generation = peak_reset_generation.get();
                if reset_generation != last_peak_reset_generation {
                    peak_holds.fill(0.0);
                    last_peak_reset_generation = reset_generation;
                }
                if peak_holds.len() < count {
                    peak_holds.resize(count, 0.0);
                }

                let track_count = count.saturating_sub(1);
                let mut lp = vec![0.0f32; track_count];
                let mut rp = vec![0.0f32; track_count];
                core_tele.get_all_peaks_l(&mut lp);
                core_tele.get_all_peaks_r(&mut rp);
                let master_peak_l = 10.0f32.powf(loudness.true_peak_l / 20.0).clamp(0.0, 2.0);
                let master_peak_r = 10.0f32.powf(loudness.true_peak_r / 20.0).clamp(0.0, 2.0);
                let mut combined = Vec::with_capacity(count);
                combined.push(master_peak_l.max(master_peak_r));
                combined.extend((0..track_count).map(|i| {
                    lp.get(i)
                        .copied()
                        .unwrap_or(0.0)
                        .max(rp.get(i).copied().unwrap_or(0.0))
                }));

                for i in 0..count {
                    if combined[i] > peak_holds[i] {
                        peak_holds[i] = combined[i];
                    } else {
                        peak_holds[i] = (peak_holds[i] - 0.01).max(0.0);
                    }
                }
                if combined != last_pks {
                    ui.set_pks(slint::ModelRc::new(slint::VecModel::from(combined.clone())));
                    last_pks = combined;
                }
                if peak_holds != last_peak_holds {
                    gpu_plot_store_tele.publish_meters(&peak_holds);
                    ui.set_pks_h(slint::ModelRc::new(slint::VecModel::from(
                        peak_holds.clone(),
                    )));
                    last_peak_holds = peak_holds.clone();
                }

                // 2. Master Precision Telemetry (High Frequency)
                ui.set_lufs_integrated(loudness.integrated);
                ui.set_lufs_short_term(loudness.short_term);
                ui.set_master_true_peak_l(loudness.true_peak_l);
                ui.set_master_true_peak_r(loudness.true_peak_r);
                ui.set_phase_correlation(loudness.correlation);

                crate::ui::telemetry_analysis::update_analysis_telemetry(
                    &ui,
                    &core_tele,
                    &tracks_tele,
                    waveform_due,
                    &mut ev_buffer,
                );
            }
        },
    );
    // A Slint timer stops when its owner is dropped.  This loop is the
    // application's control-rate telemetry source, so keep it alive for the
    // lifetime of the process; otherwise render completion and device/watchdog
    // transitions remain stuck at their initial UI values.
    std::mem::forget(timer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_matrix_is_empty_until_core_reports_connections() {
        let core = HirariCore::new().expect("core must initialize");
        let tracks = slint::VecModel::<Z_Track>::from(Vec::new());
        assert_eq!(active_route_matrix(&core, &tracks), vec![false; 100]);
    }

    #[test]
    fn route_matrix_reflects_a_core_connection_by_track_row() {
        let core = HirariCore::new().expect("core must initialize");
        let source_id = core.add_track(0);
        let destination_id = core.add_track(0);
        assert_ne!(source_id, 0);
        assert_ne!(destination_id, 0);
        assert!(core.set_route(source_id, destination_id, true));

        let tracks = slint::VecModel::<Z_Track>::from(Vec::new());
        crate::ui::track_model::sync_tracks_from_engine_allow_empty(&tracks, &core);
        let matrix = active_route_matrix(&core, &tracks);
        assert_eq!(matrix.len(), 100);
        assert!(matrix[1]);
        assert!(!matrix[0]);
    }
}
