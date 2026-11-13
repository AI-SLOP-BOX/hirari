//! Track index restoration phase, split out of hirari_core_project_loading.rs.
use super::*;

impl HirariCore {
    pub(super) fn restore_project_track_index(
        &self,
        document: &ProjectDocument,
        engine: &crate::AudioEngine,
        sample_rate: u32,
        rollback_error: &dyn Fn(anyhow::Error) -> anyhow::Error,
    ) -> anyhow::Result<std::collections::HashMap<u32, u32>> {
        let _ = sample_rate; // kept for signature stability
        let mut native_track_ids = std::collections::HashMap::with_capacity(document.tracks.len());
        let mut used_native_track_ids =
            std::collections::HashSet::with_capacity(document.tracks.len());
        for track in &document.tracks {
            let native_id = match ProjectDocument::native_track_type_code(&track.track_type) {
                Ok(track_type) => engine.add_track(track_type),
                Err(error) => {
                    return Err(rollback_error(error));
                }
            };
            if native_id == 0 {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to create track {}",
                    track.id
                )));
            }
            if !used_native_track_ids.insert(native_id) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine returned duplicate track id {native_id}"
                )));
            }
            if !engine.set_track_name(native_id, &track.name) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to name track {}",
                    track.id
                )));
            }
            let mut effective_expression_map = track.expression_map.clone();
            for entry in &mut effective_expression_map {
                if entry.name.trim().is_empty() {
                    entry.name = format!("Articulation {}", entry.articulation_id);
                }
                if entry.outputs.is_empty() {
                    if let Some(note) = entry.keyswitch_pitch {
                            entry.outputs.push(crate::project_contracts::ExpressionMapOutput::KeySwitch {
                                note, velocity: 127, length_ticks: 0,
                        });
                    }
                }
            }
            let mut expression_map = Vec::new();
            for entry in &effective_expression_map {
                for (output, off_phase) in entry.outputs.iter().map(|output| (output, false))
                    .chain(entry.off_outputs.iter().map(|output| (output, true))) {
                    use crate::project_contracts::ExpressionMapOutput as Output;
                    let (base_kind, a, b, c) = match output {
                        Output::KeySwitch { note, velocity, length_ticks } => (
                            1, *note as u32, *velocity as u32, *length_ticks,
                        ),
                        Output::ProgramChange { bank_msb, bank_lsb, program } => (
                            2, *program as u32, bank_msb.unwrap_or(255) as u32, bank_lsb.unwrap_or(255) as u32,
                        ),
                        Output::ControlChange { controller, value } => (3, *controller as u32, *value as u32, 0),
                        Output::ChannelPressure { value } => (4, *value as u32, 0, 0),
                        Output::PitchBend { value } => {
                            let bend = (*value as i32 + 8192) as u32;
                            (5, bend & 0x7f, (bend >> 7) & 0x7f, 0)
                        }
                    };
                    let kind = base_kind + if off_phase { 5 } else { 0 };
                    expression_map.extend([entry.articulation_id as u32, entry.channel as u32, kind, a, b, c]);
                }
            }
            let expression_map_json = serde_json::to_string(&effective_expression_map)?;
            if !engine.set_expression_map(native_id, expression_map, expression_map_json, 0) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to apply the expression map for track {}",
                    track.id
                )));
            }
            let expression_map_pro_json = track.expression_map_pro.as_ref()
                .map(serde_json::to_string)
                .transpose()?
                .unwrap_or_else(|| "null".to_owned());
            if !engine.set_expression_map_pro(native_id, expression_map_pro_json) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to apply the rich expression map for track {}",
                    track.id
                )));
            }
            // Project loading is a control-thread state transition, not an
            // audio callback command. Applying these values immediately
            // prevents stale queued commands from overwriting later edits
            // when the first render/audio block drains the queue.
            if !engine.set_track_volume(native_id, track.volume)
                || !engine.set_track_pan(native_id, track.pan)
                || !engine.set_track_mute(native_id, track.muted)
                || !engine.set_track_solo(native_id, track.solo)
                || !engine.set_track_armed(native_id, track.record_armed)
                || !engine.set_phase_invert(native_id, track.phase_invert)
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore state for track {}",
                    track.id
                )));
            }
            let mut sandbox_index = 0u32;
            for (plugin_index, plugin_type) in track.plugin_types.iter().copied().enumerate() {
                if plugin_type == u32::MAX {
                    let Some(path) = track.sandbox_plugin_paths.get(sandbox_index as usize) else {
                        return Err(rollback_error(anyhow::anyhow!(
                            "sandbox plugin metadata is missing for track {}",
                            track.id
                        )));
                    };
                    if !engine.add_sandboxed_plugin(native_id, path) {
                        let (_, admission) = crate::plugin_sandbox_maintenance::last_sandbox_failure(
                            crate::audio_engine_delegate_ext::host(engine),
                            native_id,
                        );
                        let admission = if admission.is_empty() {
                            "track-not-found".to_owned()
                        } else {
                            admission
                        };
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore sandbox plugin {plugin_index} on track {}: {}",
                            track.id, admission
                        )));
                    }
                    if track
                        .plugin_bypasses
                        .get(plugin_index)
                        .copied()
                        .unwrap_or(false)
                        && !engine.set_plugin_bypass(native_id, plugin_index as u32, true)
                    {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore bypass state for sandbox plugin {plugin_index} on track {}",
                            track.id
                        )));
                    }
                    if let Some(state) = track.sandbox_plugin_states.get(sandbox_index as usize) {
                        if !state.is_empty()
                            && !crate::plugin_sandbox_maintenance::set_sandbox_plugin_state(
                                crate::audio_engine_delegate_ext::host(engine),
                                native_id,
                                sandbox_index,
                                state,
                            ) {
                            let (_, state_error) =
                                crate::plugin_sandbox_maintenance::sandbox_state_error(
                                    crate::audio_engine_delegate_ext::host(engine),
                                    native_id,
                                    sandbox_index,
                                );
                            let state_error = if state_error.is_empty() {
                                "state-unavailable".to_owned()
                            } else {
                                state_error
                            };
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore sandbox plugin state {plugin_index} on track {}: {}",
                                track.id, state_error
                            )));
                        }
                    }
                    sandbox_index += 1;
                    for (parameter_id, value) in track
                        .plugin_parameter_values
                        .get(plugin_index)
                        .into_iter()
                        .flatten()
                        .copied()
                        .enumerate()
                    {
                        if !engine.set_plugin_parameter_without_undo(
                            native_id,
                            plugin_index as u32,
                            parameter_id as u32,
                            value,
                        ) {
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore parameter {parameter_id} for sandbox plugin {plugin_index} on track {}",
                                track.id
                            )));
                        }
                    }
                } else {
                    if plugin_type > 10 || !engine.add_plugin(native_id, plugin_type) {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore unsupported plugin {plugin_index} on track {}",
                            track.id
                        )));
                    }
                    if let Some(state) = track.plugin_states.get(plugin_index) {
                        if !state.is_empty()
                            && !engine.set_plugin_state(native_id, plugin_index as u32, state)
                        {
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore plugin state {plugin_index} on track {}",
                                track.id
                            )));
                        }
                    }
                    if track
                        .plugin_bypasses
                        .get(plugin_index)
                        .copied()
                        .unwrap_or(false)
                        && !engine.set_plugin_bypass(native_id, plugin_index as u32, true)
                    {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore bypass state for plugin {plugin_index} on track {}",
                            track.id
                        )));
                    }
                    for (parameter_id, value) in track
                        .plugin_parameter_values
                        .get(plugin_index)
                        .into_iter()
                        .flatten()
                        .copied()
                        .enumerate()
                    {
                        if !engine.set_plugin_parameter_without_undo(
                            native_id,
                            plugin_index as u32,
                            parameter_id as u32,
                            value,
                        ) {
                            return Err(rollback_error(anyhow::anyhow!(
                                "native engine failed to restore parameter {parameter_id} for plugin {plugin_index} on track {}",
                                track.id
                            )));
                        }
                    }
                }
            }
            for (parameter_id, points) in [
                (0u32, &track.volume_automation),
                (1u32, &track.pan_automation),
            ] {
                if !points.is_empty() {
                    let packed = points
                        .iter()
                        .flat_map(|point| {
                            [point.time, f64::from(point.value), f64::from(point.curve)]
                        })
                        .collect();
                    if !engine.set_automation_data(native_id, parameter_id, packed) {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore automation {} for track {}",
                            parameter_id,
                            track.id
                        )));
                    }
                }
            }
            if !track.track_delay_automation.is_empty() {
                let packed = track
                    .track_delay_automation
                    .iter()
                    .flat_map(|point| [point.time, f64::from(point.value), f64::from(point.curve)])
                    .collect();
                if !engine.set_track_delay_automation(native_id, packed) {
                    return Err(rollback_error(anyhow::anyhow!(
                        "native engine failed to restore track delay automation for track {}",
                        track.id
                    )));
                }
            }
            for lane in &track.plugin_automation {
                let packed = lane
                    .points
                    .iter()
                    .flat_map(|point| [point.sample as f64, point.normalized, point.curve])
                    .collect();
                if !self.set_plugin_automation(
                    native_id,
                    lane.plugin_index,
                    lane.parameter_id,
                    packed,
                ) {
                    return Err(rollback_error(anyhow::anyhow!(
                        "native engine failed to restore plugin parameter automation for track {} slot {} parameter {}",
                        track.id,
                        lane.plugin_index,
                        lane.parameter_id
                    )));
                }
            }
            native_track_ids.insert(track.id, native_id);
        }
        Ok(native_track_ids)
    }
}
