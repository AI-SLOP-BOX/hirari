//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// A dropped audio file becomes a clip on the lane under the drop
    /// point (falling back to the piano track and the playhead), via the
    /// engine's queued import, like the audio import.
    pub(in crate::ameui) fn on_file_dropped(&mut self, path: &str, x: Option<f64>, y: Option<f64>) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let (bounds_x, bounds_y, _, _) = self.arrange_bounds;
        let geometry = self.arrange_geometry;
        let tracks = self.tracks.get();
        let (track_id, beat) = match (x, y) {
            (Some(drop_x), Some(drop_y)) => {
                let local_x = drop_x as f32 - bounds_x;
                let local_y = drop_y as f32 - bounds_y;
                if geometry.lane_height <= 0.0 || local_y < geometry.ruler_height {
                    return;
                }
                let lane = ((local_y - geometry.ruler_height) / geometry.lane_height) as usize;
                let Some(track) = tracks.get(lane) else {
                    return;
                };
                (track.id, f64::from(geometry.beat_for_x(local_x)))
            }
            _ => {
                let track_id = self.piano_track.get();
                (track_id, self.playhead_beats.get())
            }
        };
        if track_id == 0 {
            return;
        }
        let ticket = core.queue_region_import_at_beat(track_id, path, beat);
        if ticket == 0 {
            self.status.set("IMPORT FAILED".to_owned());
            return;
        }
        self.status.set(format!(
            "IMPORTING: {}",
            std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_owned())
        ));
        self.pending_import = Some((ticket, path.to_owned(), track_id));
    }
    /// Zooms the arrange canvas by `factor`, anchored at canvas x so the
    /// beat under the pointer stays put (wheel and pinch share this).
    pub(in crate::ameui) fn zoom_arrange_at(&self, x: f32, factor: f32) {
        let geometry = self.arrange_geometry;
        if geometry.px_per_beat <= 0.0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let zoom_old = self.arrange_zoom.get();
        let zoom_new = (zoom_old * factor).clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX);
        let timeline_end = self.timeline_end_beats.get();
        let timeline_width = geometry.px_per_beat * timeline_end / zoom_old;
        let px_new = timeline_width / timeline_end * zoom_new;
        let beat = geometry.beat_for_x(x);
        let scroll_new = beat - (x - geometry.header_width) / px_new;
        self.arrange_zoom.set(zoom_new);
        self.arrange_scroll_beats.set(scroll_new.max(0.0));
    }
}
