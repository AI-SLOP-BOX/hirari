//! Main-thread engine telemetry and UI synchronization loop.

use crate::slint_ui::*;
use crate::ui::recovery::summarize_candidates;
use aura_core_bridge::AuraCore;
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[allow(clippy::too_many_arguments)]
pub(crate) fn install_telemetry_loop(
    ui: &AppWindow,
    core: Rc<AuraCore>,
    tracks: Rc<slint::VecModel<Z_Track>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    tone_test_started_ms: Arc<AtomicU64>,
    tone_test_baseline_callbacks: Arc<AtomicU64>,
    render_started_ms: Arc<AtomicU64>,
    render_output_path: Arc<Mutex<std::path::PathBuf>>,
    render_lease: Arc<Mutex<Option<crate::ui::operation_gate::OperationLease>>>,
) {
    let ui_timer_val = Arc::new(AtomicU64::new(0));
    // --- 4. ENGINE TELEMETRY LOOP ---

    let ui_sync = ui.as_weak();
    let core_tele = core.clone();
    let tracks_tele = tracks.clone();
    let last_saved_path_tele = last_saved_path.clone();
    let tone_test_started_ms_tele = tone_test_started_ms.clone();
    let tone_test_baseline_callbacks_tele = tone_test_baseline_callbacks.clone();
    let render_started_ms_tele = render_started_ms.clone();
    let render_output_path_tele = render_output_path.clone();
    let mut peak_holds: Vec<f32> = vec![0.0; 10];
    let mut last_video_revision = 0u64;
    let mut ui_timer = 0;
    let mut analysis_elapsed_ms: u32 = 0;
    let mut waveform_elapsed_ms: u32 = 0;
    let mut plugin_elapsed_ms: u32 = 0;
    let mut pdc_elapsed_ms: u32 = 0;
    let mut last_plugin_context = (-1_i32, -1_i32);
    let mut last_pdc_context = (-1_i32, -1_i32);
    let mut last_plugin_values: Vec<f32> = Vec::new();
    let mut watchdog_total: u32 = 0;
    let mut ev_buffer = Vec::with_capacity(128);
    let timer = slint::Timer::default();

    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(16),
        move || {
            if let Some(ui) = ui_sync.upgrade() {
                ui_timer += 16;
                analysis_elapsed_ms = analysis_elapsed_ms.saturating_add(16);
                waveform_elapsed_ms = waveform_elapsed_ms.saturating_add(16);
                plugin_elapsed_ms = plugin_elapsed_ms.saturating_add(16);
                pdc_elapsed_ms = pdc_elapsed_ms.saturating_add(16);
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
                ui.set_ui_timer(ui_timer as f32);
                ui.set_undo_depth(core_tele.undo_depth() as i32);
                ui.set_redo_depth(core_tele.redo_depth() as i32);

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

                if ui_timer % 1000 < 16 {
                    let recovery_summary = last_saved_path_tele
                        .borrow()
                        .as_deref()
                        .map(|path| summarize_candidates(&core_tele.recovery_candidates_json(path)))
                        .unwrap_or_default();
                    ui.set_recovery_backup_count(recovery_summary.count);
                    ui.set_recovery_backup_generations(slint::ModelRc::new(slint::VecModel::from(
                        recovery_summary.generations,
                    )));
                    ui.set_recovery_backup_summary(recovery_summary.text.into());
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
                if !was_audio_ready && ui_timer % 1000 < 16 {
                    core_tele.try_reconnect_audio_device();
                    health = core_tele.runtime_health_snapshot();
                    if health.audio_device_ready {
                        ui.set_last_action("AUDIO DEVICE RECONNECTED · TRANSPORT READY".into());
                    }
                }
                let audio_ready = health.audio_device_ready;
                let output_peak = health.peak_left.max(health.peak_right);
                if ui_timer % 1000 < 16 && health.audio_driver_status != "running" {
                    ui.set_audio_diagnostic(health.status_text().into());
                }
                if matches!(
                    health.state(),
                    aura_core_bridge::RuntimeHealthState::Fault
                        | aura_core_bridge::RuntimeHealthState::Degraded
                ) && ui_timer % 1000 < 16
                {
                    ui.set_audio_diagnostic(health.status_text().into());
                }
                if health.sandbox_failures > 0 && ui_timer % 1000 < 16 {
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
                if ui_timer % 1000 < 16 {
                    let sandbox_snapshots = core_tele.sandbox_snapshots();
                    let mailbox_overruns: u32 = sandbox_snapshots
                        .iter()
                        .map(|snapshot| snapshot.mailbox_overruns)
                        .sum();
                    ui.set_diagnostic_overruns(mailbox_overruns.min(i32::MAX as u32) as i32);
                    ui.set_diagnostic_pdc(ui.get_plugin_pdc_status());
                }
                if !high_priority_diagnostic && ui_timer % 1000 < 16 {
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
                if ui.get_is_rec() {
                    if let Err(error) = core_tele.poll_recording_capture() {
                        let selected =
                            clamp_selection_index(ui.get_sel_idx(), tracks_tele.row_count());
                        let target_row = if tracks_tele
                            .row_data(selected)
                            .map(|track| track.armed)
                            .unwrap_or(false)
                        {
                            Some(selected)
                        } else {
                            (0..tracks_tele.row_count()).find(|&row| {
                                tracks_tele
                                    .row_data(row)
                                    .map(|track| track.armed)
                                    .unwrap_or(false)
                            })
                        };
                        let recovered = target_row
                            .and_then(|row| tracks_tele.row_data(row))
                            .and_then(|track| {
                                core_tele
                                    .commit_recording_capture_to_track(
                                        track.id.max(0) as u32,
                                        last_saved_path_tele.borrow().as_deref(),
                                    )
                                    .ok()
                                    .map(|frames| (track.name, frames))
                            });
                        ui.set_is_rec(false);
                        ui.set_last_action(if let Some((track, frames)) = recovered {
                            format!(
                                "RECORD INPUT ERROR: {error}; recovered {frames} frames on {track}"
                            )
                            .into()
                        } else {
                            format!("RECORD INPUT ERROR: {error}; no recoverable take").into()
                        });
                    } else if waveform_due {
                        let waveform = core_tele.recording_capture_waveform(96);
                        if !waveform.is_empty() {
                            ui.set_recording_waveform(slint::ModelRc::new(slint::VecModel::from(
                                waveform,
                            )));
                        }
                    }
                    if core_tele.recording_capture_auto_stop_requested() {
                        let selected =
                            clamp_selection_index(ui.get_sel_idx(), tracks_tele.row_count());
                        let target_row = if tracks_tele
                            .row_data(selected)
                            .map(|track| track.armed)
                            .unwrap_or(false)
                        {
                            Some(selected)
                        } else {
                            (0..tracks_tele.row_count()).find(|&row| {
                                tracks_tele
                                    .row_data(row)
                                    .map(|track| track.armed)
                                    .unwrap_or(false)
                            })
                        };
                        let committed = target_row
                            .and_then(|row| tracks_tele.row_data(row))
                            .and_then(|track| {
                                core_tele
                                    .commit_recording_capture_to_track(
                                        track.id.max(0) as u32,
                                        last_saved_path_tele.borrow().as_deref(),
                                    )
                                    .ok()
                                    .map(|frames| (track.name, frames))
                            });
                        ui.set_is_rec(false);
                        if let Some((track, frames)) = committed {
                            sync_tracks_from_engine(&tracks_tele, &core_tele);
                            ui.set_last_action(
                                format!("PUNCH OUT: {} frames on {}", frames, track).into(),
                            );
                        } else {
                            ui.set_last_action("PUNCH OUT: TAKE COULD NOT BE COMMITTED".into());
                        }
                    }
                }
                ui.set_audio_device_ready(audio_ready);
                let sample_rate = core_tele.get_sample_rate();
                ui.set_audio_sample_rate(if sample_rate.is_finite() {
                    sample_rate.max(0.0) as f32
                } else {
                    0.0
                });
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
                if !audio_ready && ui.get_is_rec() {
                    // Never leave the UI in a recording state after the input
                    // device disappeared. Finalize and publish the last valid
                    // capture so a device fault does not strand a recoverable
                    // take in the temporary spool directory.
                    ui.set_is_rec(false);
                    let _ = core_tele.poll_recording_capture();
                    let selected = clamp_selection_index(ui.get_sel_idx(), tracks_tele.row_count());
                    let target_row = if tracks_tele
                        .row_data(selected)
                        .map(|track| track.armed)
                        .unwrap_or(false)
                    {
                        Some(selected)
                    } else {
                        (0..tracks_tele.row_count()).find(|&row| {
                            tracks_tele
                                .row_data(row)
                                .map(|track| track.armed)
                                .unwrap_or(false)
                        })
                    };
                    let recovered = target_row
                        .and_then(|row| tracks_tele.row_data(row))
                        .and_then(|track| {
                            core_tele
                                .commit_recording_capture_to_track(
                                    track.id.max(0) as u32,
                                    last_saved_path_tele.borrow().as_deref(),
                                )
                                .ok()
                                .map(|frames| (track.name, frames))
                        });
                    if recovered.is_some() {
                        sync_tracks_from_engine(&tracks_tele, &core_tele);
                    }
                    ui.set_last_action(if let Some((track, frames)) = recovered {
                        format!(
                            "AUDIO DEVICE OFFLINE: RECOVERED {} frames on {}",
                            frames, track
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
                );

                // Transport state can change outside the UI callback (device
                // loss, engine stop, or an external host). Always reconcile
                // the UI with the Core state before drawing the playhead.
                let engine_playing = core_tele.is_playing();
                if ui.get_is_ply() != engine_playing {
                    ui.set_is_ply(engine_playing);
                }

                // Tempo-map inspection is useful while stopped as well as
                // during playback, so keep this control-rate snapshot outside
                // the transport branch.
                let tempo_events = core_tele.get_tempo_events();
                let mut tempo_beats = Vec::with_capacity(tempo_events.len() / 3);
                let mut tempo_bpms = Vec::with_capacity(tempo_events.len() / 3);
                for chunk in tempo_events.as_chunks::<3>().0 {
                    if chunk[0].is_finite() && chunk[1].is_finite() {
                        tempo_beats.push(chunk[0] as f32);
                        tempo_bpms.push(chunk[1] as f32);
                    }
                }
                ui.set_tempo_event_beats(slint::ModelRc::new(slint::VecModel::from(tempo_beats)));
                ui.set_tempo_event_bpms(slint::ModelRc::new(slint::VecModel::from(tempo_bpms)));

                if engine_playing {
                    let ph = core_tele.samples_to_beats(core_tele.get_playhead());
                    if ph.is_finite() {
                        ui.set_ph(ph as f32);
                    }

                    if analysis_due {
                        let fft = core_tele.get_fft_bands();
                        if !fft.is_empty() {
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

                let count = tracks_tele.row_count() + 1;
                if peak_holds.len() < count {
                    peak_holds.resize(count, 0.0);
                }

                let mut lp = vec![0.0f32; count];
                let mut rp = vec![0.0f32; count];
                core_tele.get_all_peaks_l(&mut lp);
                core_tele.get_all_peaks_r(&mut rp);
                let combined: Vec<f32> = (0..count)
                    .map(|i| {
                        lp.get(i)
                            .copied()
                            .unwrap_or(0.0)
                            .max(rp.get(i).copied().unwrap_or(0.0))
                    })
                    .collect();

                for i in 0..count {
                    if combined[i] > peak_holds[i] {
                        peak_holds[i] = combined[i];
                    } else {
                        peak_holds[i] = (peak_holds[i] - 0.01).max(0.0);
                    }
                }
                ui.set_pks(slint::ModelRc::new(slint::VecModel::from(combined)));
                ui.set_pks_h(slint::ModelRc::new(slint::VecModel::from(
                    peak_holds.clone(),
                )));

                // 2. Master Precision Telemetry (High Frequency)
                let loudness = core_tele.get_master_loudness();
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
