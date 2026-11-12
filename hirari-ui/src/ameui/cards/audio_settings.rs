//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn audio_settings_view(&self) -> View {
        let sample_rate = self.sample_rate.get();
        // Audio settings (the audio settings parity): sample-rate and
        // buffer cyclers plus reconnect, read from the engine each poll.
        let audio_ready = self.audio_ready.get();
        let audio_devices = self.device_count.get();
        let audio_buffer = self.audio_buffer.get();
        let rate_core = self.core.clone();
        let rate_state = self.audio_buffer.clone();
        let rate_sample_rate = self.sample_rate.clone();
        let rate_status = self.status.clone();
        let rate_button = ButtonView::new(format!("{:.0} kHz", sample_rate / 1000.0))
            .action_id("audio.sample_rate_cycle")
            .secondary()
            .on_press(move |_| {
                const RATES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];
                let Some(core) = &rate_core else { return };
                let current = rate_sample_rate.get() as u32;
                let next = RATES
                    .iter()
                    .copied()
                    .find(|rate| *rate > current)
                    .unwrap_or(RATES[0]);
                if core.apply_audio_config(next, rate_state.get()) {
                    rate_status.set(format!("AUDIO {} Hz", next));
                } else {
                    rate_status.set("AUDIO CONFIG FAILED".to_owned());
                }
            });
        let buffer_core = self.core.clone();
        let buffer_state = self.audio_buffer.clone();
        let buffer_sample_rate = self.sample_rate.clone();
        let buffer_status = self.status.clone();
        let buffer_button = ButtonView::new(format!("{audio_buffer} spl"))
            .action_id("audio.buffer_cycle")
            .secondary()
            .on_press(move |_| {
                const SIZES: [u32; 7] = [32, 64, 128, 256, 512, 1024, 2048];
                let Some(core) = &buffer_core else { return };
                let next = SIZES
                    .iter()
                    .copied()
                    .find(|size| *size > buffer_state.get())
                    .unwrap_or(SIZES[0]);
                buffer_state.set(next);
                let rate = buffer_sample_rate.get().max(1.0) as u32;
                if core.apply_audio_config(rate, next) {
                    buffer_status.set(format!("AUDIO BUFFER {}", next));
                } else {
                    buffer_status.set("AUDIO CONFIG FAILED".to_owned());
                }
            });
        let reconnect_core = self.core.clone();
        let reconnect_button = ButtonView::new("RECONNECT")
            .action_id("audio.reconnect")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &reconnect_core {
                    core.try_reconnect_audio_device();
                }
            });
        // Device cycle + select (on_select_device): the cycle picks
        // a pending index, USE applies it at the current config values.
        let device_list = self.audio_devices.get();
        let pending_index = self
            .device_index
            .get()
            .min(device_list.len().saturating_sub(1) as u32);
        let device_name = device_list
            .get(pending_index as usize)
            .map(|device| device.name.clone())
            .unwrap_or_else(|| "no device".to_owned());
        let cycle_state = self.device_index.clone();
        let device_count_for_cycle = device_list.len() as u32;
        let device_button = ButtonView::new(format!("DEVICE: {device_name}"))
            .action_id("audio.device_cycle")
            .secondary()
            .on_press(move |_| {
                if device_count_for_cycle > 0 {
                    cycle_state.set((cycle_state.get() + 1) % device_count_for_cycle);
                }
            });
        let use_core = self.core.clone();
        let use_devices = self.audio_devices.clone();
        let use_index = self.device_index.clone();
        let use_buffer = self.audio_buffer.clone();
        let use_sample_rate = self.sample_rate.clone();
        let use_status = self.status.clone();
        let use_button = ButtonView::new("USE")
            .action_id("audio.device_select")
            .secondary()
            .on_press(move |_| {
                let Some(core) = &use_core else { return };
                let devices = use_devices.get();
                let index = use_index.get().min(devices.len().saturating_sub(1) as u32) as usize;
                let Some(device) = devices.get(index) else {
                    return;
                };
                let sample_rate = match use_sample_rate.get().round() as u32 {
                    44_100 | 48_000 | 88_200 | 96_000 | 192_000 => {
                        use_sample_rate.get().round() as u32
                    }
                    _ => 48_000,
                };
                let buffer_size = match use_buffer.get() {
                    32 | 64 | 128 | 256 | 512 | 1024 | 2048 => use_buffer.get(),
                    _ => 256,
                };
                if core.select_audio_device(device.id, sample_rate, buffer_size) {
                    use_status.set(format!("AUDIO DEVICE SELECTED: {}", device.name));
                } else {
                    use_status.set("DEVICE SELECTION FAILED".to_owned());
                }
            });
        let audio_card = View::Card(vec![View::HStack(vec![
            View::Text("AUDIO".to_owned()),
            device_button.into(),
            use_button.into(),
            rate_button.into(),
            buffer_button.into(),
            reconnect_button.into(),
            View::Caption(format!(
                "{} devices · {} · {:.1}ms",
                audio_devices,
                if audio_ready { "READY" } else { "NO DEVICE" },
                self.io_latency_ms.get()
            )),
        ])]);

        audio_card
    }
}
