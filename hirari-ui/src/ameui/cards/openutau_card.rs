//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn openutau_card_view(&self) -> View {
        // OpenUtau vocal card (OpenUtau editor parity): score pick
        // + launch + render pick + import into a new Vocal track.
        let open_score_source = self.openutau_source.clone();
        let open_score_caption = self.openutau_caption.clone();
        let open_score_button = ButtonView::new("OPEN USTX")
            .action_id("openutau.open")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("OpenUtau score", &["ustx", "ust"]);
                let pending = open_score_source.clone();
                let caption = open_score_caption.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        let name = path
                            .file_name()
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                        caption.set(format!("SCORE: {name}"));
                    }
                });
            });
        let open_score_source = self.openutau_source.clone();
        let open_score_caption = self.openutau_caption.clone();
        let launch_openutau = ButtonView::new("LAUNCH OPENUTAU")
            .action_id("openutau.launch")
            .secondary()
            .on_press(move |_| {
                let source = open_score_source
                    .lock()
                    .map(|mut slot| slot.take())
                    .unwrap_or_default();
                let Some(source) = source else {
                    open_score_caption.set("PICK A SCORE FIRST".to_owned());
                    return;
                };
                if let Err(error) =
                    hirari_core_bridge::openutau::validate_source(&source.to_string_lossy())
                {
                    open_score_caption.set(format!("INVALID SCORE · {error}"));
                    return;
                }
                let app = hirari_core_bridge::openutau::application_path();
                if !app.is_dir() {
                    open_score_caption.set(format!("OPENUTAU NOT FOUND · {}", app.display()));
                    return;
                }
                #[cfg(target_os = "macos")]
                {
                    match std::process::Command::new("open")
                        .arg("-a")
                        .arg(&app)
                        .arg(&source)
                        .spawn()
                    {
                        Ok(_) => {
                            open_score_caption.set(format!("OPENUTAU LAUNCHED · {}", app.display()))
                        }
                        Err(error) => {
                            open_score_caption.set(format!("OPENUTAU LAUNCH FAILED: {error}"))
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    open_score_caption
                        .set("OPENUTAU LAUNCH IS MAC-ONLY · RENDER MUST BE LINKED BY HAND".into());
                }
            });
        let open_render_source = self.openutau_render.clone();
        let open_render_caption = self.openutau_caption.clone();
        let pick_render_button = ButtonView::new("PICK RENDER")
            .action_id("openutau.pick_render")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("Rendered vocal", &["wav", "aif", "aiff"]);
                let pending = open_render_source.clone();
                let caption = open_render_caption.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        let name = path
                            .file_name()
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                        caption.set(format!("RENDER: {name}"));
                    }
                });
            });
        let import_source = self.openutau_source.clone();
        let import_render = self.openutau_render.clone();
        let import_pending = self.pending_openutau.clone();
        let import_caption = self.openutau_caption.clone();
        let import_button = ButtonView::new("IMPORT VOCAL")
            .action_id("openutau.import")
            .primary()
            .on_press(move |_| {
                let source = import_source
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_default();
                let render = import_render
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_default();
                let (Some(source), Some(render)) = (source, render) else {
                    import_caption.set("PICK BOTH A SCORE AND A RENDER FIRST".to_owned());
                    return;
                };
                if let Ok(mut slot) = import_pending.lock() {
                    *slot = Some((source, render));
                }
                import_caption.set("VOCAL IMPORT QUEUED".to_owned());
            });
        // Tuning sliders — normalized 0..=1 matching the engine contract.
        let scoop_state = self.openutau_scoop.clone();
        let vibrato_state = self.openutau_vibrato.clone();
        let dynamics_state = self.openutau_dynamics.clone();
        let consonants_state = self.openutau_consonants.clone();
        let scoop_slider = SliderView::new(scoop_state.get(), 0.0, 1.0)
            .action_id("openutau.scoop")
            .on_change(move |value| {
                scoop_state.set(value.clamp(0.0, 1.0));
            });
        let vibrato_slider = SliderView::new(vibrato_state.get(), 0.0, 1.0)
            .action_id("openutau.vibrato")
            .on_change(move |value| {
                vibrato_state.set(value.clamp(0.0, 1.0));
            });
        let dynamics_slider = SliderView::new(dynamics_state.get(), 0.0, 1.0)
            .action_id("openutau.dynamics")
            .on_change(move |value| {
                dynamics_state.set(value.clamp(0.0, 1.0));
            });
        let consonants_slider = SliderView::new(consonants_state.get(), 0.0, 1.0)
            .action_id("openutau.consonants")
            .on_change(move |value| {
                consonants_state.set(value.clamp(0.0, 1.0));
            });
        let apply_core = self.core.clone();
        let apply_source = self.openutau_source.clone();
        let apply_render = self.openutau_render.clone();
        let apply_scoop = self.openutau_scoop.clone();
        let apply_vibrato = self.openutau_vibrato.clone();
        let apply_dynamics = self.openutau_dynamics.clone();
        let apply_consonants = self.openutau_consonants.clone();
        let apply_caption = self.openutau_caption.clone();
        let apply_button = ButtonView::new("APPLY TUNING")
            .action_id("openutau.apply_tuning")
            .primary()
            .on_press(move |_| {
                let Some(core) = &apply_core else { return };
                let source = apply_source
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_default();
                let render = apply_render
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_default();
                let (Some(source), Some(render)) = (source, render) else {
                    apply_caption.set("PICK BOTH A SCORE AND A RENDER FIRST".to_owned());
                    return;
                };
                match core.set_openutau_tuning(
                    &source.to_string_lossy(),
                    &render.to_string_lossy(),
                    apply_scoop.get(),
                    apply_vibrato.get(),
                    apply_dynamics.get(),
                    apply_consonants.get(),
                ) {
                    Ok(()) => apply_caption.set("VOCAL TUNING SAVED".to_owned()),
                    Err(error) => apply_caption.set(format!("TUNING FAILED: {error}")),
                }
            });
        let openutau_card = View::Card(vec![View::VStack(vec![
            View::HStack(vec![
                View::Text("VOCAL SCORE".to_owned()),
                open_score_button.into(),
                launch_openutau.into(),
                pick_render_button.into(),
                import_button.into(),
                View::Caption(self.openutau_caption.get()),
            ]),
            View::HStack(vec![
                View::Caption("SCOOP".to_owned()),
                scoop_slider.into(),
                View::Caption("VIBRATO".to_owned()),
                vibrato_slider.into(),
                View::Caption("DYNAMICS".to_owned()),
                dynamics_slider.into(),
                View::Caption("CONSONANTS".to_owned()),
                consonants_slider.into(),
                apply_button.into(),
            ]),
        ])]);

        openutau_card
    }
}
