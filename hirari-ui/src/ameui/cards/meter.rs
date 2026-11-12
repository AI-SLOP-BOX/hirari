//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn meter_view(&self) -> View {
        let peak = self.master_peak.get();
        let status = self.status.get();
        let master_gain_core = self.core.clone();
        let master_gain_state = self.master_gain.clone();
        let master_gain = SliderView::new(self.master_gain.get(), 0.0, 2.0)
            .action_id("mixer.master_gain")
            .on_change(move |value| {
                if let Some(core) = &master_gain_core {
                    if core.set_master_gain(value.clamp(0.0, 2.0)) {
                        master_gain_state.set(value.clamp(0.0, 2.0));
                    }
                }
            });
        let meter = View::Card(vec![View::HStack(vec![
            View::Text("Master".to_owned()),
            View::ProgressBar((peak * 100.0) as u8).grow(1),
            View::Caption(format!("{:.0}%", peak * 100.0)),
            View::Caption("Gain".to_owned()),
            master_gain.into(),
            View::Caption(status),
        ])]);

        meter
    }
}
