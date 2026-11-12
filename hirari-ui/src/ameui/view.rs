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
        if canvas_id == "piano" {
            return self.on_piano_pointer(event);
        }
        if canvas_id != "arrange" {
            return false;
        }
        match event.phase {
            CanvasPointerPhase::Down => {
                // Double-click splits the clip under the cursor at the
                // click beat (the landed click_count makes the second
                // press reliable instead of arming a move/seek).
                if event.click_count >= 2
                    && (event.y as f32) >= self.arrange_geometry.ruler_height
                    && (event.x as f32) >= self.arrange_geometry.header_width
                {
                    let tracks = self.tracks.get();
                    if let Some((track_id, clip_id)) = clip_at(
                        &tracks,
                        &self.arrange_geometry,
                        event.x as f32,
                        event.y as f32,
                    ) {
                        if let Some(core) = self.core.clone() {
                            let beat = f64::from(self.arrange_geometry.beat_for_x(event.x as f32));
                            if core.split_region(track_id, clip_id as u32, beat) {
                                self.selected_clip.set(Some((track_id, clip_id)));
                                self.status.set(format!("CLIP SPLIT AT {beat:.2} BEATS"));
                                self.refresh_arrange_state(&core);
                            }
                        }
                        return true;
                    }
                }
                if (event.y as f32) < self.arrange_geometry.ruler_height {
                    // Ruler drag pans; lane drags seek (slice 4).
                    self.panning = true;
                    self.pan_last_x = event.x;
                    true
                } else if (event.y as f32) >= self.arrange_geometry.ruler_height
                    && (event.x as f32) >= self.arrange_geometry.header_width
                    && self.auto_mode.get()
                {
                    // Automation mode: press near a point to move it,
                    // elsewhere on the lane to add one (parity).
                    if self.arrange_locked.get() {
                        return true;
                    }
                    let tracks = self.tracks.get();
                    let lane_index = ((event.y as f32 - self.arrange_geometry.ruler_height)
                        / self.arrange_geometry.lane_height)
                        as usize;
                    let lane = self.auto_lane.get();
                    let mut found: Option<AutoEdit> = None;
                    if let Some(track) = tracks.get(lane_index) {
                        let points: &[crate::ui_shared::arrange_model::AutomationPoint] = match lane
                        {
                            0 => &track.volume_automation,
                            _ => &track.pan_automation,
                        };
                        let local_y = event.y as f32;
                        let lane_top = self.arrange_geometry.ruler_height
                            + lane_index as f32 * self.arrange_geometry.lane_height;
                        let inner = (self.arrange_geometry.lane_height - 8.0).max(4.0);
                        let (min_value, max_value) =
                            if lane == 0 { (0.0, 2.0) } else { (-1.0, 1.0) };
                        let span = max_value - min_value;
                        let nearest = points.iter().enumerate().min_by(|(_, a), (_, b)| {
                            let da = ((a.beat - self.arrange_geometry.beat_for_x(event.x as f32))
                                * self.arrange_geometry.px_per_beat)
                                .abs();
                            let db = ((b.beat - self.arrange_geometry.beat_for_x(event.x as f32))
                                * self.arrange_geometry.px_per_beat)
                                .abs();
                            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        if let Some((point_index, point)) = nearest {
                            let py =
                                lane_top + 4.0 + inner * (1.0 - (point.value - min_value) / span);
                            if (py - local_y).abs() <= 6.0 {
                                found = Some(AutoEdit {
                                    track_id: track.id,
                                    lane,
                                    point_index: Some(point_index),
                                    down_x: event.x,
                                    down_y: event.y,
                                    orig_beat: point.beat,
                                    orig_value: point.value,
                                    delta_beats: 0.0,
                                    delta_value: 0.0,
                                });
                                self.auto_point.set(Some(point_index));
                                self.auto_track.set(track.id);
                            }
                        }
                        if found.is_none() {
                            let beat = self.arrange_geometry.beat_for_x(event.x as f32);
                            let value =
                                min_value + span * (1.0 - ((local_y - lane_top - 4.0) / inner));
                            self.auto_track.set(track.id);
                            found = Some(AutoEdit {
                                track_id: track.id,
                                lane,
                                point_index: None,
                                down_x: event.x,
                                down_y: event.y,
                                orig_beat: beat,
                                orig_value: value.clamp(min_value, max_value),
                                delta_beats: 0.0,
                                delta_value: 0.0,
                            });
                        }
                    }
                    if let Some(edit) = found {
                        self.auto_edit = Some(edit);
                        return true;
                    }
                    return false;
                } else {
                    // Provisional until the click threshold is crossed:
                    // a clip press selects immediately (parity) and
                    // arms a move; empty space arms a seek.
                    self.seeking = true;
                    self.down_x = event.x;
                    self.down_y = event.y;
                    let tracks = self.tracks.get();
                    self.down_clip = clip_at(
                        &tracks,
                        &self.arrange_geometry,
                        event.x as f32,
                        event.y as f32,
                    );
                    match self.down_clip {
                        Some((track_id, clip_id)) => {
                            self.selected_clip.set(Some((track_id, clip_id)));
                            let clip = tracks.iter().find(|track| track.id == track_id).and_then(
                                |track| track.clips.iter().find(|clip| clip.id == clip_id),
                            );
                            let track_kind = tracks
                                .iter()
                                .find(|track| track.id == track_id)
                                .map(|track| track.kind.clone());
                            match clip {
                                Some(_) if self.arrange_locked.get() => {
                                    self.down_mode = LaneDrag::Hold
                                }
                                Some(clip) if clip.midi => self.down_mode = LaneDrag::Hold,
                                Some(clip) => {
                                    // Edge zones trim (Audio/Vocal only, as
                                    // in the dock); the body moves.
                                    let edge = track_kind
                                        .as_deref()
                                        .filter(|kind| *kind == "Audio" || *kind == "Vocal")
                                        .and_then(|_| {
                                            trim_edge_at(
                                                &self.arrange_geometry,
                                                clip,
                                                event.x as f32,
                                            )
                                        });
                                    match edge {
                                        Some(TrimEdge::Left) => {
                                            self.down_mode = LaneDrag::TrimLeft;
                                            self.trim_clip = Some((track_id, clip_id));
                                            self.trim_edge = TrimEdge::Left;
                                            self.trim_delta_beats = 0.0;
                                        }
                                        Some(TrimEdge::Right) => {
                                            self.down_mode = LaneDrag::TrimRight;
                                            self.trim_clip = Some((track_id, clip_id));
                                            self.trim_edge = TrimEdge::Right;
                                            self.trim_delta_beats = 0.0;
                                        }
                                        None => {
                                            self.down_mode = LaneDrag::Move;
                                            self.move_clip = Some((track_id, clip_id));
                                            self.move_start_beat = clip.start_beat;
                                            self.move_delta_beats = 0.0;
                                        }
                                    }
                                }
                                None => self.down_mode = LaneDrag::Seek,
                            }
                        }
                        None => self.down_mode = LaneDrag::Seek,
                    }
                    true
                }
            }
            CanvasPointerPhase::Moved => {
                if self.auto_edit.is_some() {
                    if let Some(edit) = self.auto_edit.as_mut() {
                        let px_per_beat = self.arrange_geometry.px_per_beat;
                        let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                        let raw_beats = if px_per_beat > 0.0 {
                            ((event.x - edit.down_x) / f64::from(px_per_beat)) as f32
                        } else {
                            0.0
                        };
                        edit.delta_beats = (raw_beats / step).round() * step;
                        // Value delta from vertical travel, sign follows the
                        // drawn mapping (up raises the value). Both lanes
                        // span 2 value units, so the scale is uniform.
                        let inner = (self.arrange_geometry.lane_height - 8.0).max(4.0);
                        if inner > 0.0 {
                            edit.delta_value = ((edit.down_y - event.y) as f32 / inner) * 2.0;
                        }
                    }
                    return true;
                }
                if self.panning {
                    let px_per_beat = self.arrange_geometry.px_per_beat;
                    if px_per_beat > 0.0 {
                        let delta_beats =
                            ((event.x - self.pan_last_x) / f64::from(px_per_beat)) as f32;
                        self.arrange_scroll_beats
                            .set((self.arrange_scroll_beats.get() - delta_beats).max(0.0));
                    }
                    self.pan_last_x = event.x;
                    true
                } else if self.seeking {
                    match self.down_mode {
                        LaneDrag::Hold => true,
                        LaneDrag::TrimLeft | LaneDrag::TrimRight => {
                            if self.click_distance(&event) <= 4.0 {
                                true
                            } else {
                                let px_per_beat = self.arrange_geometry.px_per_beat;
                                if px_per_beat > 0.0 {
                                    let raw =
                                        ((event.x - self.down_x) / f64::from(px_per_beat)) as f32;
                                    let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                                    let snapped = (raw / step).round() * step;
                                    // Clamp the preview delta through the
                                    // same fraction math the commit uses.
                                    self.trim_delta_beats = self.clamped_trim_delta(snapped);
                                }
                                true
                            }
                        }
                        LaneDrag::Move => {
                            if self.click_distance(&event) <= 4.0 {
                                true
                            } else {
                                let px_per_beat = self.arrange_geometry.px_per_beat;
                                if px_per_beat > 0.0 {
                                    let raw =
                                        ((event.x - self.down_x) / f64::from(px_per_beat)) as f32;
                                    let step = 4.0 / self.arrange_snap_division.get().max(1.0);
                                    let snapped = (raw / step).round() * step;
                                    self.move_delta_beats = snapped.max(-self.move_start_beat);
                                }
                                true
                            }
                        }
                        LaneDrag::Seek => {
                            if self.click_distance(&event) <= 4.0 {
                                // Still a click candidate; seek on release.
                                true
                            } else {
                                self.seek_arrange(event.x)
                            }
                        }
                    }
                } else {
                    false
                }
            }
            CanvasPointerPhase::Up | CanvasPointerPhase::Cancelled => {
                if self.auto_edit.is_some() {
                    if event.phase == CanvasPointerPhase::Up {
                        self.commit_auto_edit();
                    } else {
                        self.auto_edit = None;
                    }
                    return true;
                }
                if event.phase == CanvasPointerPhase::Up {
                    match self.down_mode {
                        LaneDrag::Move => self.commit_clip_move(),
                        LaneDrag::TrimLeft | LaneDrag::TrimRight => self.commit_clip_trim(),
                        LaneDrag::Hold => {}
                        LaneDrag::Seek => {
                            if self.seeking && self.click_distance(&event) <= 4.0 {
                                // Click on empty lane space seeks on release.
                                self.seek_arrange(self.down_x);
                            }
                        }
                    }
                }
                self.seeking = false;
                self.panning = false;
                self.down_clip = None;
                self.move_clip = None;
                self.move_delta_beats = 0.0;
                self.trim_clip = None;
                self.trim_delta_beats = 0.0;
                true
            }
        }
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
        if canvas_id == "arrange" {
            self.draw_arrange(scene, bounds, context);
            return;
        }
        if canvas_id == "notation" {
            self.draw_notation(scene, bounds, context);
            return;
        }
        if canvas_id == "piano" {
            self.draw_piano(scene, bounds, context);
            return;
        }
        if canvas_id == "video" {
            let Some(frame) = self.video_frame.take() else {
                return;
            };
            let Some((width, height)) = self.video_size else {
                return;
            };
            let texture = self.video_reusable.take().unwrap_or_else(|| {
                context.create_render_target(
                    width,
                    height,
                    Some("hirari-video"),
                    wgpu::TextureUsages::COPY_DST,
                )
            });
            context.queue().write_texture(
                texture.as_image_copy(),
                &frame,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            if let Some(image) = &self.video_image {
                match context.replace_texture(image, texture) {
                    Ok(previous) => self.video_reusable = Some(previous),
                    Err(error) => {
                        eprintln!("could not refresh video canvas: {error}");
                        return;
                    }
                }
            } else {
                match context.register_texture(texture) {
                    Ok(image) => self.video_image = Some(image),
                    Err(error) => {
                        eprintln!("could not register video canvas: {error}");
                        return;
                    }
                }
            }
            if let Some(image) = &self.video_image {
                context.draw_texture(scene, image, bounds);
            }
            return;
        }
        if canvas_id == "spectrum" {
            let frame = self.plots.snapshot();
            let pixels = plot_raster::spectrum_rgba(&frame);
            let width = plot_raster::WIDTH;
            let height = 96u32;
            let texture = self.spectrum_reusable.take().unwrap_or_else(|| {
                context.create_render_target(
                    width,
                    height,
                    Some("hirari-spectrum"),
                    wgpu::TextureUsages::COPY_DST,
                )
            });
            context.queue().write_texture(
                texture.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            if let Some(image) = &self.spectrum_image {
                match context.replace_texture(image, texture) {
                    Ok(previous) => self.spectrum_reusable = Some(previous),
                    Err(error) => {
                        eprintln!("could not refresh spectrum canvas: {error}");
                        return;
                    }
                }
            } else {
                match context.register_texture(texture) {
                    Ok(image) => self.spectrum_image = Some(image),
                    Err(error) => {
                        eprintln!("could not register spectrum canvas: {error}");
                        return;
                    }
                }
            }
            if let Some(image) = &self.spectrum_image {
                context.draw_texture(scene, image, bounds);
            }
            return;
        }
        if canvas_id != "waveform" {
            return;
        }
        let frame = self.plots.snapshot();
        let pixels = plot_raster::waveform_rgba(&frame);
        let width = plot_raster::WIDTH;
        let height = plot_raster::HEIGHT;

        // Reuse the prior frame's texture so steady-state drawing allocates
        // nothing: write into it, swap it into the registered image, and keep
        // the texture the image released for the next frame.
        let texture = self.waveform_reusable.take().unwrap_or_else(|| {
            context.create_render_target(
                width,
                height,
                Some("hirari-waveform"),
                wgpu::TextureUsages::COPY_DST,
            )
        });
        context.queue().write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        if let Some(image) = &self.waveform_image {
            match context.replace_texture(image, texture) {
                Ok(previous) => self.waveform_reusable = Some(previous),
                Err(error) => {
                    eprintln!("could not update waveform canvas: {error}");
                    return;
                }
            }
        } else {
            match context.register_texture(texture) {
                Ok(image) => self.waveform_image = Some(image),
                Err(error) => {
                    eprintln!("could not register waveform canvas: {error}");
                    return;
                }
            }
        }

        if let Some(image) = &self.waveform_image {
            context.draw_texture(scene, image, bounds);
        }
    }
    fn view(&mut self) -> View {
        self.describe()
    }
}
