//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn sends_card_view(&self) -> View {
        // SENDS card (audio routing / aux sends parity): source
        // cycler, destination bus cycler, gain slider with live apply,
        // pre/post toggle and send enable.
        let sends_card = {
            let tracks_for_sends = self.tracks.get();
            let source_count = tracks_for_sends.len() as u32;
            let bus_count = tracks_for_sends
                .iter()
                .filter(|track| track.kind == "Bus")
                .count() as u32;
            let source_index = if source_count == 0 {
                0
            } else {
                (self.send_source.get() as usize) % source_count as usize
            };
            let dest_index = if bus_count == 0 {
                0
            } else {
                (self.send_dest.get() as usize) % bus_count as usize
            };
            let source_label = tracks_for_sends
                .get(source_index)
                .map(|track| track.name.clone())
                .unwrap_or_else(|| "NO SOURCE".to_owned());
            let source_id = tracks_for_sends
                .get(source_index)
                .map(|track| track.id)
                .unwrap_or(0);
            let dest_track = tracks_for_sends
                .iter()
                .filter(|track| track.kind == "Bus")
                .nth(dest_index);
            let dest_label = dest_track
                .map(|track| track.name.clone())
                .unwrap_or_else(|| "NO BUS".to_owned());
            let dest_id = dest_track.map(|track| track.id).unwrap_or(0);
            let s_idx = self.send_source.clone();
            let send_source_button = ButtonView::new(format!("SRC: {source_label}"))
                .action_id("sends.source_cycle")
                .secondary()
                .on_press(move |_| {
                    if source_count > 0 {
                        s_idx.set((s_idx.get() + 1) % source_count);
                    }
                });
            let d_idx = self.send_dest.clone();
            let send_dest_button = ButtonView::new(format!("-> {dest_label}"))
                .action_id("sends.dest_cycle")
                .secondary()
                .on_press(move |_| {
                    if bus_count > 0 {
                        d_idx.set((d_idx.get() + 1) % bus_count);
                    }
                });
            let gain_state = self.send_gain.clone();
            let gain_apply_core = self.core.clone();
            let gain_source = source_id;
            let gain_dest = dest_id;
            let gain_enabled = self.send_enabled.clone();
            let gain_pre = self.send_pre.clone();
            let gain_slider = SliderView::new(self.send_gain.get(), 0.0, 1.0)
                .action_id("sends.gain")
                .on_change(move |value| {
                    let value = value.clamp(0.0, 1.0);
                    gain_state.set(value);
                    // Live-apply on change, like the aux-sends panel.
                    if let Some(core) = &gain_apply_core {
                        let _ = core.set_send_route(
                            gain_source,
                            gain_dest,
                            value,
                            gain_pre.get(),
                            gain_enabled.get(),
                        );
                    }
                });
            let pre_state = self.send_pre.clone();
            let pre_apply_core = self.core.clone();
            let pre_source = source_id;
            let pre_dest = dest_id;
            let pre_gain = self.send_gain.clone();
            let pre_enabled = self.send_enabled.clone();
            let send_pre_button = ButtonView::new(if self.send_pre.get() { "PRE" } else { "POST" })
                .action_id("sends.pre_toggle")
                .secondary()
                .on_press(move |_| {
                    let next = !pre_state.get();
                    pre_state.set(next);
                    if let Some(core) = &pre_apply_core {
                        let _ = core.set_send_route(
                            pre_source,
                            pre_dest,
                            pre_gain.get(),
                            next,
                            pre_enabled.get(),
                        );
                    }
                });
            let enable_core = self.core.clone();
            let enable_source = source_id;
            let enable_dest = dest_id;
            let enable_gain = self.send_gain.clone();
            let enable_pre = self.send_pre.clone();
            let enable_state = self.send_enabled.clone();
            let enable_button = ButtonView::new(if self.send_enabled.get() {
                "SEND ON"
            } else {
                "SEND OFF"
            })
            .action_id("sends.enable_toggle")
            .primary()
            .on_press(move |_| {
                let next = !enable_state.get();
                enable_state.set(next);
                if let Some(core) = &enable_core {
                    let _ = core.set_send_route(
                        enable_source,
                        enable_dest,
                        enable_gain.get(),
                        enable_pre.get(),
                        next,
                    );
                }
            });
            View::Card(vec![View::HStack(vec![
                View::Text("SENDS".to_owned()),
                send_source_button.into(),
                send_dest_button.into(),
                View::Caption("GAIN".to_owned()),
                gain_slider.into(),
                send_pre_button.into(),
                enable_button.into(),
            ])])
        };

        sends_card
    }
}
