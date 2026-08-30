use aura_core_bridge::AuraCore;
use slint::{Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::Z_Track;
use crate::ui::sync::replace_track;

pub fn handle_command(command: &str, core: &AuraCore, tracks: &Rc<VecModel<Z_Track>>) -> bool {
    let Some(track_id) = command
        .strip_prefix("assign_map_")
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return false;
    };
    core.set_articulation_map(track_id, "Violins 1 Pro".into());
    for index in 0..tracks.row_count() {
        let Some(mut track) = tracks.row_data(index) else {
            continue;
        };
        if track.id == track_id as i32 {
            track.artic_map = "Violins 1 Pro".into();
            replace_track(tracks, index, track);
            break;
        }
    }
    true
}
