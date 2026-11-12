//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn markers_card_view(&self) -> View {
        // MARKERS card (global tracks parity): add marker at playhead,
        // list beat/label/delete for each persisted marker.
        let sorted_for_card = {
            let markers = self.markers.get();
            let mut sorted = markers.clone();
            sorted.sort_by(|a, b| {
                a.beat
                    .partial_cmp(&b.beat)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.id.cmp(&b.id))
            });
            sorted
        };
        let add_marker_core = self.core.clone();
        let add_marker_playhead = self.playhead_beats.clone();
        let add_marker_refresh = self.markers.clone();
        let add_marker_button = ButtonView::new("ADD MARKER")
            .action_id("markers.add")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &add_marker_core else { return };
                let markers = add_marker_refresh.get();
                let next_id = markers.iter().map(|marker| marker.id).max().unwrap_or(0) + 1;
                let beat = add_marker_playhead.get();
                let _ = core.upsert_marker(next_id, &format!("Mark {next_id}"), beat, "#646496");
                add_marker_refresh.set(parse_markers(&core.markers_json()));
            });
        let mut marker_rows: Vec<View> = Vec::new();
        marker_rows.push(View::Text(format!("MARKERS · {}", sorted_for_card.len())));
        marker_rows.push(View::HStack(vec![add_marker_button.into()]));
        for marker in sorted_for_card {
            let id = marker.id;
            let label = marker.label.clone();
            let beat = marker.beat;
            let delete_core = self.core.clone();
            let delete_refresh = self.markers.clone();
            let delete_button = ButtonView::new("DEL")
                .action_id(format!("markers.delete.{id}"))
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &delete_core else { return };
                    let _ = core.delete_marker(id);
                    delete_refresh.set(parse_markers(&core.markers_json()));
                });
            marker_rows.push(View::HStack(vec![
                View::Caption(format!("BEAT {:.2}", beat)),
                View::Caption(label),
                View::Spacer,
                delete_button.into(),
            ]));
        }
        let markers_card = View::Card(vec![View::VStack(marker_rows)]);

        markers_card
    }
}
