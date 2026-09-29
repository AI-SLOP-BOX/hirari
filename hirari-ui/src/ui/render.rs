use hirari_core_bridge::stable_api::{CoreApiV1, LoadedMixRenderRequest, WaveContainer};
use hirari_core_bridge::HirariCore;
use slint::ComponentHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::slint_ui::{
    default_render_output_path, display_path, ui_error_message, unix_time_millis, AppWindow,
    RenderActions, UiErrorKind,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct StemExportJob {
    pub(crate) track_id: u32,
    pub(crate) output: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct StemExportBatch {
    pub(crate) output_dir: PathBuf,
    pub(crate) format: u32,
    pub(crate) jobs: Vec<StemExportJob>,
    pub(crate) current: usize,
    pub(crate) started: usize,
}

fn safe_stem_name(name: &str, track_id: u32, used: &mut HashSet<String>) -> String {
    let base = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let base = if base.is_empty() {
        format!("track-{track_id}")
    } else {
        base
    };
    let mut suffix = 1u32;
    loop {
        let stem = if suffix == 1 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        // Windows reserves these device names even when they have an
        // extension. Prefix them so tracks named "CON" or "COM1" export.
        let candidate = if matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            format!("track-{stem}")
        } else {
            stem
        };
        // The destination commonly lives on a case-insensitive filesystem
        // (including default macOS volumes). Match collisions the way the
        // file system will, not by Rust's case-sensitive `String` equality.
        if used.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

pub(crate) fn queue_stem(core: &HirariCore, job: &StemExportJob, format: u32) -> bool {
    if job.output.exists() || !core.set_offline_render_target(job.track_id) {
        return false;
    }
    core.start_render_async_to_format(job.output.to_string_lossy().as_ref(), format)
}

pub(crate) fn render_format_label(format: u32) -> &'static str {
    match format {
        1 => "WAVE64 Float32",
        2 => "Float32 WAV",
        3 => "PCM24 WAV",
        _ => "PCM16 WAV",
    }
}

pub(crate) fn render_tail_seconds(index: i32) -> f32 {
    [0.0, 1.0, 2.0, 5.0, 10.0, 30.0]
        .get(index.clamp(0, 5) as usize)
        .copied()
        .unwrap_or(2.0)
}

pub(crate) fn cancellation_status(accepted: bool) -> (&'static str, &'static str, &'static str) {
    if accepted {
        (
            "CANCEL_REQUESTED",
            "CANCELLATION REQUESTED",
            "RENDER CANCELLATION REQUESTED",
        )
    } else {
        (
            "CANCEL_REJECTED",
            "CANCELLATION REJECTED · RENDER CONTINUES",
            "RENDER CANCELLATION REJECTED",
        )
    }
}

/// Render queue and cancellation bindings. Progress telemetry is handled by
/// the existing timer; this module owns only commands that change render state.
pub(crate) fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    started: Arc<AtomicU64>,
    output_path: Arc<Mutex<std::path::PathBuf>>,
    operation_gate: OperationGate,
    render_lease: Arc<Mutex<Option<crate::ui::operation_gate::OperationLease>>>,
    stem_batch: Arc<Mutex<Option<StemExportBatch>>>,
) {
    let render_api = Rc::new(CoreApiV1::from_shared_core(core.clone()));
    let weak = ui.as_weak();
    ui.global::<RenderActions>().on_start_render({
        let weak = weak.clone();
        let render_api = render_api.clone();
        let core = core.clone();
        let started = started.clone();
        let output_path = output_path.clone();
        let operation_gate = operation_gate.clone();
        let render_lease = render_lease.clone();
        let stem_batch = stem_batch.clone();
        move |requested_path, requested_format| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_is_rendering() {
                ui.set_last_action("RENDER ALREADY RUNNING".into());
                return;
            }
            let Some(lease) = operation_gate.try_enter(OperationKind::Render) else {
                ui.set_last_action("PROJECT BUSY: RENDER IGNORED".into());
                return;
            };
            let output = if requested_path.is_empty() {
                default_render_output_path()
            } else {
                std::path::PathBuf::from(requested_path.as_str())
            };
            if let Ok(mut current) = output_path.lock() {
                *current = output.clone();
            }
            let container = match requested_format {
                1 => WaveContainer::WavPcm24,
                2 => WaveContainer::WavFloat32,
                3 => WaveContainer::Wave64,
                _ => WaveContainer::Wav,
            };
            core.set_offline_render_tail_seconds(render_tail_seconds(
                ui.global::<RenderActions>().get_render_tail_index(),
            ));
            if render_api
                .queue_loaded_mix(LoadedMixRenderRequest {
                    output_path: output.clone(),
                    container,
                })
                .is_ok()
            {
                if let Ok(mut batch) = stem_batch.lock() {
                    *batch = None;
                }
                if let Ok(mut active) = render_lease.lock() {
                    *active = Some(lease);
                }
                started.store(unix_time_millis(), Ordering::Release);
                ui.set_is_rendering(true);
                ui.global::<RenderActions>().set_stem_export_mode(false);
                ui.set_render_progress(0.0);
                ui.set_render_progress_available(false);
                ui.set_render_progress_indeterminate(true);
                ui.set_render_elapsed_seconds(0);
                ui.set_render_state("QUEUED".into());
                ui.set_render_error("WAITING FOR ENGINE STATUS".into());
                ui.set_export_open(false);
                ui.set_last_action(format!("RENDER QUEUED: {}", display_path(&output)).into());
            } else {
                core.set_offline_render_tail_seconds(0.0);
                started.store(0, Ordering::Release);
                ui.set_is_rendering(false);
                ui.set_render_state("FAILED_TO_QUEUE".into());
                ui.set_render_error(
                    ui_error_message(UiErrorKind::Render, "engine busy or render could not start")
                        .into(),
                );
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "could not queue render").into(),
                );
            }
        }
    });

    ui.global::<RenderActions>().on_start_stems({
        let weak = weak.clone();
        let core = core.clone();
        let started = started.clone();
        let output_path = output_path.clone();
        let operation_gate = operation_gate.clone();
        let render_lease = render_lease.clone();
        let stem_batch = stem_batch.clone();
        move |requested_dir, jobs_json| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_is_rendering() {
                ui.set_last_action("RENDER ALREADY RUNNING".into());
                return;
            }
            let output_dir = PathBuf::from(requested_dir.as_str());
            if !output_dir.is_dir() {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "stem output folder is unavailable")
                        .into(),
                );
                return;
            }
            let requested = match serde_json::from_str::<Vec<(u32, String)>>(jobs_json.as_str()) {
                Ok(jobs) if !jobs.is_empty() => jobs,
                _ => {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Render, "there are no tracks to export")
                            .into(),
                    );
                    return;
                }
            };
            let mut seen_ids = HashSet::with_capacity(requested.len());
            let mut used_names = HashSet::with_capacity(requested.len());
            let mut jobs = Vec::with_capacity(requested.len());
            for (track_id, name) in requested {
                if track_id == 0 || !seen_ids.insert(track_id) {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Render, "invalid stem track selection")
                            .into(),
                    );
                    return;
                }
                let filename = safe_stem_name(&name, track_id, &mut used_names);
                let output = output_dir.join(format!("{filename}.wav"));
                if output.exists() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Render,
                            &format!("stem output already exists: {}", output.display()),
                        )
                        .into(),
                    );
                    return;
                }
                jobs.push(StemExportJob { track_id, output });
            }
            let Some(lease) = operation_gate.try_enter(OperationKind::Render) else {
                ui.set_last_action("PROJECT BUSY: STEM EXPORT IGNORED".into());
                return;
            };
            let selected_format = ui.global::<RenderActions>().get_render_format();
            let format = match selected_format {
                1 => 3,
                2 => 2,
                3 => 1,
                _ => 0,
            };
            core.set_offline_render_tail_seconds(render_tail_seconds(
                ui.global::<RenderActions>().get_render_tail_index(),
            ));
            core.set_offline_render_options(false, true);
            let batch = StemExportBatch {
                output_dir: output_dir.clone(),
                format,
                jobs,
                current: 0,
                started: 0,
            };
            let track_count = batch.jobs.len();
            let first_job = batch.jobs[0].clone();
            if !core.set_offline_render_target(first_job.track_id) {
                core.clear_offline_render_target();
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "first stem target is unavailable")
                        .into(),
                );
                return;
            }
            if !core
                .start_render_async_to_format(first_job.output.to_string_lossy().as_ref(), format)
            {
                core.clear_offline_render_target();
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "stem render could not be queued").into(),
                );
                return;
            }
            if let Ok(mut active_batch) = stem_batch.lock() {
                *active_batch = Some(StemExportBatch {
                    started: 1,
                    ..batch
                });
            }
            if let Ok(mut active) = render_lease.lock() {
                *active = Some(lease);
            }
            if let Ok(mut current) = output_path.lock() {
                *current = output_dir.clone();
            }
            started.store(unix_time_millis(), Ordering::Release);
            ui.set_is_rendering(true);
            ui.global::<RenderActions>().set_stem_export_mode(true);
            ui.set_render_progress(0.0);
            ui.set_render_progress_available(true);
            ui.set_render_progress_indeterminate(false);
            ui.set_render_elapsed_seconds(0);
            ui.set_render_state("QUEUED".into());
            ui.set_render_error("PREPARING STEM EXPORT".into());
            ui.set_export_open(false);
            ui.set_last_action(
                format!(
                    "STEM EXPORT QUEUED: {track_count} tracks · {}",
                    display_path(&output_dir)
                )
                .into(),
            );
        }
    });

    ui.global::<RenderActions>().on_cancel_render({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let (state, error, action) = cancellation_status(core.cancel_render());
                ui.set_render_state(state.into());
                ui.set_render_error(error.into());
                ui.set_last_action(action.into());
                if state == "CANCEL_REJECTED" {
                    // Keep the render state active: a rejected cancellation
                    // must never make the UI claim that rendering stopped.
                    ui.set_is_rendering(true);
                    ui.set_render_progress_indeterminate(true);
                }
            }
        }
    });

    ui.global::<RenderActions>().on_pause_render({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                if core.pause_render() {
                    ui.set_render_state("PAUSED".into());
                    ui.set_render_error("RENDER PAUSED · CHECKPOINT RETAINED".into());
                    ui.set_last_action("RENDER PAUSED".into());
                } else {
                    ui.set_last_action("PAUSE REJECTED · RENDER NOT ACTIVE".into());
                }
            }
        }
    });

    ui.global::<RenderActions>().on_resume_render({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                if core.resume_render() {
                    ui.set_render_state("RENDERING".into());
                    ui.set_render_error("RENDER RESUMED".into());
                    ui.set_last_action("RENDER RESUMED".into());
                } else {
                    ui.set_last_action("RESUME REJECTED · RENDER NOT PAUSED".into());
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::cancellation_status;

    #[test]
    fn cancellation_status_preserves_running_state_when_rejected() {
        assert_eq!(
            cancellation_status(true),
            (
                "CANCEL_REQUESTED",
                "CANCELLATION REQUESTED",
                "RENDER CANCELLATION REQUESTED"
            )
        );
        assert_eq!(
            cancellation_status(false),
            (
                "CANCEL_REJECTED",
                "CANCELLATION REJECTED · RENDER CONTINUES",
                "RENDER CANCELLATION REJECTED"
            )
        );
    }
}
