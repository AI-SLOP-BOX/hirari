//! Render progress and completion synchronization.

use crate::slint_ui::*;
use crate::ui::operation_gate::OperationLease;
use crate::ui::render::StemExportBatch;
use hirari_core_bridge::HirariCore;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

pub(crate) fn update_render_telemetry(
    ui: &AppWindow,
    core: &Rc<HirariCore>,
    started_ms: &Arc<AtomicU64>,
    output_path: &Arc<Mutex<PathBuf>>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    stem_batch: &Arc<Mutex<Option<StemExportBatch>>>,
) {
    if !ui.get_is_rendering() {
        return;
    }

    let started = started_ms.load(Ordering::Acquire);
    let elapsed_ms = if started > 0 {
        unix_time_millis().saturating_sub(started)
    } else {
        0
    };
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);

    match core.bounce_snapshot() {
        Some(snapshot) => {
            if ui.global::<RenderActions>().get_stem_export_mode() {
                update_stem_export(
                    ui,
                    core,
                    started_ms,
                    render_lease,
                    stem_batch,
                    snapshot,
                    elapsed_ms,
                );
                return;
            }
            let state = snapshot.state;
            // The native renderer publishes the durable WAV before the UI
            // telemetry edge is observed.  Treat a fresh, valid output as a
            // terminal success in that narrow window; otherwise a completed
            // export can remain stuck at QUEUED after a missed poll/timer
            // tick.  Existing files are excluded by the render start time.
            if matches!(state, 1 | 2) && output_is_fresh_and_valid(output_path, started) {
                core.set_offline_render_tail_seconds(0.0);
                finish_success(ui, started_ms, output_path, render_lease, elapsed_ms);
                return;
            }
            let progress_available = state <= 6 && snapshot.progress_available;
            ui.set_render_progress_available(progress_available);
            ui.set_render_progress_indeterminate(state == 2 && !progress_available);
            ui.set_render_progress(snapshot.progress);
            ui.set_render_state(bounce_state_label(state).into());

            match state {
                3 => {
                    core.set_offline_render_tail_seconds(0.0);
                    finish_success(ui, started_ms, output_path, render_lease, elapsed_ms)
                }
                4 => {
                    core.set_offline_render_tail_seconds(0.0);
                    finish_error(ui, started_ms, render_lease, elapsed_ms)
                }
                5 => {
                    core.set_offline_render_tail_seconds(0.0);
                    finish_cancelled(ui, started_ms, render_lease, elapsed_ms)
                }
                _ => {}
            }
        }
        None => {
            ui.set_render_progress_available(false);
            ui.set_render_progress_indeterminate(true);
            ui.set_render_state("RENDERING".into());
            ui.set_render_progress(0.0);
            ui.set_render_error(render_progress_status(2, false, 0.0, elapsed_ms / 1000).into());
        }
    }
}

