//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Sample the engine on the UI thread and publish reactive state.
    /// Drives the offline-render flow while a render or a stems batch is
    /// in flight, mirroring the `telemetry_render` cadence: the
    /// engine reports bounce progress/completion and the stems queue is
    /// advanced here, never on the UI event path.
    pub(in crate::ameui) fn poll_render(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let Some((state, progress)) = core.get_bounce_status() else {
            return;
        };
        match state {
            3 => {
                if !self.stem_export_active.get() {
                    self.render_caption.set("RENDER COMPLETE".to_owned());
                    return;
                }
                match stem_step(&core.stem_batch_settle(true)) {
                    StemStep::StemDone { rendered, total } => {
                        match stem_step(&core.stem_batch_next()) {
                            StemStep::Ready { output, format, .. } => {
                                if kick_stem(&core, &output, format) {
                                    self.render_caption.set(format!(
                                        "STEM {rendered} OF {total} COMPLETE · NEXT QUEUED"
                                    ));
                                } else {
                                    core.stem_batch_abort();
                                    self.stem_export_active.set(false);
                                    self.render_caption.set(format!(
                                        "could not queue stem {} of {total}",
                                        rendered + 1
                                    ));
                                }
                            }
                            StemStep::Failed(reason) => {
                                core.stem_batch_abort();
                                self.stem_export_active.set(false);
                                self.render_caption.set(reason);
                            }
                            _ => {
                                core.stem_batch_abort();
                                self.stem_export_active.set(false);
                                self.render_caption
                                    .set("stem export queue is invalid".to_owned());
                            }
                        }
                    }
                    StemStep::Complete { outputs, .. } => {
                        self.stem_export_active.set(false);
                        self.render_caption
                            .set(format!("STEM EXPORT COMPLETE · {} FILES", outputs.len()));
                    }
                    StemStep::Failed(reason) => {
                        self.stem_export_active.set(false);
                        self.render_caption.set(reason);
                    }
                    StemStep::Ready { .. } => {
                        core.stem_batch_abort();
                        self.stem_export_active.set(false);
                        self.render_caption
                            .set("stem export reported an unexpected state".to_owned());
                    }
                }
            }
            4 => {
                if self.stem_export_active.get() {
                    let reason = match stem_step(&core.stem_batch_settle(false)) {
                        StemStep::Failed(reason) => reason,
                        _ => "native stem render failed".to_owned(),
                    };
                    self.render_caption.set(reason);
                } else {
                    self.render_caption.set("RENDER FAILED".to_owned());
                }
                self.stem_export_active.set(false);
            }
            5 => {
                if self.stem_export_active.get() {
                    core.stem_batch_abort();
                }
                self.stem_export_active.set(false);
                self.render_caption.set("RENDER CANCELLED".to_owned());
            }
            1 | 2 => {
                self.render_caption.set(format!(
                    "RENDERING {:.0}%",
                    progress.clamp(0.0, 1.0) * 100.0
                ));
            }
            _ => {}
        }
    }

    /// Video frame poll (control rate, follows playhead): requests the
    /// frame for the playhead position and stashes it in `self.video_frame`
    /// when a new revision lands. The actual texture upload happens on the
    /// draw path, like the waveform.
    pub(in crate::ameui) fn poll_video(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        if !self.show_video.get() {
            return;
        }
        let sample_rate = core.get_sample_rate().max(1.0);
        let seconds = core.get_playhead() as f64 / sample_rate;
        let _ = core.request_video_frame(seconds);
        let revision = core.get_video_frame_revision();
        let last = self.video_revision.lock().map(|last| *last).unwrap_or(0);
        if revision == 0 || revision == last {
            return;
        }
        let frame = core.get_video_frame();
        if frame.len() < 8 {
            return;
        }
        let width = u32::from_le_bytes(frame[0..4].try_into().unwrap_or([0; 4]));
        let height = u32::from_le_bytes(frame[4..8].try_into().unwrap_or([0; 4]));
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if width == 0 || height == 0 || width > 8192 || height > 8192 || frame.len() != expected + 8
        {
            return;
        }
        self.video_frame = Some(frame[8..].to_vec());
        self.video_size = Some((width, height));
        if let Ok(mut slot) = self.video_revision.lock() {
            *slot = revision;
        }
    }

    /// Moufu connector poll. Same event discipline as the dock's live
    /// link timer: publish the snapshot when it changes while an outbound
    /// link is registered, and mirror the queue effects into the card.
    /// Runs every 8th tick (~250ms) so the drain does not contend with the
    /// 30 Hz engine poll.
    pub(in crate::ameui) fn poll_moufu(&mut self) {
        let Some(publisher) = self.moufu_publisher.clone() else {
            return;
        };
        if self.tick % 8 != 0 {
            return;
        }
        let Some(core) = self.core.clone() else {
            return;
        };
        if self.moufu_has_outbound.load(Ordering::Acquire) {
            let revision = core.project_layout_revision();
            let should_publish = self
                .moufu_last_revision
                .lock()
                .map(|last| *last != Some(revision))
                .unwrap_or(true);
            if should_publish {
                crate::ui_shared::moufu_layout::publish_project_snapshot(
                    &publisher,
                    &core,
                    Vec::new(),
                    true,
                );
                if let Ok(mut last) = self.moufu_last_revision.lock() {
                    *last = Some(revision);
                }
            }
        }
        for event in publisher.drain_events() {
            match event {
                MoufuEvent::ConnectionConfigured { enabled } => {
                    self.moufu_outbound_link_ids
                        .lock()
                        .map(|mut set| set.clear())
                        .unwrap_or(());
                    self.moufu_has_outbound.store(false, Ordering::Release);
                    self.moufu_last_revision
                        .lock()
                        .map(|mut rev| *rev = None)
                        .unwrap_or(());
                    self.moufu_remote_versions
                        .lock()
                        .map(|mut remote| remote.clear())
                        .unwrap_or(());
                    self.moufu_entity_labels.set(Vec::new());
                    self.moufu_entity_ids.set(Vec::new());
                    self.moufu_link_labels.set(Vec::new());
                    self.moufu_link_ids.set(Vec::new());
                    self.moufu_entity_selected.set(0);
                    self.moufu_link_selected.set(0);
                    self.moufu_remote_source.set(String::new());
                    self.moufu_preview
                        .set("No linked project state received.".to_owned());
                    self.moufu_status.set(if enabled {
                        "MOUFU CONNECTING".to_owned()
                    } else {
                        "MOUFU NOT CONFIGURED".to_owned()
                    });
                }
                MoufuEvent::Connected { server_version } => {
                    log::info!("Connected to Moufu server {server_version}");
                    self.moufu_status.set("MOUFU CONNECTED".to_owned());
                    let _ = publisher.list_entities();
                    let _ = publisher.list_links();
                }
                MoufuEvent::Disconnected { reason } => {
                    log::debug!("Moufu reconnect scheduled: {reason}");
                    self.moufu_outbound_link_ids
                        .lock()
                        .map(|mut set| set.clear())
                        .unwrap_or(());
                    self.moufu_has_outbound.store(false, Ordering::Release);
                    self.moufu_last_revision
                        .lock()
                        .map(|mut rev| *rev = None)
                        .unwrap_or(());
                    self.moufu_status.set("MOUFU RECONNECTING".to_owned());
                    self.status
                        .set(format!("MOUFU INTERRUPTED; RETRYING: {reason}"));
                }
                MoufuEvent::EntityList(entities) => {
                    let project_entities: Vec<_> = entities
                        .iter()
                        .filter(|entity| {
                            entity.data_type == HIRARI_PROJECT_DATA_TYPE
                                && entity.id.0 != HIRARI_PROJECT_ENTITY
                        })
                        .collect();
                    let labels: Vec<String> = project_entities
                        .iter()
                        .map(|entity| {
                            format!("{} · {} · {}", entity.source_app, entity.name, entity.id.0)
                        })
                        .collect();
                    let ids: Vec<String> = project_entities
                        .iter()
                        .map(|entity| entity.id.0.clone())
                        .collect();
                    let count = labels.len();
                    if !labels.is_empty() {
                        self.moufu_entity_selected.set(0);
                    }
                    self.moufu_entity_labels.set(labels);
                    self.moufu_entity_ids.set(ids);
                    self.moufu_status
                        .set(format!("MOUFU CONNECTED · {count} PROJECT ENTITIES"));
                }
                MoufuEvent::LinkList(links) => {
                    let outbound: HashSet<String> = links
                        .iter()
                        .filter(|link| {
                            link.source_entity.0 == HIRARI_PROJECT_ENTITY
                                && link.target_entity.0 != HIRARI_PROJECT_ENTITY
                                && link.status == LinkStatus::Active
                        })
                        .map(|link| link.link_id.to_string())
                        .collect();
                    let had_new = outbound.iter().any(|id| {
                        self.moufu_outbound_link_ids
                            .lock()
                            .map(|set| !set.contains(id))
                            .unwrap_or(true)
                    });
                    if had_new {
                        // A freshly linked peer needs the current snapshot
                        // even when the project has not moved.
                        let _ = publisher.republish_latest_layout();
                        if let Ok(mut rev) = self.moufu_last_revision.lock() {
                            *rev = Some(core.project_layout_revision());
                        }
                    }
                    if outbound.is_empty() {
                        self.moufu_last_revision
                            .lock()
                            .map(|mut rev| *rev = None)
                            .unwrap_or(());
                    }
                    if let Ok(mut set) = self.moufu_outbound_link_ids.lock() {
                        *set = outbound.clone();
                    }
                    self.moufu_has_outbound
                        .store(!outbound.is_empty(), Ordering::Release);
                    let relevant: Vec<_> = links
                        .iter()
                        .filter(|link| {
                            link.source_entity.0 == HIRARI_PROJECT_ENTITY
                                || link.target_entity.0 == HIRARI_PROJECT_ENTITY
                        })
                        .collect();
                    let labels: Vec<String> = relevant
                        .iter()
                        .map(|link| {
                            format!(
                                "{} → {} · {:?}",
                                link.source_entity.0, link.target_entity.0, link.status
                            )
                        })
                        .collect();
                    let ids: Vec<String> = relevant
                        .iter()
                        .map(|link| link.link_id.to_string())
                        .collect();
                    let count = labels.len();
                    if !labels.is_empty() {
                        self.moufu_link_selected.set(0);
                    }
                    // Forget snapshots for sources whose links went away.
                    let inbound_sources: HashSet<&str> = links
                        .iter()
                        .filter(|link| {
                            link.target_entity.0 == HIRARI_PROJECT_ENTITY
                                && link.status == LinkStatus::Active
                        })
                        .map(|link| link.source_entity.0.as_str())
                        .collect();
                    if let Ok(mut versions) = self.moufu_remote_versions.lock() {
                        versions.retain(|source, _| inbound_sources.contains(source.as_str()));
                    }
                    self.moufu_entity_ids.get();
                    let remote_source = self.moufu_remote_source.get();
                    if !remote_source.is_empty()
                        && !inbound_sources.contains(remote_source.as_str())
                    {
                        self.moufu_remote_source.set(String::new());
                        self.moufu_preview
                            .set("No active inbound project link.".to_owned());
                    }
                    self.moufu_link_labels.set(labels);
                    self.moufu_link_ids.set(ids);
                    self.moufu_status
                        .set(format!("MOUFU CONNECTED · {count} PROJECT LINKS"));
                }
                MoufuEvent::EntityUpdated(change) => {
                    let accepted = self
                        .moufu_remote_versions
                        .lock()
                        .map(|mut versions| {
                            accept_moufu_version(
                                &mut versions,
                                &change.entity_id.0,
                                change.version,
                                change.is_transient,
                            )
                        })
                        .unwrap_or(false);
                    if accepted {
                        let (source, preview) = linked_project_preview(
                            &change.entity_id.0,
                            change.version,
                            change.is_transient,
                            &change.payload,
                        );
                        self.moufu_remote_source.set(source);
                        self.moufu_preview.set(preview);
                    }
                }
                MoufuEvent::EntityState {
                    entity_id,
                    version,
                    payload,
                } => {
                    let accepted = self
                        .moufu_remote_versions
                        .lock()
                        .map(|mut versions| {
                            accept_moufu_version(&mut versions, &entity_id.0, version, false)
                        })
                        .unwrap_or(false);
                    if accepted {
                        let (source, preview) =
                            linked_project_preview(&entity_id.0, version, false, &payload);
                        self.moufu_remote_source.set(source);
                        self.moufu_preview.set(preview);
                    }
                }
                _ => {}
            }
        }
    }
}
