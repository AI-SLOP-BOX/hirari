//! Render progress and completion synchronization.

use crate::slint_ui::*;
use crate::ui::operation_gate::OperationLease;
use aura_core_bridge::AuraCore;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

pub(crate) fn update_render_telemetry(
    ui: &AppWindow,
    core: &Rc<AuraCore>,
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
            let state = snapshot.state;
            // The native renderer publishes the durable WAV before the UI
            // telemetry edge is observed.  Treat a fresh, valid output as a
            // terminal success in that narrow window; otherwise a completed
            // export can remain stuck at QUEUED after a missed poll/timer
            // tick.  Existing files are excluded by the render start time.
            if matches!(state, 1 | 2) && output_is_fresh_and_valid(output_path, started) {
                finish_success(ui, started_ms, output_path, render_lease, elapsed_ms);
                return;
            }
            let progress_available = state <= 5 && snapshot.progress_available;
            ui.set_render_progress_available(progress_available);
            ui.set_render_progress_indeterminate(state == 2 && !progress_available);
            ui.set_render_progress(snapshot.progress);
            ui.set_render_state(bounce_state_label(state).into());

            match state {
                3 => finish_success(ui, started_ms, output_path, render_lease, elapsed_ms),
                4 => finish_error(ui, started_ms, render_lease, elapsed_ms),
                5 => finish_cancelled(ui, started_ms, render_lease, elapsed_ms),
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
    let summary = inspect_rendered_wav(&path);
    !summary.contains("読み込めません")
        && !summary.contains("有効なWAVではありません")
        && !summary.contains("WAV has no audio data")
        && !summary.contains("truncated")
        && !summary.contains("too large")
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
    let summary = inspect_rendered_wav(&output);
    let output_valid = summary
        != "出力ファイルを読み込めません。保存先の権限と空き容量を確認してください"
        && summary
            != "出力ファイルは有効なWAVではありません。別の形式または保存先を確認してください";
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
