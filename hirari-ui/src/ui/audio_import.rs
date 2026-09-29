use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, VecModel};
use std::rc::Rc;
use std::time::Duration;

use crate::slint_ui::{
    display_path, sync_tracks_from_engine, ui_error_message, AppWindow, UiErrorKind, Z_Track,
};

pub(crate) fn import_audio_async(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    track_id: u32,
    path: String,
    beat: f64,
    register_preview: bool,
) {
    let ticket = core.queue_region_import_at_beat(track_id, &path, beat);
    if ticket == 0 {
        ui.set_last_action(
            ui_error_message(
                UiErrorKind::Project,
                "import could not be queued (busy or invalid input)",
            )
            .into(),
        );
        return;
    }

    ui.set_last_action(format!("IMPORTING: {}", display_path(&path)).into());
    poll_import(ui.as_weak(), core, tracks, ticket, path, register_preview);
}

fn poll_import(
    ui: slint::Weak<AppWindow>,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    ticket: u64,
    path: String,
    register_preview: bool,
) {
    slint::Timer::single_shot(Duration::from_millis(50), move || {
        match core.complete_region_import(ticket) {
            0 => poll_import(ui, core, tracks, ticket, path, register_preview),
            1 => {
                if register_preview {
                    let _ = core.register_preview_audio(&path);
                }
                sync_tracks_from_engine(&tracks, &core);
                if let Some(ui) = ui.upgrade() {
                    ui.set_last_action(format!("IMPORTED: {}", display_path(&path)).into());
                }
            }
            _ => {
                if let Some(ui) = ui.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Project,
                            &format!("import failed or project changed: {}", display_path(&path)),
                        )
                        .into(),
                    );
                }
            }
        }
    });
}
