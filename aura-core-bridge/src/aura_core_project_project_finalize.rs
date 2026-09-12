impl AuraCore {
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
        for note in &document.midi_notes {
            engine.set_midi_note(
                note.track_id,
                note.pitch,
                note.velocity,
                note.start_sample,
                note.length_samples,
            );
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
            restored_comping.add_take(crate::comping::Take {
                id: take.id,
                name: take.name.clone(),
                start_sample: take.start_sample,
                end_sample: take.end_sample,
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
            // realtime monitor graph. Keeping only the Rust copy would make
            // the UI look correct while the audio callback still used the
            // default speaker set after reopening a project.
            engine.reset_control_room();
            for (index, name) in state.monitor_outputs.iter().enumerate() {
                if index > 0 && !engine.add_control_room_speaker(name, state.monitor_output_gains[index]) {
                    return Err(rollback_error(anyhow::anyhow!("failed to restore control room output")));
                }
                if !engine.set_control_room_speaker_gain(index as u32, state.monitor_output_gains[index])
                    || !engine.set_control_room_speaker_enabled(index as u32, state.monitor_output_enabled[index])
                {
                    return Err(rollback_error(anyhow::anyhow!("failed to restore control room output settings")));
                }
            }
            if !engine.select_control_room_speaker(state.active_output as u32) {
                return Err(rollback_error(anyhow::anyhow!("failed to restore active control room output")));
            }
            for cue in &state.cues {
                if !engine.upsert_control_room_cue(cue.id, cue.gain, cue.enabled) {
                    return Err(rollback_error(anyhow::anyhow!("failed to restore control room cue")));
                }
            }
            engine.set_control_room_dim(state.dim);
            engine.set_control_room_talkback(state.talkback, state.talkback_gain);
            *self
                .control_room
                .lock()
                .map_err(|_| anyhow::anyhow!("control room lock poisoned"))? = state;
        }
        let _ = std::fs::remove_file(&rollback_path);
        Ok(())
    }
}
