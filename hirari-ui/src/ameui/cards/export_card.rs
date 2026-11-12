//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn export_card_view(&self) -> View {
        // Export card (export dialog parity): format and tail
        // cyclers, mix render, stem batch, MIDI export, cancel. Stems
        // are armed by the folder-picker poll; the render poll advances
        // the batch while the engine reports bounce state 3.
        let export_format_state = self.export_format.clone();
        let format_label = render_format_label(self.export_format.get());
        let export_format_button = ButtonView::new(format!("FORMAT: {format_label}"))
            .action_id("export.format_cycle")
            .secondary()
            .on_press(move |_| {
                const FORMATS: [u32; 4] = [0, 3, 2, 1]; // PCM16, PCM24, Float32, Wave64
                let current = export_format_state.get();
                let index = FORMATS
                    .iter()
                    .position(|code| *code == current)
                    .unwrap_or(0);
                export_format_state.set(FORMATS[(index + 1) % FORMATS.len()]);
            });
        let export_tail_state = self.export_tail_index.clone();
        let tail_label = match self.export_tail_index.get() {
            0 => "NONE",
            1 => "1s",
            2 => "2s",
            3 => "5s",
            4 => "10s",
            _ => "30s",
        };
        let export_tail_button = ButtonView::new(format!("TAIL: {tail_label}"))
            .action_id("export.tail_cycle")
            .secondary()
            .on_press(move |_| {
                export_tail_state.set((export_tail_state.get() + 1) % 6);
            });
        let render_core = self.core.clone();
        let render_format = self.export_format.clone();
        let render_tail = self.export_tail_index.clone();
        let render_status = self.render_caption.clone();
        let render_button = ButtonView::new("RENDER")
            .action_id("export.render")
            .primary()
            .on_press(move |_| {
                let Some(core) = &render_core else { return };
                let path = default_render_output_path();
                core.set_offline_render_tail_seconds(render_tail_seconds(render_tail.get() as i32));
                if core.start_render_async_to_format(&path.to_string_lossy(), render_format.get()) {
                    render_status.set(format!(
                        "RENDER QUEUED: {}",
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ));
                } else {
                    render_status.set("RENDER COULD NOT BE QUEUED".to_owned());
                }
            });
        let stems_pending = self.pending_stems.clone();
        let stems_button = ButtonView::new("STEMS")
            .action_id("export.stems")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new();
                let pending = stems_pending.clone();
                dialog.pick_folder_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let musicxml_pending = self.pending_musicxml.clone();
        let musicxml_button = ButtonView::new("MUSICXML")
            .action_id("export.musicxml")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .set_file_name("project.musicxml")
                    .add_filter("MusicXML", &["musicxml", "xml"]);
                let pending = musicxml_pending.clone();
                dialog.save_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let midi_pending = self.pending_midi.clone();
        let midi_button = ButtonView::new("MIDI")
            .action_id("export.midi")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .set_file_name("project.mid")
                    .add_filter("MIDI file", &["mid", "midi"]);
                let pending = midi_pending.clone();
                dialog.save_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let cancel_core = self.core.clone();
        let cancel_status = self.render_caption.clone();
        let cancel_button = ButtonView::new("CANCEL")
            .action_id("export.cancel")
            .secondary()
            .on_press(move |_| {
                if let Some(core) = &cancel_core {
                    let (_, message, _) = cancellation_status(core.cancel_render());
                    cancel_status.set(message.to_owned());
                }
            });
        let export_card = View::Card(vec![View::HStack(vec![
            View::Text("EXPORT".to_owned()),
            export_format_button.into(),
            export_tail_button.into(),
            render_button.into(),
            stems_button.into(),
            midi_button.into(),
            musicxml_button.into(),
            cancel_button.into(),
            View::Caption(self.render_caption.get()),
        ])]);

        export_card
    }
}
