use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{AppWindow, TimelineActions, Z_Marker, Z_Track};

fn normalized_range(a: f32, b: f32) -> Option<(f32, f32)> {
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    Some((a.min(b), a.max(b)))
}

/// Timeline interactions own marker navigation and selection feedback.
/// Marker edits are applied to the shared model before the next frame, while
/// transport seeks go directly to the Core source of truth.
pub fn install(
    ui: &AppWindow,
    core: Rc<AuraCore>,
    markers: Rc<VecModel<Z_Marker>>,
    tracks: Rc<VecModel<Z_Track>>,
) {
    let weak = ui.as_weak();
    ui.global::<TimelineActions>().on_scrub_to_marker({
        let weak = weak.clone();
        let core = core.clone();
        let markers = markers.clone();
        move |index| {
            let Some(marker) = markers.row_data(index.max(0) as usize) else {
                return;
            };
            let beat = marker.beat.max(0.0) as f64;
            core.set_playhead(core.beats_to_samples(beat));
            if let Some(ui) = weak.upgrade() {
                ui.set_ph(beat as f32);
                ui.set_last_action(format!("SCRUB: {} · {:.2} beats", marker.label, beat).into());
            }
        }
    });

    ui.global::<TimelineActions>().on_add_marker({
        let weak = weak.clone();
        let markers = markers.clone();
        let core = core.clone();
        move |beat| {
            if !beat.is_finite() || beat < 0.0 {
                return;
            }
            let id = (1..=65_536u32)
                .find(|candidate| {
                    !(0..markers.row_count())
                        .filter_map(|index| markers.row_data(index))
                        .any(|marker| marker.id == *candidate as i32)
                })
                .unwrap_or(65_536);
            let label = format!("MARKER {id}");
            if !core.upsert_marker(id, &label, beat as f64, "#6472a8") {
                return;
            }
            let marker = Z_Marker {
                id: id as i32,
                label: label.clone().into(),
                beat,
                color: slint::Color::from_rgb_u8(100, 114, 168),
            };
            let mut rows = (0..markers.row_count())
                .filter_map(|index| markers.row_data(index))
                .collect::<Vec<_>>();
            rows.push(marker);
            rows.sort_by(|a, b| a.beat.total_cmp(&b.beat));
            markers.set_vec(rows);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(format!("MARKER ADDED: {label}").into());
            }
        }
    });

    ui.global::<TimelineActions>().on_marker_moved({
        let weak = weak.clone();
        let markers = markers.clone();
        let core = core.clone();
        move |index, beat| {
            let index = index.max(0) as usize;
            let Some(mut marker) = markers.row_data(index) else {
                return;
            };
            if !beat.is_finite() {
                return;
            }
            marker.beat = beat.max(0.0);
            markers.set_row_data(index, marker.clone());
            // Rows are sorted by beat, so use the persisted marker identity,
            // never the current row index, when publishing the edit.
            let _ = core.upsert_marker(
                marker.id.max(1) as u32,
                &marker.label,
                marker.beat as f64,
                "#6472a8",
            );
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!("MARKER MOVED: {} · {:.2}", marker.label, marker.beat).into(),
                );
            }
        }
    });

    ui.global::<TimelineActions>().on_area_select({
        let weak = weak.clone();
        let tracks = tracks.clone();
        move |track_id, x1, y1, x2, y2| {
            let Some((left, right)) = normalized_range(x1, x2) else {
                return;
            };
            let Some((top, bottom)) = normalized_range(y1, y2) else {
                return;
            };
            let Some(row) = (0..tracks.row_count()).find(|index| {
                tracks
                    .row_data(*index)
                    .is_some_and(|track| track.id == track_id)
            }) else {
                return;
            };
            let Some(mut track) = tracks.row_data(row) else {
                return;
            };
            let mut clips: Vec<_> = track.clips.iter().collect();
            let mut selected_id = None;
            let mut selected_count = 0u32;
            for clip in &mut clips {
                let clip_left = clip.start_beat;
                let clip_right = clip.start_beat + clip.length_beats.max(0.0);
                let clip_top = 3.0 + clip.layer as f32 * 42.0;
                let clip_bottom = clip_top + 38.0;
                let selected = clip_right >= left
                    && clip_left <= right
                    && clip_bottom >= top
                    && clip_top <= bottom;
                clip.selected = selected;
                if selected {
                    selected_count = selected_count.saturating_add(1);
                    if selected_id.is_none() {
                        selected_id = Some(clip.id);
                    }
                }
            }
            track.clips = slint::ModelRc::new(VecModel::from(clips));
            tracks.set_row_data(row, track);
            if let Some(ui) = weak.upgrade() {
                ui.set_sel_idx(row as i32);
                ui.set_sel_cid(selected_id.unwrap_or(-1));
                ui.set_last_action(
                    format!(
                        "AREA SELECT: TRACK {} · {:.2}..{:.2} / {:.2}..{:.2} · {} CLIP(S)",
                        track_id, left, right, top, bottom, selected_count
                    )
                    .into(),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::normalized_range;

    #[test]
    fn range_normalization_is_order_independent() {
        assert_eq!(normalized_range(8.0, 2.0), Some((2.0, 8.0)));
    }

    #[test]
    fn range_normalization_rejects_non_finite_values() {
        assert_eq!(normalized_range(f32::NAN, 2.0), None);
        assert_eq!(normalized_range(2.0, f32::INFINITY), None);
    }
}
