//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn transport_view(&self) -> View {
        let playing = self.playing.get();
        let tempo = self.tempo.get();
        let beats = self.playhead_beats.get();
        let sample_rate = self.sample_rate.get();
        let play_core = self.core.clone();
        let play_state = self.playing.clone();
        let play_button = ButtonView::new(if playing { "Playing" } else { "Play" })
            .action_id("transport.play")
            .primary()
            .on_press(move |_| {
                if let Some(core) = &play_core {
                    core.set_playing(true);
                }
                play_state.set(true);
            });

        let stop_core = self.core.clone();
        let stop_state = self.playing.clone();
        let stop_button = ButtonView::new("Stop")
            .action_id("transport.stop")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &stop_core {
                    core.set_playing(false);
                }
                stop_state.set(false);
            });

        let tempo_core = self.core.clone();
        let tempo_state = self.tempo.clone();
        let tempo_slider = SliderView::new(tempo, 20.0, 300.0)
            .action_id("transport.tempo")
            .on_change(move |value| {
                if let Some(core) = &tempo_core {
                    core.set_tempo(value);
                }
                tempo_state.set(value);
            });

        let loop_core = self.core.clone();
        let loop_state = self.loop_enabled.clone();
        let loop_on = self.loop_enabled.get();
        let loop_button = ButtonView::new(if loop_on { "Loop On" } else { "Loop" })
            .action_id("transport.loop")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &loop_core {
                    core.set_loop(!loop_on);
                }
                loop_state.set(!loop_on);
            });
        let metro_core = self.core.clone();
        let metro_state = self.metronome_enabled.clone();
        let metro_on = self.metronome_enabled.get();
        let metro_button = ButtonView::new(if metro_on { "Click On" } else { "Click" })
            .action_id("transport.metronome")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &metro_core {
                    core.set_metronome_enabled(!metro_on);
                }
                metro_state.set(!metro_on);
            });

        let transport = View::Card(vec![View::HStack(vec![
            play_button.into(),
            stop_button.into(),
            loop_button.into(),
            metro_button.into(),
            View::Text(format!("{beats:.2} beats")).font_size(18),
            View::Spacer,
            View::Text(format!("{tempo:.1} BPM")),
            tempo_slider.into(),
            View::Caption(format!("{sample_rate:.0} Hz")),
        ])]);
        transport
    }
}
