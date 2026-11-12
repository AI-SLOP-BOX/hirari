//! View composition for the AmeUI shell.
use super::*;

impl HirariAmeApp {
    fn describe(&mut self) -> View {
        let project_bar = self.project_bar_view();
        if self.show_genesis.get() {
            let mut template_buttons = Vec::new();
            for (label, template) in [
                ("ELECTRONIC", "ELECTRONIC"),
                ("CINEMATIC SCORE", "CINEMATIC SCORE"),
                ("ATMOS MASTERING", "ATMOS MASTERING"),
                ("VOICE-OVER PRO", "VOICE-OVER PRO"),
            ] {
                let template_core = self.core.clone();
                let template_selected = self.selected_clip.clone();
                let template_status = self.status.clone();
                let template_show = self.show_genesis.clone();
                let template_name = template.to_string();
                let button = ButtonView::new(label)
                    .action_id(format!("genesis.template.{label}"))
                    .secondary()
                    .on_press(move |_| {
                        let Some(core) = &template_core else { return };
                        match create_template_project(core, &template_name) {
                            Ok(()) => {
                                template_selected.set(None);
                                template_show.set(false);
                                template_status.set(format!("STARTED {template_name}"));
                            }
                            Err(error) => {
                                template_status.set(format!("TEMPLATE FAILED: {error}"));
                            }
                        }
                    });
                template_buttons.push(button.into());
            }
            let blank_core = self.core.clone();
            let blank_selected = self.selected_clip.clone();
            let blank_status = self.status.clone();
            let blank_show = self.show_genesis.clone();
            let blank_button = ButtonView::new("BLANK")
                .action_id("genesis.template.BLANK")
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &blank_core {
                        core.new_project();
                        blank_selected.set(None);
                        blank_show.set(false);
                        blank_status.set("NEW PROJECT".to_owned());
                    }
                });
            template_buttons.push(blank_button.into());
            let mut recent_buttons: Vec<View> = Vec::new();
            for project in crate::ui_shared::recent_projects::load_recent_projects() {
                let open_pending = self.pending_open.clone();
                let path = project.path.to_string();
                let file_name = std::path::Path::new(&path)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("Project")
                    .to_owned();
                let button = ButtonView::new(file_name)
                    .action_id("genesis.recent")
                    .secondary()
                    .on_press(move |_| {
                        if let Ok(mut slot) = open_pending.lock() {
                            *slot = Some(std::path::PathBuf::from(path.clone()));
                        }
                    });
                recent_buttons.push(button.into());
            }
            let genesis = View::Card(vec![
                View::Text("HIRARI".to_owned()),
                View::Caption("Pick a starting point or open a recent project".to_owned()),
                View::HStack(template_buttons),
                View::Caption("RECENT PROJECTS".to_owned()),
                View::HStack(recent_buttons),
            ]);
            return View::VStack(vec![project_bar, genesis]);
        }
        let export_card = self.export_card_view();
        let diagnostics_card = self.diagnostics_card_view();
        let moufu_card = self.moufu_card_view();
        let video_card = self.video_card_view();
        let tempo_card = self.tempo_card_view();
        let openutau_card = self.openutau_card_view();
        let synth_card = self.synth_card_view();
        let sends_card = self.sends_card_view();
        let step_seq_card = self.step_seq_card_view();
        let control_room_card = self.control_room_card_view();
        let chord_lane_card = self.chord_lane_card_view();
        let midi_learn_card = self.midi_learn_card_view();
        let audio_edit_card = self.audio_edit_card_view();
        let flex_card = self.flex_card_view();
        let stacks_card = self.stacks_card_view();
        let comping_card = self.comping_card_view();
        let transport = self.transport_view();
        let meter = self.meter_view();
        let master_card = self.master_card_view();
        let audio_card = self.audio_settings_view();
        let arrange_toolbar = self.arrange_toolbar_view();
        let inspector = self.clip_inspector_view();
        let plugin_strip = self.plugin_strip_view();
        let track_list = self.mixer_strips_view();
        let piano_selector = self.piano_selector_view();
        let arrange_timeline = self.timeline_view();
        let markers_card = self.markers_card_view();
        let spotlight_card = self.spotlight_card_view();

        let mut views = vec![project_bar];
        if self.show_diagnostics.get() {
            views.push(diagnostics_card);
        }
        if self.show_moufu.get() {
            views.push(moufu_card);
        }
        if self.show_control_room.get() {
            views.push(control_room_card);
        }
        if self.show_chords.get() {
            views.push(chord_lane_card);
        }
        if self.show_midi.get() {
            views.push(midi_learn_card);
        }
        if self.show_step_seq.get() {
            views.push(step_seq_card);
        }
        if self.show_sends.get() {
            views.push(sends_card);
        }
        views.push(synth_card);
        views.push(openutau_card);
        views.push(audio_edit_card);
        if self.show_spotlight.get() {
            views.insert(1, spotlight_card);
        }
        if self.focus_mode.get() {
            views.extend([
                transport,
                meter,
                View::canvas("waveform").frame(None, Some(168)),
                arrange_timeline,
                View::canvas("arrange").grow(3),
            ]);
        } else {
            views.extend([
                export_card,
                tempo_card,
                flex_card,
                stacks_card,
                comping_card,
                transport,
                meter,
                master_card,
                markers_card,
                audio_card,
                video_card,
                arrange_toolbar,
                View::canvas("waveform").frame(None, Some(168)),
                if self.show_spectrum.get() {
                    View::canvas("spectrum").frame(None, Some(100))
                } else {
                    View::Caption(String::new())
                },
                if self.show_notation.get() {
                    View::canvas("notation").frame(None, Some(240))
                } else {
                    View::Caption(String::new())
                },
                if self.show_video.get() {
                    View::canvas("video").frame(None, Some(200))
                } else {
                    View::Caption(String::new())
                },
                arrange_timeline,
                View::canvas("arrange").grow(3),
            ]);
        }
        if let Some(inspector) = inspector {
            views.push(inspector);
        }
        if let Some(plugin_strip) = plugin_strip {
            views.push(plugin_strip);
        }
        let tracks = self.tracks.get();
        // Plugin parameter strip for the selected plugin of the selected
        // track (normalized 0..1 sliders, as in the plugin UI).
        if let Some(core) = &self.core {
            let params_track = tracks
                .iter()
                .find(|track| track.id == self.piano_track.get());
            if let Some(track) = params_track {
                let params_track_id = track.id;
                let plugin_count = track.plugin_types.len() as u32;
                if plugin_count > 0 {
                    let plugin_index = self.selected_plugin.get().min(plugin_count - 1);
                    let count = core
                        .get_plugin_parameter_count(params_track_id, plugin_index)
                        .min(32);
                    if count > 0 {
                        let param_locked = self.arrange_locked.get();
                        let mut param_rows: Vec<View> = vec![View::Text(format!(
                            "PARAMS · {}",
                            plugin_name(
                                track.plugin_types[plugin_index as usize],
                                plugin_index as usize
                            )
                        ))];
                        for parameter_id in 0..count {
                            let name = core.get_plugin_parameter_name(
                                params_track_id,
                                plugin_index,
                                parameter_id,
                            );
                            let label = if name.trim().is_empty() {
                                format!("Parameter {parameter_id}")
                            } else {
                                name
                            };
                            let value = core
                                .get_plugin_parameter(params_track_id, plugin_index, parameter_id)
                                .clamp(0.0, 1.0);
                            let param_core = self.core.clone();
                            let slider = SliderView::new(value, 0.0, 1.0)
                                .action_id(format!("plugin.param.{plugin_index}.{parameter_id}"))
                                .disabled(param_locked)
                                .on_change(move |new_value| {
                                    if param_locked {
                                        return;
                                    }
                                    if let Some(core) = &param_core {
                                        let _ = core.set_plugin_parameter(
                                            params_track_id,
                                            plugin_index,
                                            parameter_id,
                                            new_value.clamp(0.0, 1.0),
                                        );
                                    }
                                });
                            param_rows
                                .push(View::HStack(vec![View::Caption(label), slider.into()]));
                        }
                        // Preset save/load for this plugin
                        // on_save_preset/on_load_preset via native dialogs).
                        let preset_pending = self.pending_preset.clone();
                        let preset_track = params_track_id;
                        let preset_plugin = plugin_index;
                        for (label, is_save) in [("SAVE PRESET", true), ("LOAD PRESET", false)] {
                            let pending = preset_pending.clone();
                            let preset_button = ButtonView::new(label)
                                .action_id(format!(
                                    "plugin.preset.{}",
                                    if is_save { "save" } else { "load" }
                                ))
                                .secondary()
                                .on_press(move |_| {
                                    let dialog = ame_ui::platform::NativeFileDialog::new()
                                        .add_filter("Hirari Plugin Preset", &["aupreset"]);
                                    let pending = pending.clone();
                                    if is_save {
                                        dialog.save_file_async(move |path| {
                                            if let Some(path) = path {
                                                if let Ok(mut slot) = pending.lock() {
                                                    *slot = Some((
                                                        path,
                                                        true,
                                                        preset_track,
                                                        preset_plugin,
                                                    ));
                                                }
                                            }
                                        });
                                    } else {
                                        dialog.pick_file_async(move |path| {
                                            if let Some(path) = path {
                                                if let Ok(mut slot) = pending.lock() {
                                                    *slot = Some((
                                                        path,
                                                        false,
                                                        preset_track,
                                                        preset_plugin,
                                                    ));
                                                }
                                            }
                                        });
                                    }
                                });
                            param_rows.push(preset_button.into());
                        }
                        views.push(View::Card(vec![View::VStack(param_rows)]));
                    }
                }
            }
        }
        views.extend([piano_selector, View::canvas("piano").grow(2), track_list]);

        // Audio library: folder scan, search
        // filter, per-asset preview and import to the piano track.
        let library_assets = self.library_assets.get();
        let library_filter = self.library_query.get().to_lowercase();
        let scan_pending = self.pending_library.clone();
        let scan_button = ButtonView::new("SCAN")
            .action_id("library.scan")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new();
                let pending = scan_pending.clone();
                dialog.pick_folder_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });
        let search_state = self.library_query.clone();
        let library_search = TextFieldView::new(library_filter.clone(), "Search sounds")
            .change_action_id("library.search")
            .submit_action_id("library.search")
            .on_change(move |value| {
                search_state.set(value);
            });
        let mut library_rows: Vec<(ame_ui::view::StableKey, View)> = Vec::new();
        for asset in library_assets.iter().filter(|asset| {
            library_filter.is_empty() || asset.name.to_lowercase().contains(&library_filter)
        }) {
            let play_core = self.core.clone();
            let play_path = asset.path.clone();
            let play_button = ButtonView::new("PLAY")
                .action_id(format!("library.preview.{}", asset.id))
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &play_core {
                        let _ = core.register_preview_audio(&play_path);
                    }
                });
            let import_core = self.core.clone();
            let import_path = asset.path.clone();
            let import_track = self.piano_track.clone();
            let import_playhead = self.playhead_beats.clone();
            let import_status = self.status.clone();
            let import_ticket_slot = self.library_ticket.clone();
            let import_button = ButtonView::new("IMPORT")
                .action_id(format!("library.import.{}", asset.id))
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &import_core {
                        let track_id = import_track.get();
                        if track_id == 0 {
                            return;
                        }
                        let ticket = core.queue_region_import_at_beat(
                            track_id,
                            &import_path,
                            import_playhead.get(),
                        );
                        if ticket == 0 {
                            import_status.set("IMPORT FAILED".to_owned());
                        } else {
                            if let Ok(mut slot) = import_ticket_slot.lock() {
                                *slot = Some((ticket, import_path.clone(), track_id));
                            }
                            import_status.set("IMPORTING: Audio".to_owned());
                        }
                    }
                });
            library_rows.push((
                ame_ui::view::StableKey::from(asset.id),
                View::HStack(vec![
                    View::Text(asset.name.clone()),
                    View::Caption(format!(
                        "{} Hz · {} bytes",
                        asset.sample_rate as u32, asset.size_bytes
                    )),
                    play_button.into(),
                    import_button.into(),
                ]),
            ));
        }
        views.push(View::Card(vec![
            View::HStack(vec![
                View::Text("AUDIO LIBRARY".to_owned()),
                scan_button.into(),
                library_search.into(),
            ]),
            // Variable-height virtual list: large sound folders stay
            // scrollable without laying out every row up front.
            View::virtual_list_variable_height(140, 32, 4, library_rows),
        ]));
        View::VStack(views)
    }
}

