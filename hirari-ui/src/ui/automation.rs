use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::slint_ui::{
    ui_error_message, AppWindow, AutomationActions, UiErrorKind, Z_AutomationLane,
    Z_AutomationPoint, Z_Track,
};
use crate::ui::sync::replace_track;

fn next_automation_mode(mode: i32) -> i32 {
    (mode.rem_euclid(4) + 1) % 4
}

fn sort_automation_lane(tracks: &VecModel<Z_Track>, track_id: i32, lane_index: i32) {
    if lane_index < 0 {
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
        let mut points: Vec<Z_AutomationPoint> = lane.points.iter().collect();
        points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        lane.points = slint::ModelRc::new(VecModel::from(points));
        track.auto_lanes = slint::ModelRc::new(VecModel::from(lanes));
        tracks.set_row_data(row, track);
        return;
    }
}

fn finish_automation_edit(
    core: &HirariCore,
    tracks: &VecModel<Z_Track>,
    active_edit: &Rc<RefCell<Option<(i32, i32)>>>,
    track_id: i32,
    lane_index: i32,
) {
    let matches_active = active_edit.borrow().as_ref() == Some(&(track_id, lane_index));
    if !matches_active {
        return;
    }
    *active_edit.borrow_mut() = None;
    let _ = core.end_undo_transaction();
    sort_automation_lane(tracks, track_id, lane_index);
}

pub fn install(ui: &AppWindow, core: Rc<HirariCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    let active_edit = Rc::new(RefCell::new(None::<(i32, i32)>));
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
    ui.global::<AutomationActions>()
        .on_automation_edit_started({
            let active_edit = active_edit.clone();
            let core = core.clone();
            let tracks = tracks.clone();
            move |track_id, lane_index| {
                if lane_index < 0
                    || !(0..tracks.row_count()).any(|row| {
                        tracks.row_data(row).is_some_and(|track| {
                            track.id == track_id && lane_index < track.auto_lanes.row_count() as i32
                        })
                    })
                {
                    return;
                }
                let previous_edit = active_edit.borrow_mut().take();
                if let Some((previous_track, previous_lane)) = previous_edit {
                    let _ = core.end_undo_transaction();
                    sort_automation_lane(&tracks, previous_track, previous_lane);
                }
                core.begin_undo_transaction("Edit Automation");
                *active_edit.borrow_mut() = Some((track_id, lane_index));
            }
        });
    ui.global::<AutomationActions>().on_automation_point_moved({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        let active_edit = active_edit.clone();
        move |track_id, lane_index, point_index, beat, value, curve| {
            if lane_index < 0
                || point_index < 0
                || !beat.is_finite()
                || !value.is_finite()
                || !curve.is_finite()
                || active_edit.borrow().as_ref() != Some(&(track_id, lane_index))
            {
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
                let mut points: Vec<Z_AutomationPoint> = lane.points.iter().collect();
                let Some(point) = points.get_mut(point_index as usize) else {
                    return;
                };
                let previous_point = point.clone();
                point.beat = beat.max(0.0);
                let normalized_value = value.clamp(0.0, 1.0);
                point.value = if lane.plugin_index >= 0 {
                    normalized_value
                } else {
                    match lane_index {
                        0 => normalized_value * 2.0,
                        1 => normalized_value * 2.0 - 1.0,
                        2 => normalized_value,
                        _ => {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_last_action(
                                    ui_error_message(
                                        UiErrorKind::Project,
                                        "unknown automation lane",
                                    )
                                    .into(),
                                );
                            }
                            return;
                        }
                    }
                };
                point.curve = curve.clamp(-1.0, 1.0);
                // Keep the displayed point indices stable for the entire
                // pointer gesture. The engine still receives chronological
                // points because it validates automation times strictly.
                let mut ordered_points = points.clone();
                ordered_points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                let is_delay = lane.name.as_str() == "Track Delay";
                let native: Vec<f64> = ordered_points
                    .iter()
                    .flat_map(|p| {
                        // All automation lanes are stored against native sample
                        // positions, even though the editor displays beats.
                        let time = core.beats_to_samples(p.beat.max(0.0) as f64) as f64;
                        [time, p.value as f64, p.curve as f64]
                    })
                    .collect();
                let accepted = if lane.plugin_index >= 0 {
                    core.set_plugin_automation(
                        track_id as u32,
                        lane.plugin_index as u32,
                        lane.parameter_id as u32,
                        native,
                    )
                } else if is_delay {
                    serde_json::from_str::<serde_json::Value>(
                        &core.set_track_delay_automation_diagnostic_json(track_id as u32, native),
                    )
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false)
                } else {
                    core.set_automation_data(track_id as u32, lane_index as u32, native)
                };
                if !accepted {
                    points[point_index as usize] = previous_point;
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
    ui.global::<AutomationActions>().on_add_automation_point({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, lane_index, beat, value| {
            if lane_index < 0 || !beat.is_finite() || !value.is_finite() {
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
                if lane.plugin_index < 0 {
                    return;
                }
                let beat = beat.max(0.0);
                let sample = core.beats_to_samples(beat as f64);
                let mut points: Vec<Z_AutomationPoint> = lane.points.iter().collect();
                let point = Z_AutomationPoint {
                    beat,
                    value: value.clamp(0.0, 1.0),
                    curve: 0.0,
                };
                if let Some(existing) = points
                    .iter_mut()
                    .find(|existing| core.beats_to_samples(existing.beat.max(0.0) as f64) == sample)
                {
                    *existing = point;
                } else {
                    points.push(point);
                }
                points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                let packed = points
                    .iter()
                    .flat_map(|point| {
                        [
                            core.beats_to_samples(point.beat.max(0.0) as f64) as f64,
                            point.value as f64,
                            point.curve as f64,
                        ]
                    })
                    .collect();
                core.begin_undo_transaction("Add Plugin Automation Point");
                let accepted = core.set_plugin_automation(
                    track_id as u32,
                    lane.plugin_index as u32,
                    lane.parameter_id as u32,
                    packed,
                );
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
                                "automation point rejected by Core",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                lane.points = slint::ModelRc::new(VecModel::from(points));
                lane.active = true;
                track.auto_lanes = slint::ModelRc::new(VecModel::from(lanes));
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "AUTOMATION POINT ADDED: TRACK {} LANE {}",
                            track_id, lane_index
                        )
                        .into(),
                    );
                }
                return;
            }
        }
    });
    ui.global::<AutomationActions>()
        .on_automation_edit_finished({
            let core = core.clone();
            let tracks = tracks.clone();
            let active_edit = active_edit.clone();
            move |track_id, lane_index| {
                finish_automation_edit(&core, &tracks, &active_edit, track_id, lane_index);
            }
        });
    ui.global::<AutomationActions>().on_clear_automation_lane({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, lane_index| {
            if lane_index < 0 {
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
                let Some(lane) = lanes.get(lane_index as usize) else {
                    return;
                };
                let is_delay = lane.name.as_str() == "Track Delay";
                let plugin_index = lane.plugin_index;
                let parameter_id = lane.parameter_id;
                core.begin_undo_transaction("Clear Automation");
                let accepted = if plugin_index >= 0 {
                    core.set_plugin_automation(
                        track_id as u32,
                        plugin_index as u32,
                        parameter_id as u32,
                        Vec::new(),
                    )
                } else if is_delay {
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
                if plugin_index >= 0 {
                    lanes.remove(lane_index as usize);
                } else if let Some(lane) = lanes.get_mut(lane_index as usize) {
                    lane.points =
                        slint::ModelRc::new(VecModel::from(Vec::<Z_AutomationPoint>::new()));
                }
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
