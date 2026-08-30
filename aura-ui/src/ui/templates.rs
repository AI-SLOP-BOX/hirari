use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

use crate::slint_ui::{
    fallback_template_tracks, midi_notes_path, sync_tracks_from_engine, AppWindow, Z_Track,
};

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
    tracks.set_vec(Vec::new());
    sync_tracks_from_engine(tracks, core);
    if tracks.row_count() == 0 {
        tracks.set_vec(fallback_template_tracks(template_tracks));
    }
    ui.set_tracks(slint::ModelRc::new(tracks.clone()));
    ui.set_mx_open(false);
    ui.set_pr_open(false);
    ui.set_bot_view(0);
    ui.set_sel_idx(0);
    ui.set_is_ply(false);
    ui.set_auto_save_status("Auto-save: New Project".into());
    ui.set_last_action(
        format!("TEMPLATE READY: {template} ({} TRACKS)", tracks.row_count()).into(),
    );
    true
}
