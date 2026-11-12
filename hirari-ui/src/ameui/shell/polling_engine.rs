//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    pub(in crate::ameui) fn poll_engine(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        self.poll_render();
        self.poll_moufu();
        self.poll_track_stacks();
        if self.show_midi.get() {
            self.poll_midi_learn(&core);
        } else if self.midi_input_started {
            self.stop_midi_learn(&core);
            self.midi_status.set("MIDI INPUT OFF".to_owned());
        }
        let snapshot = core.comping_snapshot_json();
        self.comping_takes.set(parse_comping_takes(&snapshot));
        if let Some(path) = self
            .pending_video_load
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            if core.load_video(&path.to_string_lossy()) {
                self.status.set(format!(
                    "VIDEO LOADED: {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.to_string_lossy().into_owned())
                ));
            } else {
                self.status.set("VIDEO LOAD FAILED".to_owned());
            }
        }
        self.poll_video();
        // Diagnostics counters accumulate from the engine every tick.
        self.watchdog_trips.set(
            self.watchdog_trips
                .get()
                .saturating_add(core.take_watchdog_trips()),
        );
        self.playing.set(core.is_playing());
        self.audio_ready.set(core.is_audio_device_ready());
        self.io_latency_ms
            .set((core.io_latency_seconds() * 1000.0) as f32);
        self.loop_enabled.set(core.is_loop_enabled());
        self.metronome_enabled.set(core.is_metronome_enabled());
        self.tempo.set(core.get_tempo());
        self.sample_rate.set(core.get_sample_rate());
        self.playhead_beats
            .set(core.samples_to_beats(core.get_playhead()));

        let loudness = core.get_master_loudness();
        self.integrated_lufs.set(loudness.integrated);
        self.true_peak_l.set(loudness.true_peak_l);
        self.true_peak_r.set(loudness.true_peak_r);
        let peak = 10.0f32
            .powf(loudness.true_peak_l / 20.0)
            .max(10.0f32.powf(loudness.true_peak_r / 20.0))
            .clamp(0.0, 1.0);
        self.master_peak.set(peak);
        self.master_gain.set(core.master_gain());
        self.plots.publish_meters(&[peak, peak]);
        let bands = core.get_fft_bands();
        if !bands.is_empty() {
            self.plots.publish_spectrum(&bands);
        }

        // The layout walk is comparatively costly; refresh it a few times a
        // second rather than on every 30 Hz tick.
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 10 == 1 {
            self.refresh_arrange_state(&core);
            self.control_room_dim.set(core.control_room_dimmed());
            self.control_room_dim_db.set(core.control_room_dim_db());
            self.talkback_enabled
                .set(core.control_room_talkback_enabled());
            self.poll_waveforms(&core);
            let devices = parse_audio_devices(&core.list_audio_devices_json());
            self.device_count.set(devices.len() as u32);
            self.audio_devices.set(devices);
        }
        // Library imports complete asynchronously; same ticket poll.
        if self.show_sends.get() {
            if let Ok(routes) = serde_json::from_str::<serde_json::Value>(&core.audio_routes_json())
            {
                if let Some(routes) = routes.as_array() {
                    let source_id = self
                        .tracks
                        .get()
                        .get(self.send_source.get() as usize)
                        .map(|t| t.id)
                        .unwrap_or(0);
                    let dest_id = self
                        .tracks
                        .get()
                        .iter()
                        .filter(|t| t.kind == "Bus")
                        .nth(self.send_dest.get() as usize)
                        .map(|t| t.id)
                        .unwrap_or(0);
                    let route = routes.iter().find(|route| {
                        route
                            .get("send")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                            && route.get("source_id").and_then(serde_json::Value::as_u64)
                                == Some(u64::from(source_id))
                            && route
                                .get("destination_id")
                                .and_then(serde_json::Value::as_u64)
                                == Some(u64::from(dest_id))
                    });
                    if let Some(route) = route {
                        self.send_enabled.set(true);
                        if let Some(gain) = route.get("gain").and_then(serde_json::Value::as_f64) {
                            self.send_gain.set(gain.clamp(0.0, 1.0) as f32);
                        }
                        self.send_pre.set(
                            route
                                .get("pre_fader")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                        );
                    } else {
                        self.send_enabled.set(false);
                    }
                }
            }
        }
        if let Some(ticket) = self
            .library_ticket
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            if self.pending_import.is_none() {
                self.pending_import = Some(ticket);
            } else {
                let _ = core.complete_region_import(ticket.0);
            }
        }
        // Dropped-file imports complete asynchronously; poll the ticket
        // each tick like the import poller.
        if let Some((ticket, path, imported_track)) = self.pending_import.take() {
            match core.complete_region_import(ticket) {
                0 => self.pending_import = Some((ticket, path, imported_track)),
                1 => {
                    let _ = core.register_preview_audio(&path);
                    self.status.set(format!(
                        "IMPORTED: {}",
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or(path)
                    ));
                    self.refresh_arrange_state(&core);
                }
                _ => self.status.set("IMPORT FAILED".to_owned()),
            }
        }
        // Library folder scans execute here (scan_preview_audio indexes
        // the filesystem, so it runs on the poll, not the dialog thread).
        if let Some(path) = self
            .pending_library
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            match core.scan_preview_audio(&path.to_string_lossy()) {
                Ok(count) => {
                    self.library_assets
                        .set(parse_preview_catalog(&core.preview_audio_catalog_json()));
                    self.status.set(format!("LIBRARY SCANNED: {} items", count));
                }
                Err(_) => self.status.set("LIBRARY SCAN FAILED".to_owned()),
            }
        }
        // Preset dialog results (path + save flag + target) execute here.
        if let Some((path, save, track_id, plugin_index)) = self
            .pending_preset
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let accepted = if save {
                core.save_plugin_preset(track_id, plugin_index, &path.to_string_lossy())
            } else {
                core.load_plugin_preset(track_id, plugin_index, &path.to_string_lossy())
            };
            self.status.set(if accepted {
                format!(
                    "PRESET {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            } else {
                "PRESET REJECTED".to_owned()
            });
        }
        // File-dialog results arrive from the async native dialog on a
        // background thread; the engine work happens here on the UI thread.
        if let Some(path) = self
            .pending_open
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let path = path.to_string_lossy().into_owned();
            if core.load_project(&path) {
                *self.project_path.borrow_mut() = Some(path.clone());
                self.selected_clip.set(None);
                self.show_genesis.set(false);
                self.status.set(format!(
                    "OPENED {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or(path)
                ));
            } else {
                self.status.set("LOAD FAILED".to_owned());
            }
        }
        if let Some(path) = self
            .pending_save
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let path = path.to_string_lossy().into_owned();
            if core.save_project(&path) {
                *self.project_path.borrow_mut() = Some(path.clone());
                self.status.set(format!(
                    "SAVED {}",
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or(path)
                ));
            } else {
                self.status.set("SAVE FAILED".to_owned());
            }
        }
        // OpenUtau import: source is the UST/USTX path, render is the
        // rendered stem; both are required by the engine's import path.
        if let Some((source, render)) = self
            .pending_openutau
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let diagnostic = core.openutau_import_diagnostic_json(
                &source.to_string_lossy(),
                &render.to_string_lossy(),
            );
            let ok = serde_json::from_str::<serde_json::Value>(&diagnostic)
                .ok()
                .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                == Some(true);
            if !ok {
                self.openutau_caption
                    .set(format!("IMPORT REJECTED: {diagnostic}"));
            } else {
                let track_id = core.add_track(4);
                if track_id == 0 || !core.add_region(track_id, &render.to_string_lossy(), 0.0) {
                    self.openutau_caption
                        .set("OPENUTAU IMPORT FAILED: COULD NOT CREATE VOCAL REGION".to_owned());
                } else {
                    let _ = core.register_openutau_vocal(
                        &source.to_string_lossy(),
                        &render.to_string_lossy(),
                    );
                    self.tracks.set(parse_arrange_tracks(
                        &core.get_project_layout_json(),
                        |samples| core.samples_to_beats(samples),
                    ));
                    self.openutau_caption.set(format!(
                        "VOCAL IMPORTED: {} · SOURCE KEPT: {}",
                        render
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("render"),
                        source
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("score"),
                    ));
                }
            }
        }
        // MusicXML export: engine takes care of layout + instrument names.
        if let Some(path) = self
            .pending_musicxml
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            match core.export_musicxml(&path.to_string_lossy()) {
                Ok(count) => self
                    .status
                    .set(format!("MUSICXML EXPORTED · {count} NOTES")),
                Err(error) => self.status.set(format!("MUSICXML EXPORT FAILED: {error}")),
            }
        }
        // MIDI export dialog result: write the .mid through the engine.
        if let Some(path) = self
            .pending_midi
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            match core.export_midi_file(&path.to_string_lossy()) {
                Ok(count) => self.status.set(format!("MIDI EXPORTED · {count} NOTES")),
                Err(_) => self.status.set("MIDI EXPORT FAILED".to_owned()),
            }
        }
        // Stem export folder selected: arm the batch; the poll advances it
        // once the engine reports bounce state 3 (see poll_render).
        if let Some(dir) = self
            .pending_stems
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or_default()
        {
            let track_ids: Vec<u32> = self.tracks.get().iter().map(|t| t.id).collect();
            if track_ids.is_empty() {
                self.render_caption
                    .set("STEM EXPORT NEEDS AT LEAST ONE TRACK".to_owned());
            } else {
                let prepared = core.stem_batch_prepare(
                    dir.to_string_lossy().as_ref(),
                    self.export_format.get(),
                    &track_ids,
                    render_tail_seconds(self.export_tail_index.get() as i32),
                    false,
                    true,
                );
                match prepare_rejected(&prepared) {
                    Some(reason) => self.render_caption.set(reason),
                    None => match stem_step(&core.stem_batch_next()) {
                        StemStep::Ready { output, format, .. } => {
                            if kick_stem(&core, &output, format) {
                                self.stem_export_active.set(true);
                                self.render_caption.set("STEM EXPORT STARTED".to_owned());
                            } else {
                                core.stem_batch_abort();
                                self.render_caption
                                    .set("could not queue the first stem".to_owned());
                            }
                        }
                        StemStep::Failed(reason) => self.render_caption.set(reason),
                        _ => self
                            .render_caption
                            .set("stem export queue is invalid".to_owned()),
                    },
                }
            }
        }
    }
}
