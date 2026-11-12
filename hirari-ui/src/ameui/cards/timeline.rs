//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn timeline_view(&self) -> View {
        // Timeline ruler above the arrange canvas: same zoom/scroll as the
        // canvas (px/beat units in beats-domain mode), 104px lane-header
        // indent, markers synced to the engine, scrub seeks the engine
        // playhead. The canvas keeps drawing lanes/clips below it.
        let last_timeline_width = (self.arrange_bounds.2 - 104.0).max(1.0);
        let timeline_end_beats = self.timeline_end_beats.get();
        let arrange_zoom = self.arrange_zoom.get();
        let base_px_per_beat = if timeline_end_beats > 0.0 {
            f64::from(last_timeline_width / timeline_end_beats)
        } else {
            60.0
        };
        let zoom_px_per_beat = base_px_per_beat * f64::from(arrange_zoom);
        let marker_list = self.markers.get();
        let mut sorted_markers = marker_list.clone();
        sorted_markers.sort_by(|a, b| {
            a.beat
                .partial_cmp(&b.beat)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        let timeline_zoom = self.arrange_zoom.clone();
        let timeline_scroll = self.arrange_scroll_beats.clone();
        let timeline_playhead = self.playhead_beats.clone();
        let timeline_seek_core = self.core.clone();
        let timeline_delete_core = self.core.clone();
        let timeline_move_core = self.core.clone();
        let sorted_marker_ids: Vec<u32> = sorted_markers.iter().map(|marker| marker.id).collect();
        let sorted_marker_delete_ids = sorted_marker_ids.clone();
        let sorted_marker_move_ids = sorted_marker_ids;
        let marker_add_core = self.core.clone();
        let marker_add_refresh = self.markers.clone();
        let marker_refresh = self.markers.clone();
        let arrange_timeline = TimelineView::new(
            self.playhead_beats.get(),
            f64::from(timeline_end_beats.max(1.0)),
            24,
        )
        .ruler_height(16)
        .header_indent(104)
        .beats_domain(4, 4)
        .zoom_if(Some(zoom_px_per_beat as f32))
        .offset_if(Some(f64::from(self.arrange_scroll_beats.get())))
        .markers(
            sorted_markers
                .iter()
                .map(|marker| TimelineMarker::new(marker.beat, marker.label.clone())),
        )
        .on_scrub(move |beat| {
            if (beat - timeline_playhead.get()).abs() < f64::EPSILON {
                return;
            }
            if let Some(core) = &timeline_seek_core {
                core.set_playhead(core.beats_to_samples(beat));
            }
        })
        .on_navigation(move |zoom, offset| {
            let clamped_zoom = (zoom as f64 / base_px_per_beat) as f32;
            timeline_zoom.set(clamped_zoom.clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX));
            timeline_scroll.set((offset as f32).max(0.0));
        })
        .on_marker_add(move |beat| {
            let Some(core) = &marker_add_core else {
                return;
            };
            let markers = marker_add_refresh.get();
            let next_id = markers.iter().map(|marker| marker.id).max().unwrap_or(0) + 1;
            let _ = core.upsert_marker(next_id, &format!("Mark {next_id}"), beat, "#646496");
        })
        .on_marker_delete(move |index| {
            let Some(core) = &timeline_delete_core else {
                return;
            };
            if let Some(id) = sorted_marker_delete_ids.get(index) {
                let _ = core.delete_marker(*id);
            }
        })
        .on_marker_move(move |index, beat| {
            let Some(core) = &timeline_move_core else {
                return;
            };
            let markers = marker_refresh.get();
            let Some(marker) = sorted_marker_move_ids
                .get(index)
                .and_then(|id| markers.iter().find(|marker| marker.id == *id))
            else {
                return;
            };
            let _ = core.upsert_marker(marker.id, &marker.label, beat, "#646496");
        });
        let arrange_timeline: View = arrange_timeline.into();
        arrange_timeline
    }
}