fn update_stem_export(
    ui: &AppWindow,
    core: &HirariCore,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    stem_batch: &Arc<Mutex<Option<StemExportBatch>>>,
    snapshot: hirari_core_bridge::BounceSnapshot,
    elapsed_ms: u64,
) {
    let Ok(mut state) = stem_batch.lock() else {
        finish_stem_failure(
            ui,
            core,
            started_ms,
            render_lease,
            stem_batch,
            "stem export state is unavailable",
            elapsed_ms,
        );
        return;
    };
    if state.is_none() {
        drop(state);
        finish_stem_failure(
            ui,
            core,
            started_ms,
            render_lease,
            stem_batch,
            "stem export queue is missing",
            elapsed_ms,
        );
        return;
    }
    let Some(batch) = state.as_mut() else {
        return;
    };
    let total = batch.jobs.len();
    let current = batch.current;
    let output_dir = batch.output_dir.clone();
    let format = batch.format;
    let output = batch.jobs.get(current).map(|job| job.output.clone());
    if total == 0 || current >= total || output.is_none() {
        drop(state);
        finish_stem_failure(
            ui,
            core,
            started_ms,
            render_lease,
            stem_batch,
            "stem export queue is invalid",
            elapsed_ms,
        );
        return;
    }

    let overall_progress = (current as f32 + snapshot.progress.clamp(0.0, 1.0)) / total as f32;
    ui.set_render_progress_available(snapshot.progress_available);
    ui.set_render_progress_indeterminate(snapshot.state == 2 && !snapshot.progress_available);
    ui.set_render_progress(overall_progress.clamp(0.0, 1.0));
    ui.set_render_state(bounce_state_label(snapshot.state).into());

    match snapshot.state {
        3 => {
            let output = output.expect("validated above");
            if !core.validate_render_output(output.to_string_lossy().as_ref(), false) {
                drop(state);
                finish_stem_failure(
                    ui,
                    core,
                    started_ms,
                    render_lease,
                    stem_batch,
                    &format!("rendered stem failed WAV validation: {}", output.display()),
                    elapsed_ms,
                );
                return;
            }
            batch.current += 1;
            if batch.current < total {
                let next = batch.jobs[batch.current].clone();
                if crate::ui::render::queue_stem(core, &next, format) {
                    batch.started = batch.current + 1;
                    ui.set_render_progress(batch.current as f32 / total as f32);
                    ui.set_render_state("QUEUED".into());
                    ui.set_render_error(
                        format!(
                            "STEM {} OF {} COMPLETE · QUEUING NEXT",
                            batch.current, total
                        )
                        .into(),
                    );
                    ui.set_last_action(
                        format!("STEM {} OF {} EXPORTED", batch.current, total).into(),
                    );
                    return;
                }
                let failed_index = batch.current + 1;
                drop(state);
                finish_stem_failure(
                    ui,
                    core,
                    started_ms,
                    render_lease,
                    stem_batch,
                    &format!("could not queue stem {failed_index} of {total}"),
                    elapsed_ms,
                );
                return;
            }
            drop(state);
            core.clear_offline_render_target();
            finish_stem_success(
                ui,
                started_ms,
                render_lease,
                stem_batch,
                total,
                &output_dir,
                format,
                elapsed_ms,
            );
        }
        4 => {
            let failed_track = batch.jobs[current].track_id;
            drop(state);
            finish_stem_failure(
                ui,
                core,
                started_ms,
                render_lease,
                stem_batch,
                &format!("native stem render failed for track {failed_track}"),
                elapsed_ms,
            );
        }
        5 => {
            drop(state);
            finish_stem_cancelled(ui, core, started_ms, render_lease, stem_batch, elapsed_ms);
        }
        _ => {}
    }
}

fn remove_started_stems(batch: &StemExportBatch) {
    for job in batch.jobs.iter().take(batch.started) {
        if let Err(error) = std::fs::remove_file(&job.output) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Could not remove partial stem {}: {error}",
                    job.output.display()
                );
            }
        }
    }
}

fn finish_stem_success(
    ui: &AppWindow,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    stem_batch: &Arc<Mutex<Option<StemExportBatch>>>,
    count: usize,
    output_dir: &PathBuf,
    format: u32,
    elapsed_ms: u64,
) {
    if let Ok(mut state) = stem_batch.lock() {
        state.take();
    }
    ui.set_is_rendering(false);
    ui.set_render_progress(1.0);
    ui.set_render_progress_available(true);
    ui.set_render_progress_indeterminate(false);
    ui.set_render_state("COMPLETE".into());
    ui.set_render_error("STEM EXPORT COMPLETE".into());
    ui.set_render_summary(
        format!(
            "{count} {} stems · {}",
            crate::ui::render::render_format_label(format),
            display_path(output_dir)
        )
        .into(),
    );
    ui.set_last_action(format!("STEMS EXPORTED: {count} · {}", display_path(output_dir)).into());
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);
    started_ms.store(0, Ordering::Release);
    clear_render_lease(render_lease);
}

fn finish_stem_failure(
    ui: &AppWindow,
    core: &HirariCore,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    stem_batch: &Arc<Mutex<Option<StemExportBatch>>>,
    reason: &str,
    elapsed_ms: u64,
) {
    core.clear_offline_render_target();
    if let Ok(mut state) = stem_batch.lock() {
        if let Some(batch) = state.take() {
            remove_started_stems(&batch);
        }
    }
    ui.set_is_rendering(false);
    ui.global::<RenderActions>().set_stem_export_mode(false);
    ui.set_render_progress_indeterminate(false);
    ui.set_render_state("FAILED".into());
    ui.set_render_error(ui_error_message(UiErrorKind::Render, reason).into());
    ui.set_last_action(
        ui_error_message(
            UiErrorKind::Render,
            "stem export failed; partial files removed",
        )
        .into(),
    );
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);
    started_ms.store(0, Ordering::Release);
    clear_render_lease(render_lease);
}