impl App for HirariAmeApp {
    fn window_config(&self) -> WindowConfig {
        WindowConfig {
            title: "Hirari (AmeUI)".to_owned(),
            width: 1280.0,
            height: 800.0,
            min_width: 900.0,
            min_height: 600.0,
            ..WindowConfig::default()
        }
    }

    fn full_bleed_layout(&self) -> bool {
        true
    }

    fn content_max_width(&self) -> Option<f32> {
        None
    }

    fn theme(&self) -> Theme {
        Theme::dark()
    }

    fn on_event_loop_ready(&mut self, sender: AppEventSender) {
        // The ticker never touches the engine; it only nudges the UI thread.
        // When the loop closes the proxy disconnects and `send` returns false.
        std::thread::spawn(move || {
            while sender.send("tick") {
                std::thread::sleep(Duration::from_millis(33));
            }
        });
    }

    fn on_event(&mut self, event: &AmeEvent) {
        match event {
            AmeEvent::AppMessage(message) => {
                if message == "tick" {
                    self.poll_engine();
                }
            }
            AmeEvent::FileDropped { path, x, y } => {
                self.on_file_dropped(path, *x, *y);
            }
            _ => {}
        }
    }

    /// App-wide shortcuts (parity: Space plays, L loops). The
    /// framework only calls this with no text editor focused, so the
    /// lyric field keeps Space/L as text input.
    fn on_shortcut(&mut self, key: &str, modifiers: KeyModifiers) -> bool {
        if (modifiers.control || modifiers.super_key) && matches!(key, "k" | "K") {
            self.show_spotlight.set(!self.show_spotlight.get());
            return true;
        }
        if modifiers.control || modifiers.alt || modifiers.super_key {
            return false;
        }
        match key {
            "Named(Space)" => {
                if let Some(core) = &self.core {
                    let playing = !self.playing.get();
                    core.set_playing(playing);
                    self.playing.set(playing);
                }
                true
            }
            "l" | "L" => {
                if let Some(core) = &self.core {
                    let loop_on = !self.loop_enabled.get();
                    core.set_loop(loop_on);
                    self.loop_enabled.set(loop_on);
                }
                true
            }
            _ => false,
        }
    }

