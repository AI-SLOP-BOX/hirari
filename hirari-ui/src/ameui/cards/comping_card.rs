//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn comping_card_view(&self) -> View {
        // COMPING card (P10): take list from comping_snapshot_json, crossfade
        // slider wired to set_comp_crossfade_normalized, and per-take SELECT
        // (select_comp_take) wiring into the engine's undo/notification path.
        let comp_crossfade = self.comp_crossfade.clone();
        let comp_crossfade_ref = self.core.clone();
        let comp_crossfade_slider = SliderView::new(self.comp_crossfade.get(), 0.0, 1.0)
            .action_id("comping.crossfade")
            .on_change(move |value| {
                let clamped = value.clamp(0.0, 1.0);
                comp_crossfade.set(clamped);
                if let Some(core) = &comp_crossfade_ref {
                    let _ = core.set_comp_crossfade_normalized(clamped);
                }
            });
        let comp_takes = self.comping_takes.get();
        let mut comp_rows: Vec<View> = Vec::with_capacity(comp_takes.len() + 2);
        comp_rows.push(View::Text("COMPING".to_owned()));
        comp_rows.push(View::HStack(vec![
            View::Caption("CROSSFADE".to_owned()),
            comp_crossfade_slider.into(),
        ]));
        if comp_takes.is_empty() {
            comp_rows.push(View::Caption(
                "No takes yet. Record audio on a track to create a take lane.".to_owned(),
            ));
        } else {
            for take in comp_takes {
                let take_id = take.id;
                let take_name = take.name.clone();
                let take_span = take.end_sample.saturating_sub(take.start_sample) as f64
                    / take.start_sample.max(1) as f64;
                let _ = take_span;
                let select_core = self.core.clone();
                let select_button = ButtonView::new(format!("SELECT #{take_id} {take_name}"))
                    .action_id(format!("comping.select.{take_id}"))
                    .secondary()
                    .on_press(move |_| {
                        if let Some(core) = &select_core {
                            let _ = core.select_comp_take(take_id);
                        }
                    });
                comp_rows.push(View::HStack(vec![select_button.into()]));
            }
        }
        let comping_card = View::Card(vec![View::VStack(comp_rows)]);

        comping_card
    }
}
