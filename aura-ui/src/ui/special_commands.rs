use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::slint_ui::{
    clamp_selection_index, ui_error_message, unix_time_millis, AppWindow, UiErrorKind, Z_Track,
};

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    render_started_ms: &AtomicU64,
) -> bool {
    match command {
        "STEM SEPARATION (AI)" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            if let Some(track) = tracks.row_data(selected) {
                core.execute_vocal_remover(track.id.max(0) as u32);
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Engine, "stem separation failed: no track")
                        .into(),
                );
            }
            true
        }
        "EXPORT ATMOS MASTER" => {
            if core.start_render_async() {
                render_started_ms.store(unix_time_millis(), Ordering::Release);
                ui.set_is_rendering(true);
                ui.set_render_progress(0.0);
                ui.set_render_progress_available(false);
                ui.set_render_progress_indeterminate(true);
                ui.set_render_state("QUEUED".into());
                ui.set_render_error("WAITING FOR ENGINE STATUS".into());
                ui.set_last_action("RENDER QUEUED".into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "busy or failed to queue").into(),
                );
            }
            true
        }
        _ => false,
    }
}
