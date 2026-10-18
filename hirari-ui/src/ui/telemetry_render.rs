//! Render progress and completion synchronization.

use crate::slint_ui::*;
use crate::ui::operation_gate::OperationLease;
use crate::ui::render::{stem_step, StemStep};
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
                    output_path,
                    render_lease,
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

/// Drives the core-owned stems batch from the telemetry timer.
///
/// Core decides whether a stem published (`settle`) and which track comes next
/// (`next`); this only feeds the engine's terminal state back to it and kicks
/// the following render, which is the one thing a GUI can do that the CLI's
/// blocking loop does for itself.
fn update_stem_export(
    ui: &AppWindow,
    core: &HirariCore,
    started_ms: &Arc<AtomicU64>,
    output_path: &Arc<Mutex<PathBuf>>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    snapshot: hirari_core_bridge::BounceSnapshot,
    elapsed_ms: u64,
) {
    let Some((rendered, total)) = core.stem_batch_progress() else {
        finish_stem_failure(
            ui,
            core,
            started_ms,
            render_lease,
            "stem export queue is missing",
            elapsed_ms,
        );
        return;
    };

    let overall_progress =
        (rendered as f32 + snapshot.progress.clamp(0.0, 1.0)) / total.max(1) as f32;
    ui.set_render_progress_available(snapshot.progress_available);
    ui.set_render_progress_indeterminate(snapshot.state == 2 && !snapshot.progress_available);
    ui.set_render_progress(overall_progress.clamp(0.0, 1.0));
    ui.set_render_state(bounce_state_label(snapshot.state).into());

    match snapshot.state {
        3 => {
            // The engine published the armed stem. Core validates the WAV and
            // either advances the queue or rolls the whole batch back.
            let step = stem_step(&core.stem_batch_settle(true));
            advance_stem_export(
                ui,
                core,
                started_ms,
                output_path,
                render_lease,
                step,
                elapsed_ms,
            );
        }
        4 => {
            // `settle(false)` rolls the batch back and clears the render-only
            // target, so a failed export leaves no partial set of stems.
            let reason = match stem_step(&core.stem_batch_settle(false)) {
                StemStep::Failed(reason) => reason,
                _ => "native stem render failed".to_owned(),
            };
            finish_stem_failure(ui, core, started_ms, render_lease, &reason, elapsed_ms);
        }
        5 => {
            core.stem_batch_abort();
            finish_stem_cancelled(ui, core, started_ms, render_lease, elapsed_ms);
        }
        _ => {}
    }
}

/// Reacts to what core made of the stem that just finished: publish the next
/// one, or end the export. Every path that cannot continue goes through
/// `finish_stem_failure`, which reconciles the UI once core has rolled back.
fn advance_stem_export(
    ui: &AppWindow,
    core: &HirariCore,
    started_ms: &Arc<AtomicU64>,
    output_path: &Arc<Mutex<PathBuf>>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    step: StemStep,
    elapsed_ms: u64,
) {
    match step {
        StemStep::Complete { outputs, format } => {
            let output_dir = output_path
                .lock()
                .map(|path| path.clone())
                .unwrap_or_default();
            finish_stem_success(
                ui,
                started_ms,
                render_lease,
                &output_dir,
                outputs.len(),
                format,
                elapsed_ms,
            );
        }
        StemStep::StemDone { rendered, total } => {
            let queued = match stem_step(&core.stem_batch_next()) {
                StemStep::Ready { output, format, .. } => {
                    crate::ui::render::kick_stem(core, &output, format)
                }
                StemStep::Failed(reason) => {
                    finish_stem_failure(ui, core, started_ms, render_lease, &reason, elapsed_ms);
                    return;
                }
                // Core owns the queue, so `next` can only report the target
                // `settle` just advanced past. Anything else means the queue
                // is gone, and continuing would re-render the last stem.
                _ => {
                    core.stem_batch_abort();
                    finish_stem_failure(
                        ui,
                        core,
                        started_ms,
                        render_lease,
                        "stem export queue is invalid",
                        elapsed_ms,
                    );
                    return;
                }
            };
            if !queued {
                core.stem_batch_abort();
                finish_stem_failure(
                    ui,
                    core,
                    started_ms,
                    render_lease,
                    &format!("could not queue stem {} of {total}", rendered + 1),
                    elapsed_ms,
                );
                return;
            }
            ui.set_render_progress(rendered as f32 / total.max(1) as f32);
            ui.set_render_state("QUEUED".into());
            ui.set_render_error(
                format!("STEM {rendered} OF {total} COMPLETE · QUEUING NEXT").into(),
            );
            ui.set_last_action(format!("STEM {rendered} OF {total} EXPORTED").into());
        }
        StemStep::Failed(reason) => {
            finish_stem_failure(ui, core, started_ms, render_lease, &reason, elapsed_ms);
        }
        // `settle` reports `ready` only for a target it armed itself, which
        // this timer never observes. Fail loudly instead of pretending the
        // export is still moving.
        StemStep::Ready { .. } => {
            core.stem_batch_abort();
            finish_stem_failure(
                ui,
                core,
                started_ms,
                render_lease,
                "stem export reported an unexpected state",
                elapsed_ms,
            );
        }
    }
}

fn finish_stem_success(
    ui: &AppWindow,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    output_dir: &PathBuf,
    count: usize,
    format: u32,
    elapsed_ms: u64,
) {
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

/// Reconciles the UI after a stems export stopped. By the time a caller gets
/// here core has already rolled the batch back and removed what it published;
/// the extra `clear_offline_render_target` covers the paths where core had no
/// batch to roll back in the first place.
fn finish_stem_failure(
    ui: &AppWindow,
    core: &HirariCore,
    started_ms: &Arc<AtomicU64>,
    render_lease: &Arc<Mutex<Option<OperationLease>>>,
    reason: &str,
    elapsed_ms: u64,
) {
    core.clear_offline_render_target();
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
    elapsed_ms: u64,
) {
    // `stem_batch_abort` already removed what the batch published; this
    // clears the render-only target for the paths that had no batch left.
    core.clear_offline_render_target();
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
