use hirari_core_bridge::HirariCore;
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::slint_ui::{store_onboarding_progress, sync_tracks_from_engine, AppWindow, Z_Track};
use crate::ui::operation_gate::{OperationGate, OperationKind};

fn create_native_template(
    core: &HirariCore,
    template_tracks: &[(&str, u32)],
) -> Result<(), String> {
    core.new_project();
    if !core.remove_track(0) {
        return Err("starter track could not be replaced".into());
    }
    for &(name, track_type) in template_tracks {
        let id = core.add_track(track_type);
        if id == 0 || !core.set_track_name(id, name) {
            return Err(format!("could not create template track {name}"));
        }
    }
    Ok(())
}

fn sync_native_template(tracks: &slint::VecModel<Z_Track>, core: &HirariCore) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        if sync_tracks_from_engine(tracks, core) {
            return true;
        }
        std::thread::yield_now();
    }
    false
}

pub(crate) fn handle_command(
    command: &str,
    core: &HirariCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
    session_recovery_path: &Rc<RefCell<Option<std::path::PathBuf>>>,
    operation_gate: &OperationGate,
) -> bool {
    let Some(template) = command.strip_prefix("INIT_") else {
        return false;
    };
    let Some(_lease) = operation_gate.try_enter(OperationKind::Load) else {
        ui.set_last_action("PROJECT BUSY: TEMPLATE IGNORED".into());
        return true;
    };
    // This template becomes an unsaved project, but the previous project's
    // persisted MIDI sidecar still belongs to that project and must survive.
    last_saved_path.borrow_mut().take();
    *session_recovery_path.borrow_mut() = crate::slint_ui::new_session_recovery_path();
    let template_tracks: &[(&str, u32)] = match template {
        "CINEMATIC SCORE" => &[
            ("Orchestra", 3),
            ("Strings", 2),
            ("Brass", 2),
            ("Percussion", 2),
            ("Piano", 1),
        ],
        "ATMOS MASTERING" => &[
            ("Mix Bus", 3),
            ("Atmos Bed", 0),
            ("Dialogue", 0),
            ("Master", 3),
        ],
        "VOICE-OVER PRO" => &[("Voice", 4), ("Music Bed", 0), ("Print Master", 3)],
        _ => &[
            ("Drums", 2),
            ("Bass", 2),
            ("Synth Lead", 2),
            ("Pads", 2),
            ("Main Out", 3),
        ],
    };
    if let Err(error) = create_native_template(core, template_tracks) {
        core.new_project();
        crate::ui::track_model::reset_project_scoped_track_overlays(tracks);
        let _ = sync_native_template(tracks, core);
        ui.set_show_genesis(true);
        ui.set_last_action(format!("TEMPLATE SETUP FAILED: {error}").into());
        return true;
    }
    crate::ui::track_model::reset_project_scoped_track_overlays(tracks);
    if !sync_native_template(tracks, core) {
        core.new_project();
        crate::ui::track_model::reset_project_scoped_track_overlays(tracks);
        let _ = sync_native_template(tracks, core);
        ui.set_show_genesis(true);
        ui.set_last_action("TEMPLATE SETUP FAILED: Core snapshot unavailable".into());
        return true;
    }
    crate::ui::project_commands::reset_project_scoped_ui(ui);
    ui.set_mx_open(false);
    ui.set_pr_open(false);
    ui.set_bot_view(0);
    ui.set_is_ply(false);
    // A template switch is a new editing session; never carry an interrupted
    // render/overlay state into it. Otherwise the full-window render layer can
    // legitimately cover the freshly created arrangement and look like a
    // black-screen failure on the next frame.
    ui.set_is_rendering(false);
    ui.set_render_state("IDLE".into());
    ui.set_render_error("".into());
    ui.set_export_open(false);
    ui.set_fx_open(false);
    ui.set_pal_open(false);
    ui.set_spotlight_active(false);
    ui.set_show_sentinel(false);
    ui.set_quick_help_open(false);
    ui.set_beginner_guide_open(false);
    store_onboarding_progress(true, 6);
    ui.set_project_save_status("Unsaved project".into());
    ui.set_last_action(
        format!("TEMPLATE READY: {template} ({} TRACKS)", tracks.row_count()).into(),
    );
    // Publish the ready project state together. Deferring only the overlay
    // toggle can leave a frame where the genesis layer is gone while the
    // conditional arrange tree has not been instantiated yet.
    ui.set_show_genesis(false);
    ui.set_workspace_preset("arrange".into());
    ui.set_bot_view(0);
    ui.set_mx_open(false);
    ui.set_pr_open(false);
    true
}
