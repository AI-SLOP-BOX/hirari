//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn master_card_view(&self) -> View {
        let integrated_lufs = self.integrated_lufs.get();
        let true_peak_l = self.true_peak_l.get();
        let true_peak_r = self.true_peak_r.get();
        let integrated_label = if integrated_lufs <= -119.0 {
            "— LUFS".to_owned()
        } else {
            format!("{integrated_lufs:.1} LUFS")
        };
        let master_card = View::Card(vec![View::HStack(vec![
            View::Text("MASTER".to_owned()),
            View::Caption(format!("GAIN {:.2}", self.master_gain.get())),
            View::Caption(integrated_label),
            View::Caption(format!(
                "TP L {true_peak_l:.1} dBTP · R {true_peak_r:.1} dBTP"
            )),
            View::Caption("Target −14 LUFS / −1 dBTP".to_owned()),
        ])]);

        master_card
    }
}
