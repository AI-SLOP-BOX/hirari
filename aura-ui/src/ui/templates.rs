use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use crate::slint_ui::{fallback_template_tracks, midi_notes_path, AppWindow, Z_Track};

pub fn handle_command(
    command: &str,
    core: &AuraCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
    last_saved_path: &Rc<RefCell<Option<String>>>,
) -> bool {
    let Some(template) = command.strip_prefix("INIT_") else {
        return false;
    };
    if let Some(old_path) = last_saved_path.borrow_mut().take() {
        let _ = fs::remove_file(midi_notes_path(&old_path));
    }
    core.new_project();
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
    if let Some((first_name, _)) = template_tracks.first() {
        let _ = core.set_track_name(0, first_name);
        for &(name, track_type) in template_tracks.iter().skip(1) {
            let id = core.add_track(track_type);
            if id != 0 {
                let _ = core.set_track_name(id, name);
            }
        }
    }
    // Keep the currently bound model populated while the native snapshot is
    // refreshed. Clearing it first creates a zero-track frame; on macOS that
    // can tear down the large conditional view tree before Slint has a chance
    // to publish the replacement, leaving only the chrome visible.
    // Seed the visual model before asking the native graph for its snapshot.
    // This prevents the release UI from entering a zero-row conditional tree
    // during the short interval in which the new project layout is published.
    tracks.set_vec(fallback_template_tracks(template_tracks));
    // The template model is authoritative for this transaction. The native
    // graph is updated above, but asking it for a snapshot here can expose a
    // transient empty layout while its realtime graph is rebuilding. That
    // snapshot must not replace the validated UI model during the same frame.
    // The model is installed once during window construction. Replacing the
    // ModelRc from inside its own command callback can trigger a recursive
    // binding/layout pass on macOS, leaving a black window at high CPU.
    // Mutating the existing VecModel above is sufficient and preserves all
    // bindings to the arrange, mixer, and meter surfaces.
    ui.set_mx_open(false);
    ui.set_pr_open(false);
    ui.set_bot_view(0);
    ui.set_sel_idx(0);
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
    ui.set_fx_active_id(-1);
    ui.set_pal_open(false);
    ui.set_spotlight_active(false);
    ui.set_show_sentinel(false);
    ui.set_quick_help_open(false);
    ui.set_auto_save_status("Auto-save: New Project".into());
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