    fn on_canvas_scroll(&mut self, canvas_id: &str, event: CanvasScrollEvent) -> bool {
        if canvas_id != "arrange" {
            return false;
        }
        let geometry = self.arrange_geometry;
        if geometry.px_per_beat <= 0.0 {
            return false;
        }
        if event.modifiers.control || event.modifiers.super_key {
            // Zoom anchored at the pointer beat, same factor curve as the
            // timeline ruler's wheel handling.
            let factor = (1.0 - event.delta_y / 400.0).clamp(0.25, 4.0) as f32;
            self.zoom_arrange_at(event.x as f32, factor);
        } else {
            // Plain wheel scrolls the timeline (DAW convention), matching
            // the timeline ruler's scroll direction and units.
            let timeline_end = self.timeline_end_beats.get();
            let slot_width = (self.arrange_bounds.2 - 104.0).max(0.0);
            let visible = if geometry.px_per_beat > 0.0 {
                f64::from(slot_width / geometry.px_per_beat)
            } else {
                f64::from(timeline_end)
            };
            let max_offset = (f64::from(timeline_end) - visible).max(0.0);
            let delta = if event.delta_x != 0.0 {
                event.delta_x
            } else {
                event.delta_y
            };
            let offset = f64::from(self.arrange_scroll_beats.get())
                - delta / f64::from(geometry.px_per_beat);
            self.arrange_scroll_beats
                .set(offset.clamp(0.0, max_offset) as f32);
        }
        true
    }

