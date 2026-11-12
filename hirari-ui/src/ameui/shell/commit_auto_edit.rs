//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Commits the automation gesture: one undo transaction around
    /// `set_automation_data` with beat-sorted native triples, mirroring
    /// the automation commit (it commits live during the drag
    /// inside the gesture's transaction; we commit once on release).
    pub(in crate::ameui) fn commit_auto_edit(&mut self) {
        let Some(edit) = self.auto_edit.take() else {
            return;
        };
        let Some(core) = self.core.clone() else {
            return;
        };
        let tracks = self.tracks.get();
        let Some(track) = tracks.iter().find(|track| track.id == edit.track_id) else {
            return;
        };
        let mut points: Vec<crate::ui_shared::arrange_model::AutomationPoint> = match edit.lane {
            0 => track.volume_automation.clone(),
            _ => track.pan_automation.clone(),
        };
        match edit.point_index {
            Some(index) => {
                let Some(point) = points.get_mut(index) else {
                    return;
                };
                point.beat = (edit.orig_beat + edit.delta_beats).max(0.0);
                point.value = (point.value + edit.delta_value).clamp(
                    if edit.lane == 0 { 0.0 } else { -1.0 },
                    if edit.lane == 0 { 2.0 } else { 1.0 },
                );
            }
            None => {
                points.push(crate::ui_shared::arrange_model::AutomationPoint {
                    beat: edit.orig_beat.max(0.0),
                    value: (edit.orig_value + edit.delta_value).clamp(
                        if edit.lane == 0 { 0.0 } else { -1.0 },
                        if edit.lane == 0 { 2.0 } else { 1.0 },
                    ),
                });
            }
        }
        points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let native: Vec<f64> = points
            .iter()
            .flat_map(|point| {
                let time = core.beats_to_samples(f64::from(point.beat.max(0.0))) as f64;
                [time, f64::from(point.value), 0.0]
            })
            .collect();
        core.begin_undo_transaction("Edit Automation");
        let accepted = core.set_automation_data(edit.track_id, edit.lane, native);
        if accepted {
            let _ = core.end_undo_transaction();
            self.refresh_arrange_state(&core);
        } else {
            let _ = core.abort_undo_transaction();
        }
        self.auto_point.set(None);
    }
}