fn finish_stem_cancelled(
    ui: &AppWindow,
    core: &HirariCore,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    stem_batch: &Arc<Mutex<Option<StemExportBatch>>>,
    elapsed_ms: u64,
) {
    core.clear_offline_render_target();
    if let Ok(mut state) = stem_batch.lock() {
        if let Some(batch) = state.take() {
            remove_started_stems(&batch);
        }
    }
    ui.set_is_rendering(false);
    ui.global::<RenderActions>().set_stem_export_mode(false);
    ui.set_render_progress_indeterminate(false);
    ui.set_render_state("CANCELLED".into());
    ui.set_render_error("STEM EXPORT CANCELLED · PARTIAL FILES REMOVED".into());
    ui.set_last_action("STEM EXPORT CANCELLED".into());
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);
    started_ms.store(0, Ordering::Release);
    clear_render_lease(render_lease);
}

fn output_is_fresh_and_valid(output_path: &Arc<Mutex<PathBuf>>, started_ms: u64) -> bool {
    if started_ms == 0 {
        return false;
    }
    let Ok(path) = output_path.lock().map(|path| path.clone()) else {
        return false;
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(modified_ms) = modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
    else {
        return false;
    };
    // Allow a small clock/timestamp race between the worker publication and
    // the UI recording its start timestamp, but never accept an old export.
    if modified_ms.saturating_add(1_000) < started_ms {
        return false;
    }
    if metadata.len() < 44 {
        return false;
    }
    inspect_rendered_wav_result(&path).is_ok()
}

fn finish_success(
    ui: &AppWindow,
    started_ms: &Arc<AtomicU64>,
    output_path: &Arc<Mutex<PathBuf>>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    elapsed_ms: u64,
) {
    ui.set_is_rendering(false);
    ui.set_render_progress_indeterminate(false);
    let output = output_path
        .lock()
        .map(|path| path.clone())
        .unwrap_or_else(|_| default_render_output_path());
    let (summary, output_valid) = match inspect_rendered_wav_result(&output) {
        Ok(summary) => (summary, true),
        Err(error) => (error, false),
    };
    ui.set_render_error(if output_valid {
        "EXPORT COMPLETE".into()
    } else {
        ui_error_with_action(
            UiErrorKind::Render,
            &summary,
            "Choose a writable export path and render again",
        )
        .into()
    });
    ui.set_render_summary(summary.into());
    ui.set_last_action(if output_valid {
        "RENDER COMPLETE".into()
    } else {
        "RENDER FINISHED · OUTPUT VERIFICATION FAILED".into()
    });
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);
    started_ms.store(0, Ordering::Release);
    clear_render_lease(render_lease);
}

fn finish_error(
    ui: &AppWindow,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    elapsed_ms: u64,
) {
    ui.set_is_rendering(false);
    ui.set_render_progress_indeterminate(false);
    ui.set_render_error(ui_error_message(UiErrorKind::Engine, "render failed").into());
    ui.set_last_action(ui_error_message(UiErrorKind::Render, "engine render failed").into());
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);
    started_ms.store(0, Ordering::Release);
    clear_render_lease(render_lease);
}

fn finish_cancelled(
    ui: &AppWindow,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    elapsed_ms: u64,
) {
    ui.set_is_rendering(false);
    ui.set_render_progress_indeterminate(false);
    ui.set_render_error("EXPORT CANCELLED".into());
    ui.set_last_action("RENDER CANCELLED".into());
    ui.set_render_elapsed_seconds((elapsed_ms / 1000).min(i32::MAX as u64) as i32);
    started_ms.store(0, Ordering::Release);
    clear_render_lease(render_lease);
}

fn clear_render_lease(render_lease: &Arc<Mutex<Option<OperationLease>>>) {
    if let Ok(mut lease) = render_lease.lock() {
        lease.take();
    }
}
