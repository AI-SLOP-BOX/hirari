use aura_core_bridge::stable_api::{CoreApiV1, LoadedMixRenderRequest, WaveContainer};
use aura_core_bridge::AuraCore;
use slint::ComponentHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::slint_ui::{
    default_render_output_path, display_path, ui_error_message, unix_time_millis, AppWindow,
    RenderActions, UiErrorKind,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};

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
    core: Rc<AuraCore>,
    started: Arc<AtomicU64>,
    output_path: Arc<Mutex<std::path::PathBuf>>,
    operation_gate: OperationGate,
    render_lease: Arc<Mutex<Option<crate::ui::operation_gate::OperationLease>>>,
) {
    let render_api = Rc::new(CoreApiV1::from_shared_core(core.clone()));
    let weak = ui.as_weak();
    ui.global::<RenderActions>().on_start_render({
        let weak = weak.clone();
        let render_api = render_api.clone();
        let operation_gate = operation_gate.clone();
        let render_lease = render_lease.clone();
        move |requested_path| {
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
            if render_api
                .queue_loaded_mix(LoadedMixRenderRequest {
                    output_path: output.clone(),
                    container: WaveContainer::Wav,
                })
                .is_ok()
            {
                if let Ok(mut active) = render_lease.lock() {
                    *active = Some(lease);
                }
                started.store(unix_time_millis(), Ordering::Release);
                ui.set_is_rendering(true);
                ui.set_render_progress(0.0);
                ui.set_render_progress_available(false);
                ui.set_render_progress_indeterminate(true);
                ui.set_render_elapsed_seconds(0);
                ui.set_render_state("QUEUED".into());
                ui.set_render_error("WAITING FOR ENGINE STATUS".into());
                ui.set_export_open(false);
                ui.set_last_action(format!("RENDER QUEUED: {}", display_path(&output)).into());
            } else {
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
