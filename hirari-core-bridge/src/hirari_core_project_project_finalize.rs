impl HirariCore {
    fn finalize_loaded_project<F>(
        &self,
        document: &ProjectDocument,
        engine: &crate::ffi::AudioEngine,
        native_track_ids: &std::collections::HashMap<u32, u32>,
        rollback_path: &std::path::Path,
        persisted_control_room: Option<crate::control_room::ControlRoomState>,
        rollback_error: F,
    ) -> anyhow::Result<()>
    where
        F: Fn(anyhow::Error) -> anyhow::Error,
    {
        if (engine.get_sample_rate() - document.sample_rate).abs() > 0.5 {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded engine sample rate differs from project"
            )));
        }
        if document.cycle_enabled {
            if !engine.set_cycle_range(document.cycle_start_sample, document.cycle_end_sample, true)
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid cycle range"
                )));
            }
        } else {
            engine.set_loop(false);
        }
        engine.set_metronome_enabled(document.metronome_enabled);
        if !engine.set_master_gain(document.master_gain) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains an invalid master gain"
            )));
        }
        engine.clear_midi_notes();
        let packed_midi_notes = document
            .midi_notes
            .iter()
            .flat_map(|note| {
                [
                    u64::from(note.track_id),
                    u64::from(note.pitch),
                    u64::from(note.velocity),
                    u64::from(note.midi_channel),
                    note.start_sample,
                    note.length_samples,
                    u64::from(note.probability),
                    u64::from(note.region_id),
                    u64::from(note.articulation),
                ]
            })
            .collect();
        if !engine.replace_midi_notes(packed_midi_notes, false) {
            return Err(rollback_error(anyhow::anyhow!(
                "failed to publish project MIDI notes"
            )));
        }
        engine.clear_vca_groups();
        for group in &document.vca_groups {
            if !engine.add_vca_group(group.id, group.gain) {
                return Err(rollback_error(anyhow::anyhow!(
                    "loaded project contains an invalid VCA group"
                )));
            }
            for track_id in &group.track_ids {
                let Some(native_track_id) = native_track_ids.get(track_id).copied() else {
                    return Err(rollback_error(anyhow::anyhow!(
                        "loaded VCA group references an unknown track"
                    )));
                };
                if !engine.assign_track_to_vca(native_track_id, group.id) {
                    return Err(rollback_error(anyhow::anyhow!(
                        "loaded project contains an invalid VCA track assignment"
                    )));
                }
            }
        }
        let mut restored_comping = crate::comping::CompingOrchestrator::new();
        for take in &document.comp_takes {
            let mut regions = Vec::with_capacity(take.regions.len());
            for region in &take.regions {
                let Some(track_id) = native_track_ids.get(&region.track_id).copied() else {
                    return Err(rollback_error(anyhow::anyhow!(
                        "loaded comp take references an unknown track"
                    )));
                };
                regions.push(crate::comping::TakeRegion {
                    track_id,
                    region_id: region.region_id,
                });
            }
            restored_comping.add_take(crate::comping::Take {
                id: take.id,
                name: take.name.clone(),
                start_sample: take.start_sample,
                end_sample: take.end_sample,
                regions,
            });
        }
        restored_comping.set_segments(
            document
                .comp_segments
                .iter()
                .map(|segment| crate::comping::CompSegment {
                    take_id: segment.take_id,
                    start: segment.start_sample,
                    len: segment.length_samples,
                    crossfade_samples: segment.crossfade_samples,
                })
                .collect(),
        );
        if !restored_comping.audit_comping() {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid comping state"
            )));
        }
        let comping_json = serde_json::to_string(&restored_comping)
            .map_err(|error| rollback_error(anyhow::anyhow!(error)))?;
        if !self.restore_comping_snapshot_json(&comping_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "failed to restore project comping state"
            )));
        }
        *self
            .scheduled_midi_notes
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI note lock poisoned"))? = document.midi_notes.clone();
        *self
            .midi_vibrato_rates
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI vibrato-rate lock poisoned"))? = document
            .midi_notes
            .iter()
            .map(|note| {
                (
                    (note.track_id, note.region_id, note.pitch, note.start_sample),
                    note.vibrato_rate_millihz,
                )
            })
            .collect();
        *self
            .chord_track
            .lock()
            .map_err(|_| anyhow::anyhow!("chord track lock poisoned"))? =
            document.chord_track.clone();
        *self
            .midi_events
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI event lock poisoned"))? =
            document.midi_events.clone();
        *self
            .openutau_vocals
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenUtau metadata lock poisoned"))? =
            document.openutau_vocals.clone();
        let track_stacks_json = serde_json::to_string(&document.track_stacks).map_err(|error| {
            anyhow::anyhow!("track stack snapshot serialization failed: {error}")
        })?;
        if !self.restore_track_stacks_json(&track_stacks_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid track stack state"
            )));
        }
        let markers_json = serde_json::to_string(&document.markers).map_err(|error| {
            rollback_error(anyhow::anyhow!(
                "marker snapshot serialization failed: {error}"
            ))
        })?;
        if !self.restore_markers_json(&markers_json) {
            return Err(rollback_error(anyhow::anyhow!(
                "loaded project contains invalid arrangement markers"
            )));
        }
        *self
            .macro_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("Macro mapping lock poisoned"))? =
            document.macro_mappings.clone();
        *self
            .midi_learn_mappings
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI mapping lock poisoned"))? =
            document.midi_learn_mappings.clone();
        // Hardware pickup must be reacquired after hydration; otherwise a
        // controller could jump a restored parameter on its first message.
        self.midi_pickup_acquired
            .lock()
            .map_err(|_| anyhow::anyhow!("MIDI pickup state lock poisoned"))?
            .clear();
        // Hydration uses the same mutation APIs as interactive edits. Do not
        // expose those internal mutations as the first Undo steps of the new
        // document.
        engine.clear_undo_history();
        if let Ok(mut history) = self.midi_lyric_history.lock() {
            history.clear();
        }
        if let Ok(mut history) = self.chord_history.lock() {
            history.clear();
        }
        if let Ok(mut redo) = self.chord_redo_history.lock() {
            redo.clear();
        }
        if let Some(state) = persisted_control_room {
            // Rehydrate both the Rust control-plane snapshot and the native
            // realtime monitor graph. The runtime object written below *is*
            // the graph the audio callback reads, so the UI and the callback
            // now share one state instead of maintaining two copies that could
            // drift after reopening a project. The handle is read off the
            // engine parameter rather than `self` because this method is handed
            // an engine explicitly.
            let handle = engine.control_room_state() as *mut std::ffi::c_void;
            unsafe {
                crate::control_room_runtime::hirari_control_room_reset(handle);
            }
            for (index, name) in state.monitor_outputs.iter().enumerate() {
                if index > 0
                    && !unsafe {
                        crate::control_room_runtime::hirari_control_room_add_speaker(
                            handle,
                            name.as_ptr(),
                            name.len(),
                            state.monitor_output_gains[index],
                        )
                    }
                {
                    return Err(rollback_error(anyhow::anyhow!(
                        "failed to restore control room output"
                    )));
                }
                if !unsafe {
                    crate::control_room_runtime::hirari_control_room_set_speaker_gain(
                        handle,
                        index as u32 as usize,
                        state.monitor_output_gains[index],
                    )
                } || !unsafe {
                    crate::control_room_runtime::hirari_control_room_set_speaker_enabled(
                        handle,
                        index as u32 as usize,
                        state.monitor_output_enabled[index],
                    )
                } {
                    return Err(rollback_error(anyhow::anyhow!(
                        "failed to restore control room output settings"
                    )));
                }
            }
            if !unsafe {
                crate::control_room_runtime::hirari_control_room_select_speaker(
                    handle,
                    state.active_output as u32 as usize,
                )
            } {
                return Err(rollback_error(anyhow::anyhow!(
                    "failed to restore active control room output"
                )));
            }
            for cue in &state.cues {
                if !unsafe {
                    crate::control_room_runtime::hirari_control_room_upsert_cue(
                        handle,
                        cue.id,
                        cue.gain,
                        cue.enabled,
                    )
                } {
                    return Err(rollback_error(anyhow::anyhow!(
                        "failed to restore control room cue"
                    )));
                }
                if let Some(bus_track_id) = cue.bus_track_id {
                    // The native setter only validated non-zero bus targets,
                    // and it did so against the track list C++ still owns.
                    let resolves = bus_track_id == 0 || engine.bus_track_exists(bus_track_id);
                    if !resolves
                        || !unsafe {
                            crate::control_room_runtime::hirari_control_room_set_cue_bus(
                                handle,
                                cue.id,
                                bus_track_id,
                            )
                        }
                    {
                        return Err(rollback_error(anyhow::anyhow!(
                            "failed to restore Control Room Cue bus"
                        )));
                    }
                }
                if let Some(output_channel) = cue.output_channel {
                    if !unsafe {
                        crate::control_room_runtime::hirari_control_room_set_cue_output(
                            handle,
                            cue.id,
                            output_channel,
                        )
                    } {
                        return Err(rollback_error(anyhow::anyhow!(
                            "failed to restore Control Room Cue output pair"
                        )));
                    }
                }
                if !unsafe {
                    crate::control_room_runtime::hirari_control_room_set_cue_click(
                        handle,
                        cue.id,
                        cue.click_enabled,
                    )
                } {
                    return Err(rollback_error(anyhow::anyhow!(
                        "failed to restore Control Room Cue click routing"
                    )));
                }
            }
            let active_cue = state.active_cue.unwrap_or(0);
            // Selecting a cue is only legal when its bus target resolves to a
            // real BusTrack, which is the lookup HirariUnifiedEngine used to do
            // inline before committing.
            if active_cue != 0 {
                let bus_track_id = unsafe {
                    crate::control_room_runtime::hirari_control_room_cue_bus(handle, active_cue)
                };
                if bus_track_id == 0 || !engine.bus_track_exists(bus_track_id) {
                    return Err(rollback_error(anyhow::anyhow!(
                        "failed to restore active Control Room Cue"
                    )));
                }
            }
            if !unsafe {
                crate::control_room_runtime::hirari_control_room_select_cue(handle, active_cue)
            } {
                return Err(rollback_error(anyhow::anyhow!(
                    "failed to restore active Control Room Cue"
                )));
            }
            if !unsafe {
                crate::control_room_runtime::hirari_control_room_set_dim_db(handle, state.dim_db)
            } {
                return Err(rollback_error(anyhow::anyhow!(
                    "failed to restore Control Room DIM level"
                )));
            }
            unsafe {
                crate::control_room_runtime::hirari_control_room_set_dim(handle, state.dim);
                crate::control_room_runtime::hirari_control_room_set_talkback(
                    handle,
                    state.talkback,
                    state.talkback_gain,
                );
            }
            if !unsafe {
                crate::control_room_runtime::hirari_control_room_set_talkback_channel(
                    handle,
                    state.talkback_input_channel,
                )
            } {
                return Err(rollback_error(anyhow::anyhow!(
                    "failed to restore Control Room Talkback input"
                )));
            }
            *self
                .control_room
                .lock()
                .map_err(|_| anyhow::anyhow!("control room lock poisoned"))? = state;
        }
        let _ = std::fs::remove_file(&rollback_path);
        Ok(())
    }
}
