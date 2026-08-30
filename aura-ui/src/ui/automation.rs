use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{
    ui_error_message, AppWindow, AutomationActions, UiErrorKind, Z_AutomationLane,
    Z_AutomationPoint, Z_Track,
};
use crate::ui::sync::replace_track;

fn next_automation_mode(mode: i32) -> i32 {
    (mode.rem_euclid(4) + 1) % 4
}

pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    ui.global::<AutomationActions>().on_toggle_automation({
        let weak = weak.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.show_automation = !track.show_automation;
                    let visible = track.show_automation;
                    replace_track(&tracks, i, track);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            format!(
                                "AUTOMATION VIEW {}: {}",
                                id,
                                if visible { "ON" } else { "OFF" }
                            )
                            .into(),
                        );
                    }
                    break;
                }
            }
        }
    });
    ui.global::<AutomationActions>().on_automation_point_moved({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, lane_index, point_index, beat, value, curve| {
            if !beat.is_finite() || !value.is_finite() || !curve.is_finite() {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != track_id {
                    continue;
                }
                let mut lanes: Vec<Z_AutomationLane> = track.auto_lanes.iter().collect();
                let Some(lane) = lanes.get_mut(lane_index.max(0) as usize) else {
                    return;
                };
                let mut points: Vec<Z_AutomationPoint> = lane.points.iter().collect();
                let Some(point) = points.get_mut(point_index.max(0) as usize) else {
                    return;
                };
                point.beat = beat.max(0.0);
                point.value = value.clamp(0.0, 1.0);
                point.curve = curve.clamp(-1.0, 1.0);
                points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                if lane_index == 0 || lane_index == 1 || lane_index == 2 {
                    let is_delay = lane.name.as_str() == "Track Delay";
                    let native: Vec<f64> = points
                        .iter()
                        .flat_map(|p| {
                            let time = if is_delay {
                                core.beats_to_samples(p.beat.max(0.0) as f64) as f64
                            } else {
                                // The legacy volume/pan API is sample-based in
                                // the native graph too; keeping the conversion
                                // here prevents beat/sample unit drift at 96k.
                                core.beats_to_samples(p.beat.max(0.0) as f64) as f64
                            };
                            [time, p.value as f64, p.curve as f64]
                        })
                        .collect();
                    core.begin_undo_transaction("Edit Automation");
                    let accepted = if is_delay {
                        serde_json::from_str::<serde_json::Value>(
                            &core.set_track_delay_automation_diagnostic_json(
                                track_id as u32,
                                native,
                            ),
                        )
                        .ok()
                        .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                        .unwrap_or(false)
                    } else {
                        core.set_automation_data(track_id as u32, lane_index as u32, native)
                    };
                    let accepted = if accepted {
                        core.end_undo_transaction()
                    } else {
                        let _ = core.abort_undo_transaction();
                        false
                    };
                    if !accepted {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_last_action(
                                ui_error_message(
                                    UiErrorKind::Project,
                                    "automation update rejected by Core",
                                )
                                .into(),
                            );
                        }
                        return;
                    }
                }
                lane.points = slint::ModelRc::new(VecModel::from(points));
                track.auto_lanes = slint::ModelRc::new(VecModel::from(lanes));
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "AUTOMATION POINT: TRACK {} LANE {} POINT {}",
                            track_id, lane_index, point_index
                        )
                        .into(),
                    );
                }
                return;
            }
        }
    });
    ui.global::<AutomationActions>().on_clear_automation_lane({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, lane_index| {
            if !(0..=2).contains(&lane_index) {
                return;
            }
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != track_id {
                    continue;
                }
                let mut lanes: Vec<Z_AutomationLane> = track.auto_lanes.iter().collect();
                let Some(lane) = lanes.get_mut(lane_index as usize) else {
                    return;
                };
                let is_delay = lane.name.as_str() == "Track Delay";
                core.begin_undo_transaction("Clear Automation");
                let accepted = if is_delay {
                    serde_json::from_str::<serde_json::Value>(
                        &core.set_track_delay_automation_diagnostic_json(
                            track_id as u32,
                            Vec::new(),
                        ),
                    )
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false)
                } else {
                    core.set_automation_data(track_id as u32, lane_index as u32, Vec::new())
                };
                let accepted = if accepted {
                    core.end_undo_transaction()
                } else {
                    let _ = core.abort_undo_transaction();
                    false
                };
                if !accepted {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Project,
                                "automation clear rejected by Core",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                lane.points = slint::ModelRc::new(VecModel::from(Vec::<Z_AutomationPoint>::new()));
                track.auto_lanes = slint::ModelRc::new(VecModel::from(lanes));
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!("AUTOMATION CLEAR: TRACK {} LANE {}", track_id, lane_index).into(),
                    );
                }
                return;
            }
        }
    });
    ui.global::<AutomationActions>().on_toggle_auto_rw({
        let weak = ui.as_weak();
        let tracks = tracks.clone();
        move |id| {
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != id {
                    continue;
                }
                track.auto_rw = next_automation_mode(track.auto_rw);
                let mode = track.auto_rw;
                let applied = core.set_automation_record_mode(mode as u32);
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if applied {
                        format!("AUTOMATION MODE {}: {}", id, mode).into()
                    } else {
                        ui_error_message(UiErrorKind::Project, "automation mode rejected by Core")
                            .into()
                    });
                }
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::next_automation_mode;

    #[test]
    fn automation_mode_cycles_and_recovers_invalid_values() {
        assert_eq!(next_automation_mode(0), 1);
        assert_eq!(next_automation_mode(3), 0);
        assert_eq!(next_automation_mode(-1), 0);
        assert_eq!(next_automation_mode(8), 1);
    }
}
