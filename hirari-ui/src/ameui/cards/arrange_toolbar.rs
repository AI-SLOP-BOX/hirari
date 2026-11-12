//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn arrange_toolbar_view(&self) -> View {
        let zoom = self.arrange_zoom.get();
        // Arrange toolbar (dock header): zoom steps match the dock
        // ±1.25 buttons; FIT returns to fit-to-width. Ruler drag pans.
        let zoom_out_state = self.arrange_zoom.clone();
        let zoom_out = ButtonView::new("−")
            .action_id("arrange.zoom_out")
            .secondary()
            .on_press(move |_| {
                zoom_out_state.set((zoom_out_state.get() / 1.25).max(ARRANGE_ZOOM_MIN));
            });
        let zoom_in_state = self.arrange_zoom.clone();
        let zoom_in = ButtonView::new("+")
            .action_id("arrange.zoom_in")
            .secondary()
            .on_press(move |_| {
                zoom_in_state.set((zoom_in_state.get() * 1.25).min(ARRANGE_ZOOM_MAX));
            });
        let fit_zoom_state = self.arrange_zoom.clone();
        let fit_scroll_state = self.arrange_scroll_beats.clone();
        let zoom_fit = ButtonView::new("FIT")
            .action_id("arrange.zoom_fit")
            .secondary()
            .on_press(move |_| {
                fit_zoom_state.set(1.0);
                fit_scroll_state.set(0.0);
            });
        let snap_state = self.arrange_snap_division.clone();
        let snap_label = format!("SNAP 1/{}", self.arrange_snap_division.get() as u32);
        let snap_button = ButtonView::new(snap_label)
            .action_id("arrange.snap_cycle")
            .secondary()
            .on_press(move |_| {
                let current = snap_state.get();
                let next = SNAP_DIVISIONS
                    .iter()
                    .find(|division| **division > current)
                    .copied()
                    .unwrap_or(SNAP_DIVISIONS[0]);
                snap_state.set(next);
            });
        let lock_state = self.arrange_locked.clone();
        let locked = self.arrange_locked.get();
        let lock_button = ButtonView::new(if locked { "UNLOCK" } else { "LOCK" })
            .action_id("arrange.lock_toggle")
            .secondary()
            .on_press(move |_| {
                lock_state.set(!lock_state.get());
            });
        // Automation edit mode (auto lanes): AUTO toggles lane
        // editing, the lane button cycles volume/pan, DEL removes the
        // selected automation point.
        let auto_state = self.auto_mode.clone();
        let auto_on = self.auto_mode.get();
        let auto_button = ButtonView::new(if auto_on { "AUTO ON" } else { "AUTO" })
            .action_id("arrange.auto_mode")
            .secondary()
            .on_press(move |_| {
                auto_state.set(!auto_on);
            });
        let auto_lane_state = self.auto_lane.clone();
        let auto_lane_label = if self.auto_lane.get() == 0 {
            "VOL"
        } else {
            "PAN"
        };
        let auto_lane_button = ButtonView::new(auto_lane_label)
            .action_id("arrange.auto_lane")
            .secondary()
            .on_press(move |_| {
                auto_lane_state.set((auto_lane_state.get() + 1) % 2);
            });
        let auto_del_core = self.core.clone();
        let auto_del_tracks = self.tracks.clone();
        let auto_del_point = self.auto_point.clone();
        let auto_del_track = self.auto_track.clone();
        let auto_del_lane = self.auto_lane.clone();
        let auto_del_locked = self.arrange_locked.clone();
        let auto_del_button = ButtonView::new("DEL PT")
            .action_id("arrange.auto_delete_point")
            .secondary()
            .on_press(move |_| {
                if auto_del_locked.get() {
                    return;
                }
                let Some(core) = &auto_del_core else { return };
                let Some(index) = auto_del_point.get() else {
                    return;
                };
                let tracks = auto_del_tracks.get();
                let track_id = auto_del_track.get();
                let lane = auto_del_lane.get();
                let Some(track) = tracks.iter().find(|track| track.id == track_id) else {
                    return;
                };
                let mut points: Vec<crate::ui_shared::arrange_model::AutomationPoint> = match lane {
                    0 => track.volume_automation.clone(),
                    _ => track.pan_automation.clone(),
                };
                if index >= points.len() {
                    return;
                }
                points.remove(index);
                let native: Vec<f64> = points
                    .iter()
                    .flat_map(|point| {
                        let time = core.beats_to_samples(f64::from(point.beat.max(0.0))) as f64;
                        [time, f64::from(point.value), 0.0]
                    })
                    .collect();
                core.begin_undo_transaction("Edit Automation");
                if core.set_automation_data(track_id, lane, native) {
                    let _ = core.end_undo_transaction();
                    auto_del_tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                } else {
                    let _ = core.abort_undo_transaction();
                }
                auto_del_point.set(None);
            });
        let arrange_toolbar = View::Card(vec![View::HStack(vec![
            View::Text("ARRANGEMENT".to_owned()),
            zoom_out.into(),
            zoom_in.into(),
            zoom_fit.into(),
            snap_button.into(),
            lock_button.into(),
            auto_button.into(),
            auto_lane_button.into(),
            auto_del_button.into(),
            View::Caption(format!("ZOOM {}%", (zoom * 100.0).round() as u32)),
            View::Caption("ruler: pan · empty lane: seek · clip: move · edges: trim".to_owned()),
        ])]);

        arrange_toolbar
    }
}
