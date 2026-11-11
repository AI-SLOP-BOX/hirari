//! Neutral template starter required by both the Slint home page and the AmeUI home page.
//! Only the engine-backed template creation lives here; Slint-specific decoration
//! (AppWindow drives, Z_Track model, recovery mirroring) stays in `ui::templates`.

use hirari_core_bridge::HirariCore;

/// Creates a fresh project seeded with the named template's tracks. Shared by
/// the Slint INIT_* flow and the AmeUI home page. The previous unsaved
/// project is abandoned (a new one is created) but any persisted MIDI
/// sidecar on disk is left alone.
pub(crate) fn create_template_project(core: &HirariCore, template: &str) -> Result<(), String> {
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
    create_native_template(core, template_tracks)
}

fn create_native_template(
    core: &HirariCore,
    template_tracks: &[(&str, u32)],
) -> Result<(), String> {
    // A brand-new project takes over, keeping the previous project's track
    // id 1 as the default bus the same way `core.new_project()` does.
    core.new_project();
    if !core.remove_track(1) {
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
