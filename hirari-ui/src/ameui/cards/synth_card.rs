//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn synth_card_view(&self) -> View {
        // SYNTH card (synth panel parity): engine picker and the
        // eight engine macros via set_macro_value. Local echo mirrors what
        // was sent; no engine getter exists for these values yet.
        let synth_engine_state = self.synth_engine.clone();
        let engine_label = match self.synth_engine.get() {
            0 => "SYNTH",
            2 => "SAMPLER",
            _ => "WAVETABLE",
        };
        let synth_engine_core = self.core.clone();
        let synth_engine_button = ButtonView::new(format!("ENGINE: {engine_label}"))
            .action_id("synth.engine_cycle")
            .secondary()
            .on_press(move |_| {
                let next = (synth_engine_state.get() + 1) % 3;
                synth_engine_state.set(next);
                if let Some(core) = &synth_engine_core {
                    let _ = core.set_preview_synth_engine(next);
                }
            });
        let mut synth_macros: Vec<View> = vec![synth_engine_button.into()];
        for index in 0..8u32 {
            let macros_state = self.synth_macros.clone();
            let macro_core = self.core.clone();
            let current = self.synth_macros.get()[index as usize];
            let slider = SliderView::new(current, 0.0, 1.0)
                .action_id(format!("synth.macro.{index}"))
                .on_change(move |value| {
                    let mut macros = macros_state.get();
                    macros[index as usize] = value.clamp(0.0, 1.0);
                    macros_state.set(macros);
                    if let Some(core) = &macro_core {
                        let _ = core.set_macro_value(index, value.clamp(0.0, 1.0));
                    }
                });
            synth_macros.push(View::HStack(vec![
                View::Caption(format!("M{}", index + 1)),
                slider.into(),
            ]));
        }
        let synth_card = View::Card(vec![View::VStack(synth_macros)]);

        synth_card
    }
}