    fn on_canvas_gesture(&mut self, canvas_id: &str, event: CanvasGestureEvent) -> bool {
        if canvas_id != "arrange" {
            return false;
        }
        match event.gesture {
            CanvasGestureKind::Pinch { delta, .. } => {
                self.zoom_arrange_at(event.x as f32, (1.0 + delta) as f32);
                true
            }
            CanvasGestureKind::Pan { delta_x, .. } => {
                let px_per_beat = self.arrange_geometry.px_per_beat;
                if px_per_beat > 0.0 {
                    let delta_beats = (delta_x / f64::from(px_per_beat)) as f32;
                    self.arrange_scroll_beats
                        .set((self.arrange_scroll_beats.get() - delta_beats).max(0.0));
                }
                true
            }
            CanvasGestureKind::Rotate { .. } => false,
        }
    }

    fn view(&mut self) -> View {
        self.describe()
    }

    /// Redraws canvases only when their inputs changed, using the landed
    /// dirty-canvas API: the waveform always animates (meters), while
    /// arrange/piano replay their cached scenes when idle. Geometry and
    /// scale changes re-record regardless (framework behavior).
    /// Position-aware cursor: resize over Audio/Vocal trim edges, pointer
    /// over clips/notes, default elsewhere. Uses the landed per-region
    /// cursor API (previously a static per-slot icon).
    fn canvas_cursor(&self, canvas_id: &str, x: f64, y: f64) -> Option<ame_ui::CursorIcon> {
        match canvas_id {
            "notation" => None,
            "arrange" => {
                let geometry = self.arrange_geometry;
                if geometry.lane_height <= 0.0 || (y as f32) < geometry.ruler_height {
                    return None;
                }
                let lane = ((y as f32 - geometry.ruler_height) / geometry.lane_height) as usize;
                let tracks = self.tracks.get();
                let track = tracks.get(lane)?.clone();
                let hit = clip_at(&tracks, &geometry, x as f32, y as f32);
                let Some((_, clip_id)) = hit else {
                    return None;
                };
                let clip = track.clips.iter().find(|clip| clip.id == clip_id)?;
                if clip.midi {
                    return Some(ame_ui::CursorIcon::Pointer);
                }
                if (track.kind == "Audio" || track.kind == "Vocal")
                    && trim_edge_at(&geometry, clip, x as f32).is_some()
                {
                    return Some(ame_ui::CursorIcon::EwResize);
                }
                Some(ame_ui::CursorIcon::Pointer)
            }
            "piano" => {
                let geometry = self.piano_geometry;
                if geometry.px_per_pitch <= 0.0 {
                    return None;
                }
                let track_id = self.piano_track.get();
                let notes = self.piano_notes.get();
                let visible: Vec<PianoNote> = notes
                    .iter()
                    .filter(|note| note.track_id == track_id)
                    .cloned()
                    .collect();
                if note_at(&visible, &geometry, x as f32, y as f32).is_some() {
                    Some(ame_ui::CursorIcon::Pointer)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Declares the arrange/piano elements as screen-reader regions,
    /// using the landed canvas accessibility API (bounds are slot-local,
    /// matching pointer coordinates).
    fn canvas_accessibility_items(&self, canvas_id: &str) -> Vec<CanvasAccessibilityItem> {
        const MAX_ITEMS: usize = 512;
        match canvas_id {
            "notation" => Vec::new(),
            "arrange" => {
                let geometry = self.arrange_geometry;
                let tracks = self.tracks.get();
                let selected = self.selected_clip.get();
                let mut items = Vec::new();
                for (index, track) in tracks.iter().enumerate() {
                    let top = geometry.y_for_lane(index);
                    items.push(CanvasAccessibilityItem {
                        label: track.name.clone(),
                        role: CanvasItemRole::Label,
                        x: 0.0,
                        y: top,
                        width: geometry.header_width,
                        height: geometry.lane_height,
                        value: Some(format!("{} · {} clips", track.kind, track.clips.len())),
                        selected: None,
                    });
                    for clip in &track.clips {
                        if items.len() >= MAX_ITEMS {
                            return items;
                        }
                        let start = geometry.x_for_beat(clip.start_beat);
                        let width = (clip.length_beats * geometry.px_per_beat).max(4.0) - 2.0;
                        let height = (geometry.lane_height - 4.0).clamp(2.0, 42.0);
                        let y = top + (geometry.lane_height - height) / 2.0;
                        items.push(CanvasAccessibilityItem {
                            label: clip.name.clone(),
                            role: CanvasItemRole::Button,
                            x: start,
                            y,
                            width,
                            height,
                            value: None,
                            selected: Some(selected == Some((track.id, clip.id))),
                        });
                    }
                }
                items
            }
            "piano" => {
                let geometry = self.piano_geometry;
                let track_id = self.piano_track.get();
                let selected = self.selected_note.get();
                const PITCH_NAMES: [&str; 12] = [
                    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
                ];
                self.piano_notes
                    .get()
                    .iter()
                    .enumerate()
                    .filter(|(_, note)| note.track_id == track_id)
                    .take(MAX_ITEMS)
                    .map(|(index, note)| CanvasAccessibilityItem {
                        label: format!(
                            "Note {}{} velocity {}",
                            PITCH_NAMES[(note.pitch % 12) as usize],
                            note.pitch / 12 - 1,
                            note.velocity
                        ),
                        role: CanvasItemRole::Button,
                        x: geometry.x_for_beat(note.start_beat),
                        y: geometry.y_for_pitch(note.pitch),
                        width: (note.length_beats * geometry.px_per_beat).max(2.0),
                        height: geometry.px_per_pitch.max(1.0),
                        value: None,
                        selected: Some(selected == Some(index)),
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn canvas_scene_dirty(&self, canvas_id: &str) -> bool {
        match canvas_id {
            "waveform" | "notation" | "spectrum" => true,
            "arrange" | "piano" => {
                let playhead = self.playhead_beats.get();
                let zoom = self.arrange_zoom.get();
                let scroll = self.arrange_scroll_beats.get();
                let selected = self.selected_clip.get();
                let piano = (self.piano_track.get(), self.dirty_piano.get().1);
                let dirty = playhead != self.dirty_playhead.get()
                    || zoom != self.dirty_zoom.get()
                    || scroll != self.dirty_scroll.get()
                    || selected != self.dirty_selected.get()
                    || piano != self.dirty_piano.get()
                    || self.dirty_layout.get() != self.dirty_layout_seen.get();
                self.dirty_playhead.set(playhead);
                self.dirty_zoom.set(zoom);
                self.dirty_scroll.set(scroll);
                self.dirty_selected.set(selected);
                self.dirty_piano.set(piano);
                self.dirty_layout_seen.set(self.dirty_layout.get());
                dirty
            }
            _ => true,
        }
    }

    fn on_canvas_pointer_event(&mut self, canvas_id: &str, event: CanvasPointerEvent) -> bool {
        self.canvas_pointer_event_impl(canvas_id, event)
    }

    fn draw_canvas(
        &mut self,
        canvas_id: &str,
        scene: &mut vello::Scene,
        bounds: CanvasBounds,
        context: &mut CanvasContext<'_>,
        _viewport: Viewport,
        _tree: &ViewTree,
    ) {
        self.draw_canvas_impl(canvas_id, scene, bounds, context, _viewport, _tree)
    }
}
