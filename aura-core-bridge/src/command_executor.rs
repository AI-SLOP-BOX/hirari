//! Native command execution kept behind the validated command boundary.
//!
//! This module is intentionally small and explicit.  Adding a new command
//! requires adding it to validation, the capability catalogue, and this
//! executor; an unknown action can never silently become a no-op.

use crate::bridge_error::BridgeError;
use crate::command_api::{CommandAction, MutationClass, ValidatedCommand};
use crate::AuraCore;
use serde_json::{json, Value};

#[derive(Debug, serde::Serialize)]
pub struct ExecutionReport {
    pub transaction: String,
    pub applied: usize,
    pub results: Vec<Value>,
    pub rolled_back: bool,
}

fn native_result(raw: String) -> Result<Value, BridgeError> {
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| BridgeError::new("invalid_native_result", error.to_string()))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    if let Some(error) = value.get("error") {
        if let Ok(error) = serde_json::from_value::<BridgeError>(error.clone()) {
            return Err(error);
        }
    }
    Err(BridgeError::new(
        value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("native_command_rejected"),
        value
            .get("message")
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("native command was rejected"),
    ))
}

fn search_snapshot(value: &Value, needle: &str, path: &str, hits: &mut Vec<Value>) {
    if hits.len() >= 512 {
        return;
    }
    match value {
        Value::String(text) if text.to_ascii_lowercase().contains(needle) => {
            hits.push(json!({"path": path, "value": text}));
        }
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}/{key}");
                if key.to_ascii_lowercase().contains(needle) && hits.len() < 512 {
                    hits.push(json!({"path": child_path, "field": key, "value": child}));
                }
                search_snapshot(child, needle, &child_path, hits);
                if hits.len() >= 512 {
                    break;
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                search_snapshot(child, needle, &format!("{path}/{index}"), hits);
                if hits.len() >= 512 {
                    break;
                }
            }
        }
        _ => {}
    }
}

/// Apply a validated batch to an already-open native core.
///
/// The caller owns the project-wide transaction lock, request ledger, and
/// live-generation check.  This function only performs the native part and
/// compensates reversible mutations with the engine's undo stack if a later
/// action fails.  External side effects are intentionally accepted only as a
/// single-action transaction because they cannot be rolled back reliably.
pub fn execute(
    core: &AuraCore,
    command: &ValidatedCommand,
) -> Result<ExecutionReport, BridgeError> {
    if command.mutation_class == MutationClass::ExternalSideEffect && command.actions.len() != 1 {
        return Err(BridgeError::new(
            "external_action_must_be_isolated",
            "save and render side effects must be submitted as a single-action transaction",
        ));
    }

    let undo_before = core.undo_depth();
    let stack_snapshot_before = core.track_stacks_json();
    let marker_snapshot_before = core.markers_json();
    let extension_activation_before = command
        .actions
        .iter()
        .filter_map(|action| match action {
            CommandAction::ExtensionSetEnabled {
                root, extension_id, ..
            } => crate::extensions::enabled_state(root, extension_id)
                .map(|enabled| (root.clone(), extension_id.clone(), enabled)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut results = Vec::with_capacity(command.actions.len());
    for action in &command.actions {
        let result = match action {
            CommandAction::InspectMidiNotes => {
                let notes = core.midi_notes_json();
                serde_json::from_str::<serde_json::Value>(&notes)
                    .map(|notes| json!({"ok": true, "operation": "inspect_midi_notes", "notes": notes}))
                    .map_err(|_| BridgeError::new("midi_notes_unavailable", "canonical MIDI note metadata is not valid JSON"))
            }
            CommandAction::InspectChordTrack => {
                let chords = core.chord_track_json();
                serde_json::from_str::<serde_json::Value>(&chords)
                    .map(|chords| json!({"ok": true, "operation": "inspect_chord_track", "chords": chords}))
                    .map_err(|_| BridgeError::new("chord_track_unavailable", "canonical chord track is not valid JSON"))
            }
            CommandAction::AddChordEvent { tick, root, intervals, name } => {
                if core.add_chord_event(*tick, *root, intervals.clone(), name) {
                    Ok(json!({"ok": true, "operation": "add_chord_event", "tick": tick, "root": root, "name": name}))
                } else {
                    Err(BridgeError::new("chord_event_rejected", "chord event fields were rejected"))
                }
            }
            CommandAction::PlaceGeneratedChord { track_id, start_sample, length_samples, velocity, root, octave, quality } => {
                let placed = core.place_generated_chord(*track_id, *start_sample, *length_samples, *velocity, *root, *octave, *quality);
                if placed > 0 {
                    Ok(json!({"ok": true, "operation": "place_generated_chord", "track_id": track_id, "start_sample": start_sample, "notes_placed": placed}))
                } else {
                    Err(BridgeError::new("chord_placement_rejected", "generated chord produced no valid MIDI notes"))
                }
            }
            CommandAction::RemoveChordEventsRange { start_tick, end_tick } => {
                let removed = core.remove_chord_events_range(*start_tick, *end_tick);
                Ok(json!({"ok": true, "operation": "remove_chord_events_range", "removed": removed}))
            }
            CommandAction::ClearChordTrack => {
                let removed = core.clear_chord_track();
                Ok(json!({"ok": true, "operation": "clear_chord_track", "removed": removed}))
            }
            CommandAction::DescribeDrumLane { pitch } => Ok(json!({
                "ok": true,
                "operation": "describe_drum_lane",
                "pitch": pitch,
                "label": core.drum_lane_label(*pitch),
            })),
            CommandAction::GenerateChord { root, octave, quality } => {
                let notes = core.generate_chord_notes(*root, *octave, *quality);
                if notes.is_empty() {
                    Err(BridgeError::new("chord_generation_rejected", "chord quality or voicing was rejected"))
                } else {
                    Ok(json!({"ok":true,"operation":"generate_chord","root":root,"octave":octave,"quality":quality,"notes":notes}))
                }
            }
            CommandAction::SuggestNextChords { last_chord_name } => Ok(json!({
                "ok": true,
                "operation": "suggest_next_chords",
                "after": last_chord_name,
                "suggestions": core.suggest_next_chords(last_chord_name),
            })),
            CommandAction::GenerateArpeggio { pitches, velocities, pattern, octaves, steps } => Ok(json!({
                "ok": true,
                "operation": "generate_arpeggio",
                "notes": core.generate_arpeggio(pitches.clone(), velocities.clone(), *pattern, *octaves, *steps)
                    .into_iter().map(|(pitch, velocity)| json!({"pitch": pitch, "velocity": velocity})).collect::<Vec<_>>(),
            })),
            CommandAction::PlaceArpeggio { track_id, start_sample, step_samples, gate_samples, pitches, velocities, pattern, octaves, steps } => {
                let placed = core.place_arpeggio(*track_id, *start_sample, *step_samples, *gate_samples, pitches.clone(), velocities.clone(), *pattern, *octaves, *steps);
                if placed > 0 { Ok(json!({"ok": true, "operation": "place_arpeggio", "track_id": track_id, "notes_placed": placed})) }
                else { Err(BridgeError::new("arpeggio_placement_rejected", "arpeggio produced no notes")) }
            }
            CommandAction::AnalyzeDynamics { samples, track_id } => {
                let params = crate::dynamics::DynamicsOrchestrator.suggest_parameters(samples);
                let gate = crate::dynamics::DynamicsOrchestrator.suggest_gate_threshold(samples);
                let peak = samples.iter().filter(|sample| sample.is_finite()).map(|sample| sample.abs()).fold(0.0_f32, f32::max);
                let peak_db = 20.0 * (peak + 1.0e-9).log10();
                let recommended_gain_db = (-6.0 - peak_db).clamp(-24.0, 24.0);
                let rms = (samples.iter().filter(|sample| sample.is_finite()).map(|sample| (*sample as f64) * (*sample as f64)).sum::<f64>() / samples.len().max(1) as f64).sqrt();
                let rms_db = (20.0 * (rms + 1.0e-9).log10()) as f32;
                let crest_db = (peak_db - rms_db).max(0.0);
                let mut issues = Vec::new();
                if samples.iter().any(|sample| !sample.is_finite()) { issues.push("non_finite_samples"); }
                if peak_db >= -0.1 { issues.push("clipping_risk"); }
                if peak > 1.0e-9 && peak_db < -24.0 { issues.push("very_low_level"); }
                if crest_db > 20.0 { issues.push("high_dynamic_range"); }
                Ok(json!({
                    "ok": true,
                    "operation": "analyze_dynamics",
                    "track_id": track_id,
                    "threshold_db": params.threshold_db,
                    "ratio": params.ratio,
                    "attack_ms": params.attack_ms,
                    "release_ms": params.release_ms,
                    "gate_threshold_db": gate,
                    "peak_db": peak_db,
                    "recommended_gain_db": recommended_gain_db,
                    "gain_staging_target_db": -6.0,
                    "issues": issues,
                    "rms_db": rms_db,
                    "crest_db": crest_db,
                    "sample_count": samples.len(),
                }))
            }
            CommandAction::AnalyzeMix { left, right, reference_left, reference_right, ab_left, ab_right } => {
                let metrics = crate::mix_analyzer::analyze(left, right);
                let mut assistant_actions = Vec::new();
                if metrics.true_peak_db >= -1.0 { assistant_actions.push(json!({"priority":"high","code":"reduce_master_peak","message":"True Peak is within 1 dB of clipping"})); }
                if !metrics.mono_compatible { assistant_actions.push(json!({"priority":"high","code":"check_phase","message":"Mono fold-down loses significant level"})); }
                if metrics.correlation < -0.25 { assistant_actions.push(json!({"priority":"high","code":"invert_or_align_phase","message":"Stereo correlation indicates phase opposition"})); }
                if metrics.lufs > -8.0 { assistant_actions.push(json!({"priority":"medium","code":"reduce_loudness","message":"Integrated loudness is very high"})); }
                if metrics.mono_delta_db < -6.0 { assistant_actions.push(json!({"priority":"medium","code":"inspect_wide_elements","message":"Mono compatibility loss exceeds 6 dB"})); }
                let reference = (!reference_left.is_empty() && !reference_right.is_empty())
                    .then(|| crate::mix_analyzer::analyze(reference_left, reference_right));
                let ab = (!ab_left.is_empty() && !ab_right.is_empty())
                    .then(|| crate::mix_analyzer::analyze(ab_left, ab_right));
                Ok(serde_json::to_value(metrics)
                    .map(|metrics| {
                        let mut result = json!({"ok":true,"operation":"analyze_mix","metrics":metrics,"assistant":{"status":if assistant_actions.is_empty() { "ok" } else { "attention" },"actions":assistant_actions}});
                        if let Some(reference) = reference {
                            let reference_value = serde_json::to_value(reference).unwrap_or_else(|_| json!({}));
                            result["reference_metrics"] = reference_value.clone();
                            result["reference_delta"] = json!({
                                "lufs": metrics["lufs"].as_f64().unwrap_or(0.0) - reference_value["lufs"].as_f64().unwrap_or(0.0),
                                "true_peak_db": metrics["true_peak_db"].as_f64().unwrap_or(0.0) - reference_value["true_peak_db"].as_f64().unwrap_or(0.0),
                                "rms_db": metrics["rms_db"].as_f64().unwrap_or(0.0) - reference_value["rms_db"].as_f64().unwrap_or(0.0),
                                "correlation": metrics["correlation"].as_f64().unwrap_or(0.0) - reference_value["correlation"].as_f64().unwrap_or(0.0),
                            });
                        }
                        if let Some(ab) = ab {
                            let ab_value = serde_json::to_value(ab).unwrap_or_else(|_| json!({}));
                            result["ab_metrics"] = ab_value.clone();
                            result["ab_delta"] = json!({
                                "lufs": metrics["lufs"].as_f64().unwrap_or(0.0) - ab_value["lufs"].as_f64().unwrap_or(0.0),
                                "true_peak_db": metrics["true_peak_db"].as_f64().unwrap_or(0.0) - ab_value["true_peak_db"].as_f64().unwrap_or(0.0),
                                "rms_db": metrics["rms_db"].as_f64().unwrap_or(0.0) - ab_value["rms_db"].as_f64().unwrap_or(0.0),
                                "mono_delta_db": metrics["mono_delta_db"].as_f64().unwrap_or(0.0) - ab_value["mono_delta_db"].as_f64().unwrap_or(0.0),
                                "correlation": metrics["correlation"].as_f64().unwrap_or(0.0) - ab_value["correlation"].as_f64().unwrap_or(0.0),
                            });
                        }
                        result
                    })
                    .unwrap_or_else(|_| json!({"ok":false,"code":"mix_analysis_serialization_failed"})))
            }
            CommandAction::AnalyzeSilence { samples, threshold, min_length } => {
                let ranges = crate::silence_detector::detect_silence(samples, *threshold, *min_length as usize);
                Ok(json!({
                    "ok": true,
                    "operation": "analyze_silence",
                    "threshold": threshold,
                    "min_length": min_length,
                    "ranges": ranges.into_iter().map(|range| json!({"start": range.start, "length": range.length})).collect::<Vec<_>>(),
                }))
            }
            CommandAction::SplitRegionAtSilence { track_id, region_id, samples, threshold, min_length } => {
                let count = core.split_region_at_silence(*track_id, *region_id, samples, *threshold, *min_length as usize);
                Ok(json!({"ok":true,"operation":"split_region_at_silence","track_id":track_id,"region_id":region_id,"splits":count}))
            }
            CommandAction::PreviewVocalPitchCorrection { samples, sample_rate, speed, timing_ratio } => {
                let engine = crate::pitch_corrector::PitchCorrectorEngine::new(*sample_rate);
                let timed = if (*timing_ratio - 1.0).abs() < f32::EPSILON {
                    samples.clone()
                } else {
                    let output_len = ((samples.len() as f32) * *timing_ratio).round() as usize;
                    (0..output_len).map(|index| {
                        let source = index as f32 / *timing_ratio;
                        let left = source.floor() as usize;
                        let right = (left + 1).min(samples.len().saturating_sub(1));
                        let fraction = source - source.floor();
                        samples.get(left).copied().unwrap_or_default() * (1.0 - fraction)
                            + samples.get(right).copied().unwrap_or_default() * fraction
                    }).collect()
                };
                let corrected = engine.preview_mono(&timed, *speed);
                let changed_samples = samples.iter().zip(&corrected)
                    .filter(|(before, after)| (**before - **after).abs() > 1.0e-6)
                    .count();
                Ok(json!({
                    "ok": true,
                    "operation": "preview_vocal_pitch_correction",
                    "sample_rate": sample_rate,
                    "speed": speed,
                    "timing_ratio": timing_ratio,
                    "changed_samples": changed_samples,
                    "samples": corrected,
                }))
            }
            CommandAction::ApplyDynamicsSuggestion { track_id, plugin_index, samples } => {
                native_result(core.apply_dynamics_suggestion_diagnostic_json(
                    *track_id, *plugin_index, samples.clone(),
                ))
            }
            CommandAction::ExtensionCatalog { root } => {
                native_result(core.extension_catalog_json(root))
            }
            CommandAction::ExtensionValidate { root, extension_id, command_id, payload } => {
                let (commands, discovery_errors) = crate::extensions::command_registry(root);
                let qualified_id = format!("{extension_id}.{command_id}");
                match commands.iter().find(|item| item.qualified_id == qualified_id) {
                    None => Err(BridgeError::new(
                        "extension_command_not_found",
                        format!("extension command {qualified_id} is not registered"),
                    )),
                    Some(command) => crate::extensions::validate_command_payload(command, payload)
                        .map(|_| json!({
                            "ok": true,
                            "operation": "extension_validate",
                            "qualified_id": qualified_id,
                            "payload": payload,
                            "discovery_errors": discovery_errors,
                        }))
                        .map_err(|error| BridgeError::new("extension_payload_rejected", error)),
                }
            }
            CommandAction::ExtensionInvoke { root, extension_id, command_id, payload, timeout_ms } => {
                crate::extensions::invoke_command(root, extension_id, command_id, payload, *timeout_ms)
                    .map_err(|error| BridgeError::new("extension_invocation_failed", error))
            }
            CommandAction::ExtensionSetEnabled { root, extension_id, enabled } => {
                let raw = core.set_extension_enabled_json(root, extension_id, *enabled);
                native_result(raw)
            }
            CommandAction::AddTrack { track_type, name } => {
                let id = core.add_track(*track_type);
                if id == 0 {
                    Err(BridgeError::new("track_add_rejected", "native engine rejected track"))
                } else if core.set_track_name(id, name) {
                    Ok(json!({"ok":true,"operation":"add_track","track_id":id,"name":name}))
                } else {
                    Err(BridgeError::new("track_name_rejected", "native engine rejected track name"))
                }
            }
            CommandAction::AddAuxTrack { name } => {
                let id = core.add_aux_track();
                if id == 0 {
                    Err(BridgeError::new("aux_track_add_rejected", "native engine rejected Aux track"))
                } else if core.set_track_name(id, name) {
                    Ok(json!({"ok":true,"operation":"add_aux_track","track_id":id,"name":name,"track_type":"Aux"}))
                } else {
                    Err(BridgeError::new("track_name_rejected", "native engine rejected Aux track name"))
                }
            }
            CommandAction::RemoveTrack { track_id } => {
                if core.remove_track(*track_id) {
                    Ok(json!({"ok":true,"operation":"remove_track","track_id":track_id}))
                } else { Err(BridgeError::new("track_remove_rejected", "native engine rejected track removal")) }
            }
            CommandAction::DuplicateTrack { track_id } => {
                let id = core.duplicate_track(*track_id);
                if id == 0 { Err(BridgeError::new("track_duplicate_rejected", "native engine rejected track duplication")) }
                else { Ok(json!({"ok":true,"operation":"duplicate_track","source_track_id":track_id,"track_id":id})) }
            }
            CommandAction::AddVcaGroup { group_id, gain } => {
                if core.add_vca_group(*group_id, *gain) {
                    Ok(json!({"ok":true,"operation":"add_vca_group","group_id":group_id,"gain":gain}))
                } else { Err(BridgeError::new("vca_group_rejected", "native engine rejected VCA group")) }
            }
            CommandAction::AssignTrackToVca { track_id, group_id } => {
                if core.assign_track_to_vca(*track_id, *group_id) {
                    Ok(json!({"ok":true,"operation":"assign_track_to_vca","track_id":track_id,"group_id":group_id}))
                } else { Err(BridgeError::new("vca_assignment_rejected", "native engine rejected VCA assignment")) }
            }
            CommandAction::SetVcaGroupGain { group_id, gain } => {
                if core.set_vca_group_gain(*group_id, *gain) {
                    Ok(json!({"ok":true,"operation":"set_vca_group_gain","group_id":group_id,"gain":gain}))
                } else { Err(BridgeError::new("vca_gain_rejected", "native engine rejected VCA gain")) }
            }
            CommandAction::SetLowLatencyMode { enabled } => {
                if core.set_low_latency_mode(*enabled) {
                    Ok(json!({"ok":true,"operation":"set_low_latency_mode","enabled":enabled}))
                } else {
                    Err(BridgeError::new("low_latency_mode_rejected", "native PDC manager rejected the mode change"))
                }
            }
            CommandAction::SetTonalScale { root, scale_type } => {
                if core.set_tonal_scale(*root, *scale_type) {
                    Ok(json!({"ok":true,"operation":"set_tonal_scale","root":root,"scale_type":scale_type}))
                } else {
                    Err(BridgeError::new("tonal_scale_rejected", "native tonal engine rejected the scale"))
                }
            }
            CommandAction::SetPluginFavorite { id, favorite } => {
                native_result(core.set_plugin_favorite_diagnostic_json(id, *favorite))
            }
            CommandAction::PluginSearch { query, tag, favorites_only } => {
                let catalog: Value = serde_json::from_str(&core.installed_plugin_catalog_json())
                    .map_err(|error| BridgeError::new("invalid_plugin_catalog", error.to_string()))?;
                let query = query.trim().to_ascii_lowercase();
                let tag = tag.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(str::to_ascii_lowercase);
                let plugins = catalog.get("plugins").and_then(Value::as_array).into_iter().flatten()
                    .filter(|plugin| {
                        let text = ["id", "name", "format", "capability"].into_iter()
                            .filter_map(|key| plugin.get(key).and_then(Value::as_str))
                            .collect::<Vec<_>>().join(" ").to_ascii_lowercase();
                        let matches_query = query.is_empty() || text.contains(&query);
                        let matches_tag = tag.as_deref().is_none_or(|wanted| plugin.get("tags").and_then(Value::as_array)
                            .is_some_and(|tags| tags.iter().any(|value| value.as_str().is_some_and(|item| item.eq_ignore_ascii_case(wanted)))));
                        let matches_favorite = !*favorites_only || plugin.get("favorite").and_then(Value::as_bool) == Some(true);
                        matches_query && matches_tag && matches_favorite
                    }).cloned().collect::<Vec<_>>();
                Ok(json!({"ok":true,"operation":"plugin_search","plugins":plugins}))
            }
            CommandAction::SetTrackName { track_id, name } => {
                if core.set_track_name(*track_id, name) { Ok(json!({"ok":true,"operation":"set_track_name","track_id":track_id,"name":name})) }
                else { Err(BridgeError::new("track_name_rejected", "native engine rejected track name")) }
            }
            CommandAction::AddPlugin { track_id, plugin_type } => native_result(core.add_plugin_diagnostic_json(*track_id, *plugin_type)),
            CommandAction::InsertNamedPlugin { track_id, alias } => {
                let raw = core.add_named_sandboxed_plugin_diagnostic_json(*track_id, alias);
                let value: Value = serde_json::from_str(&raw)
                    .map_err(|error| BridgeError::new("invalid_native_result", error.to_string()))?;
                let result = value.get("result").cloned().unwrap_or(value.clone());
                if result.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(json!({"ok":true,"operation":"insert_named_plugin","track_id":track_id,"alias":alias,"plugin":value.get("plugin")}))
                } else {
                    Err(BridgeError::new(
                        result.get("code").and_then(Value::as_str).unwrap_or("plugin_insert_rejected"),
                        result.get("message").and_then(Value::as_str).unwrap_or("native engine rejected named plugin insertion"),
                    ))
                }
            }
            CommandAction::InsertPluginPath { track_id, path } => {
                native_result(core.add_sandboxed_plugin_diagnostic_json(*track_id, path))
            }
            CommandAction::OpenUtauImport { track_id, source_path, rendered_audio_path } => {
                native_result(core.openutau_import_diagnostic_json(source_path, rendered_audio_path))?;
                core.register_openutau_vocal(source_path, rendered_audio_path)
                    .map_err(|error| BridgeError::new("openutau_register_failed", error.to_string()))?;
                if !core.add_region_at_beat(*track_id, rendered_audio_path, 0.0) {
                    let _ = core.unregister_openutau_vocal(source_path, rendered_audio_path);
                    Err(BridgeError::new("openutau_region_rejected", "OpenUtau vocal could not be placed on the target track"))
                } else {
                    Ok(json!({
                        "ok": true,
                        "operation": "openutau_import",
                        "track_id": track_id,
                        "source_path": source_path,
                        "rendered_audio_path": rendered_audio_path,
                        "start_beat": 0.0,
                    }))
                }
            }
            CommandAction::OpenUtauNotes { source_path } => {
                if let Err(error) = crate::openutau::validate_source_file(source_path) {
                    Err(BridgeError::new("openutau_source_rejected", error))
                } else {
                    let notes = crate::openutau::parse_notes(source_path);
                    Ok(json!({
                        "ok": true,
                        "operation": "openutau_notes",
                        "source_path": source_path,
                        "note_count": notes.len(),
                        "notes": notes,
                    }))
                }
            }
            CommandAction::OpenUtauImportMidi {
                track_id,
                source_path,
                sample_rate,
                ticks_per_beat,
            } => {
                crate::openutau::validate_source_file(source_path)
                    .map_err(|error| BridgeError::new("openutau_source_rejected", error))?;
                let notes = crate::openutau::notes_as_midi(
                    source_path,
                    *track_id,
                    *sample_rate,
                    *ticks_per_beat,
                )
                .map_err(|error| BridgeError::new("openutau_midi_conversion_failed", error))?;
                let note_count = notes.len();
                for note in notes {
                    core.set_midi_note(
                        note.track_id,
                        note.pitch,
                        note.velocity,
                        note.start_sample,
                        note.length_samples,
                    );
                }
                Ok(json!({
                    "ok": true,
                    "operation": "openutau_import_midi",
                    "track_id": track_id,
                    "source_path": source_path,
                    "sample_rate": sample_rate,
                    "ticks_per_beat": ticks_per_beat,
                    "note_count": note_count,
                }))
            }
            CommandAction::AddAudioRegion { track_id, path, start } => {
                native_result(core.add_region_diagnostic_json(*track_id, path, *start))
            }
            CommandAction::ReplaceRegionAudio { track_id, region_id, path } => {
                native_result(core.replace_region_audio_diagnostic_json(*track_id, *region_id, path))
            }
            CommandAction::PluginCatalog => {
                let catalog: Value = serde_json::from_str(&core.installed_plugin_catalog_json())
                    .map_err(|error| BridgeError::new("invalid_plugin_catalog", error.to_string()))?;
                Ok(json!({"ok":true,"operation":"plugin_catalog","plugins":catalog}))
            }
            CommandAction::ControlInspect => {
                let snapshot: Value = serde_json::from_str(&core.control_snapshot_json())
                    .map_err(|error| BridgeError::new("invalid_control_snapshot", error.to_string()))?;
                Ok(json!({"ok":true,"operation":"control_inspect","snapshot":snapshot}))
            }
            CommandAction::ProjectSearch { query } => {
                let snapshot: Value = serde_json::from_str(&core.control_snapshot_json())
                    .map_err(|error| BridgeError::new("invalid_control_snapshot", error.to_string()))?;
                let needle = query.trim().to_ascii_lowercase();
                let mut hits = Vec::new();
                search_snapshot(&snapshot, &needle, "$", &mut hits);
                Ok(json!({
                    "ok": true,
                    "operation": "project_search",
                    "query": query,
                    "project_generation": core.project_generation(),
                    "audio_generation": core.audio_config_generation(),
                    "truncated": hits.len() == 512,
                    "hits": hits,
                }))
            }
            CommandAction::ProjectInspect => {
                let layout: Value = serde_json::from_str(&core.get_project_layout_json())
                    .map_err(|error| BridgeError::new("invalid_project_snapshot", error.to_string()))?;
                Ok(json!({
                    "ok": true,
                    "operation": "project_inspect",
                    "project_generation": core.project_generation(),
                    "audio_generation": core.audio_config_generation(),
                    "master_gain": core.master_gain(),
                    "markers": serde_json::from_str::<Value>(&core.markers_json()).unwrap_or_else(|_| json!([])),
                    "audio_routes": serde_json::from_str::<Value>(&core.audio_routes_json()).unwrap_or_else(|_| json!([])),
                    "track_stacks": serde_json::from_str::<Value>(&core.track_stacks_json()).unwrap_or_else(|_| json!([])),
                    "macro_mappings": serde_json::from_str::<Value>(&core.macro_mappings_json()).unwrap_or_else(|_| json!([])),
                    "midi_learn_mappings": serde_json::from_str::<Value>(&core.midi_learn_mappings_json()).unwrap_or_else(|_| json!([])),
                    "vca_groups": serde_json::from_str::<Value>(&core.get_vca_snapshot_json()).unwrap_or_else(|_| json!([])),
                    "aux_track_ids": serde_json::from_str::<Value>(&core.aux_track_ids_json()).unwrap_or_else(|_| json!([])),
                    "comping": serde_json::from_str::<Value>(&core.comping_snapshot_json()).unwrap_or_else(|_| json!({"takes": [], "current_comp": []})),
                    "chord_track": serde_json::from_str::<Value>(&core.chord_track_json()).unwrap_or_else(|_| json!([])),
                    "midi_notes": serde_json::from_str::<Value>(&core.midi_notes_json()).unwrap_or_else(|_| json!([])),
                    "layout": layout,
                }))
            }
            CommandAction::RenderTargetCatalog => {
                native_result(core.render_target_catalog_diagnostic_json())
            }
            CommandAction::FreezeTrack { track_id, total_samples, path } => {
                if let Some(path) = path {
                    native_result(core.freeze_track_to_file_diagnostic_json(*track_id, path, *total_samples))
                } else {
                    native_result(core.freeze_track_diagnostic_json(*track_id, *total_samples))
                }
            }
            CommandAction::FreezeTrackToProjectEnd { track_id } => {
                native_result(core.freeze_track_to_project_end_diagnostic_json(*track_id))
            }
            CommandAction::UnfreezeTrack { track_id } => native_result(core.unfreeze_track_diagnostic_json(*track_id)),
            CommandAction::TrackFreezeStatus { track_id } => native_result(core.track_freeze_status_diagnostic_json(*track_id)),
            CommandAction::RemovePlugin { track_id, plugin_index } => native_result(core.remove_plugin_diagnostic_json(*track_id, *plugin_index)),
            CommandAction::MovePlugin { track_id, from_index, to_index } => {
                if core.move_plugin(*track_id, *from_index, *to_index) {
                    Ok(json!({"ok":true,"operation":"move_plugin","track_id":track_id,"from_index":from_index,"to_index":to_index}))
                } else {
                    Err(BridgeError::new("plugin_move_rejected", "native engine rejected plugin reorder"))
                }
            }
            CommandAction::SetPluginParameter { track_id, plugin_index, parameter_id, value } => native_result(core.set_plugin_parameter_diagnostic_json(*track_id, *plugin_index, *parameter_id, *value)),
            CommandAction::SetPluginBypass { track_id, plugin_index, bypassed } => native_result(core.set_plugin_bypass_diagnostic_json(*track_id, *plugin_index, *bypassed)),
            CommandAction::SetVolume { track_id, value } => native_result(core.set_volume_diagnostic_json(*track_id, *value)),
            CommandAction::ApplyGainStaging { track_id, gain_db } => native_result(core.apply_gain_staging_diagnostic_json(*track_id, *gain_db)),
            CommandAction::SetEq { track_id, low_band, low_cut, high_band, high_cut } => native_result(core.set_eq_diagnostic_json(*track_id, *low_band, *low_cut, *high_band, *high_cut)),
            CommandAction::SetMasterGain { value } => native_result(core.set_master_gain_diagnostic_json(*value)),
            CommandAction::SetTrackDelay { track_id, samples } => native_result(core.set_track_delay_samples_diagnostic_json(*track_id, *samples)),
            CommandAction::CreateTrackStack { stack_id, name, member_track_ids, master_gain, collapsed } => {
                if core.upsert_track_stack(*stack_id, name, member_track_ids, *master_gain, *collapsed) {
                    Ok(json!({"ok":true,"operation":"set_track_stack","stack_id":stack_id}))
                } else {
                    Err(BridgeError::new("track_stack_rejected", "track stack definition was rejected"))
                }
            }
            CommandAction::DeleteTrackStack { stack_id } => {
                if core.delete_track_stack(*stack_id) {
                    Ok(json!({"ok":true,"operation":"delete_track_stack","stack_id":stack_id}))
                } else {
                    Err(BridgeError::new("track_stack_delete_rejected", "track stack was not found or could not be deleted"))
                }
            }
            CommandAction::UpsertMarker { marker_id, label, beat, color } => {
                if core.upsert_marker(*marker_id, label, *beat, color) {
                    Ok(json!({"ok":true,"operation":"upsert_marker","marker_id":marker_id}))
                } else {
                    Err(BridgeError::new("marker_rejected", "arrangement marker was rejected"))
                }
            }
            CommandAction::DeleteMarker { marker_id } => {
                if core.delete_marker(*marker_id) {
                    Ok(json!({"ok":true,"operation":"delete_marker","marker_id":marker_id}))
                } else {
                    Err(BridgeError::new("marker_not_found", "arrangement marker was not found"))
                }
            }
            CommandAction::SetTrackStackGain { stack_id, master_gain } => {
                if core.set_track_stack_gain(*stack_id, *master_gain) {
                    Ok(json!({"ok":true,"operation":"set_track_stack_gain","stack_id":stack_id,"master_gain":master_gain}))
                } else {
                    Err(BridgeError::new("track_stack_gain_rejected", "track stack gain was rejected"))
                }
            }
            CommandAction::SetTrackStackCollapsed { stack_id, collapsed } => {
                if core.set_track_stack_collapsed(*stack_id, *collapsed) {
                    Ok(json!({"ok":true,"operation":"set_track_stack_collapsed","stack_id":stack_id,"collapsed":collapsed}))
                } else {
                    Err(BridgeError::new("track_stack_not_found", "track stack was not found"))
                }
            }
            CommandAction::SetPan { track_id, value } => native_result(core.set_pan_diagnostic_json(*track_id, *value)),
            CommandAction::SetMute { track_id, muted } => native_result(core.set_mute_diagnostic_json(*track_id, *muted)),
            CommandAction::SetSolo { track_id, solo } => native_result(core.set_solo_diagnostic_json(*track_id, *solo)),
            CommandAction::SetTrackArmed { track_id, armed } => native_result(core.set_track_armed_diagnostic_json(*track_id, *armed)),
            CommandAction::SetPhaseInvert { track_id, inverted } => native_result(core.set_phase_invert_diagnostic_json(*track_id, *inverted)),
            CommandAction::SetRoute { source_id, dest_id, enabled } => native_result(core.set_route_diagnostic_json(*source_id, *dest_id, *enabled)),
            CommandAction::SetRouteGain { source_id, dest_id, gain, enabled } => {
                native_result(core.set_route_gain_diagnostic_json(
                    *source_id, *dest_id, *gain, *enabled,
                ))
            }
            CommandAction::SetFeedbackRoute { source_id, dest_id, gain, enabled } => native_result(core.set_feedback_route_diagnostic_json(*source_id, *dest_id, *gain, *enabled)),
            CommandAction::SetSidechainLink { source_id, dest_id, tap_point, plugin_index, enabled } => native_result(core.set_sidechain_link_diagnostic_json(*source_id, *dest_id, *plugin_index, *tap_point, *enabled)),
            CommandAction::TransportPlay => native_result(core.set_playing_diagnostic_json(true)),
            CommandAction::TransportPause => native_result(core.set_playing_diagnostic_json(false)),
            CommandAction::TransportStop => native_result(core.set_playing_diagnostic_json(false)),
            CommandAction::SetPlayhead { position } => native_result(core.set_playhead_diagnostic_json(*position)),
            CommandAction::SetLoop { enabled } => native_result(core.set_loop_diagnostic_json(*enabled)),
            CommandAction::SetMetronome { enabled } => native_result(core.set_metronome_diagnostic_json(*enabled)),
            CommandAction::SetCycleRange { start_sample, end_sample, enabled } => native_result(core.set_cycle_range_diagnostic_json(*start_sample, *end_sample, *enabled)),
            CommandAction::SetTempo { bpm } => native_result(core.set_tempo_diagnostic_json(*bpm)),
            CommandAction::SetTimeSignature { beat, numerator, denominator } => native_result(core.set_time_signature_event_diagnostic_json(*beat, *numerator, *denominator)),
            CommandAction::SetMacroValue { macro_index, value } => native_result(core.set_macro_value_diagnostic_json(*macro_index, *value)),
            CommandAction::HumanizeMidi { timing_beats, velocity, seed } => native_result(core.humanize_midi_diagnostic_json(*timing_beats, i16::try_from(*velocity).unwrap_or(0), *seed)),
            CommandAction::ApplyMidiSwing { subdivision_beats, amount } => native_result(core.apply_midi_swing_diagnostic_json(*subdivision_beats, *amount)),
            CommandAction::QuantizeMidi { grid_beats, strength } => native_result(core.quantize_midi_diagnostic_json(*grid_beats, *strength)),
            CommandAction::ApplyMidiLogicalRule { rule } => {
                let mut notes: Vec<crate::project_contracts::MidiNoteContract> = serde_json::from_str(&core.midi_notes_json())
                    .map_err(|error| BridgeError::new("midi_notes_unavailable", error.to_string()))?;
                let changed = crate::midi_logical_editor::apply_rule(&mut notes, rule)
                    .map_err(|error| BridgeError::new("midi_logical_rule_rejected", error))?;
                if !core.replace_midi_note_contracts(notes, true) {
                    return Err(BridgeError::new("midi_logical_rule_rejected", "native MIDI snapshot replacement was rejected"));
                }
                Ok(json!({"ok": true, "operation": "apply_midi_logical_rule", "changed": changed}))
            }
            CommandAction::TakeMixSnapshot { name, states } => {
                native_result(core.take_mix_snapshot_json(name, &serde_json::to_string(states).unwrap_or_default()))
            }
            CommandAction::CaptureMixSnapshot { name } => {
                let layout: Value = serde_json::from_str(&core.get_project_layout_json())
                    .map_err(|error| BridgeError::new("snapshot_capture_failed", error.to_string()))?;
                let mut states = std::collections::HashMap::new();
                let mut plugin_states = Vec::new();
                if let Some(tracks) = layout.as_array() {
                    for track in tracks {
                        let Some(id) = track.get("id").and_then(Value::as_u64) else { continue; };
                        if id > u32::MAX as u64 / 4 { continue; }
                        if let Some(volume) = track.get("volume").and_then(Value::as_f64) {
                            states.insert((id as u32) * 4, volume as f32);
                        }
                        if let Some(pan) = track.get("pan").and_then(Value::as_f64) {
                            states.insert((id as u32) * 4 + 1, pan as f32);
                        }
                        if let Some(muted) = track.get("mute").and_then(Value::as_bool) {
                            states.insert((id as u32) * 4 + 2, if muted { 1.0 } else { 0.0 });
                        }
                        if let Some(solo) = track.get("solo").and_then(Value::as_bool) {
                            states.insert((id as u32) * 4 + 3, if solo { 1.0 } else { 0.0 });
                        }
                        if let (Some(parameters), Some(bypasses)) = (
                            track.get("plugin_parameter_values").and_then(Value::as_array),
                            track.get("plugin_bypass").and_then(Value::as_array),
                        ) {
                            for (plugin_index, parameter_values) in parameters.iter().enumerate() {
                                let values = parameter_values.as_array().map(|items| items.iter()
                                    .filter_map(Value::as_f64).map(|value| value as f32).collect::<Vec<_>>())
                                    .unwrap_or_default();
                                let bypassed = bypasses.get(plugin_index).and_then(Value::as_bool).unwrap_or(false);
                                if values.iter().all(|value| value.is_finite()) {
                                    plugin_states.push(crate::snapshots::PluginSnapshotState {
                                        track_id: id as u32,
                                        plugin_index: plugin_index as u32,
                                        parameters: values,
                                        bypassed,
                                    });
                                }
                            }
                        }
                    }
                }
                native_result(core.take_mix_snapshot_with_plugins_and_routing_json(
                    name,
                    &serde_json::to_string(&states).unwrap_or_default(),
                    &serde_json::to_string(&plugin_states).unwrap_or_default(),
                    &core.audio_routes_json(),
                ))
            }
            CommandAction::DiffMixSnapshots { first, second } => {
                native_result(core.diff_mix_snapshots_json(*first, *second))
            }
            CommandAction::RecallMixSnapshot { index } => {
                native_result(core.recall_mix_snapshot_json(*index))
            }
            CommandAction::ApplyMixSnapshot { index } => {
                native_result(core.apply_mix_snapshot_json(*index))
            }
            CommandAction::AddMacroMapping { mapping_id, macro_index, target_instance_id, target_parameter_id, min, max, curve, invert } => core
                .add_macro_mapping(crate::project_contracts::MacroMappingContract {
                    mapping_id: mapping_id.clone(),
                    macro_index: (*macro_index).try_into().unwrap_or(127),
                    target_instance_id: target_instance_id.clone(),
                    target_parameter_id: target_parameter_id.clone(),
                    min: *min,
                    max: *max,
                    curve: *curve,
                    invert: *invert,
                })
                .map(|_| json!({"ok":true,"operation":"add_macro_mapping","mapping_id":mapping_id}))
                .map_err(|error| BridgeError::new("macro_mapping_rejected", error.to_string())),
            CommandAction::RemoveMacroMapping { mapping_id } => {
                if core.remove_macro_mapping(mapping_id) { Ok(json!({"ok":true,"operation":"remove_macro_mapping","mapping_id":mapping_id})) }
                else { Err(BridgeError::new("macro_mapping_not_found", "macro mapping was not found")) }
            }
            CommandAction::AddMidiLearnMapping { mapping_id, device_id, channel, controller, target_instance_id, target_parameter_id, min, max, curve, pickup } => core
                .add_midi_learn_mapping(crate::project_contracts::MidiLearnMappingContract {
                    mapping_id: mapping_id.clone(),
                    device_id: device_id.clone(),
                    channel: (*channel).try_into().unwrap_or(15),
                    controller: (*controller).try_into().unwrap_or(16_383),
                    target_instance_id: target_instance_id.clone(),
                    target_parameter_id: target_parameter_id.clone(),
                    min: *min,
                    max: *max,
                    curve: *curve,
                    pickup: *pickup,
                    macro_group: None,
                })
                .map(|_| json!({"ok":true,"operation":"add_midi_learn_mapping","mapping_id":mapping_id}))
                .map_err(|error| BridgeError::new("midi_mapping_rejected", error.to_string())),
            CommandAction::RemoveMidiLearnMapping { mapping_id } => {
                if core.remove_midi_learn_mapping(mapping_id) { Ok(json!({"ok":true,"operation":"remove_midi_learn_mapping","mapping_id":mapping_id})) }
                else { Err(BridgeError::new("midi_mapping_not_found", "MIDI mapping was not found")) }
            }
            CommandAction::SetAutomation { track_id, parameter_id, points } => native_result(core.set_automation_data_diagnostic_json(*track_id, *parameter_id, points.clone())),
            CommandAction::SetTrackDelayAutomation { track_id, points } => native_result(core.set_track_delay_automation_diagnostic_json(*track_id, points.clone())),
            CommandAction::SetMidiNote { track_id, pitch, velocity, start_sample, length_samples, lyric, phoneme, pitch_curve_cents, vibrato_depth_cents, portamento_samples } => {
                let result = native_result(core.set_midi_note_lyric_diagnostic_json(
                    *track_id, *pitch, *velocity, *start_sample, *length_samples, lyric,
                ));
                if result.is_ok() && (!phoneme.is_empty() || !pitch_curve_cents.is_empty() || *vibrato_depth_cents != 0 || *portamento_samples != 0)
                    && !core.set_midi_note_articulation(*track_id, *pitch, *start_sample, phoneme, pitch_curve_cents, *vibrato_depth_cents, *portamento_samples) {
                    return Err(BridgeError::new("midi_articulation_rejected", "vocal note articulation was rejected"));
                }
                result
            }
            CommandAction::ClearMidiNotes => native_result(core.clear_midi_notes_diagnostic_json()),
            CommandAction::RemoveMidiNotesRange { track_id, start_sample, end_sample } => native_result(core.remove_midi_notes_range_diagnostic_json(*track_id, *start_sample, *end_sample)),
            CommandAction::TransposeMidiNotesRange { track_id, start_sample, end_sample, semitones } => native_result(core.transpose_midi_notes_range_diagnostic_json(*track_id, *start_sample, *end_sample, *semitones)),
            CommandAction::MoveMidiNotesRange { track_id, start_sample, end_sample, delta_samples } => native_result(core.move_midi_notes_range_diagnostic_json(*track_id, *start_sample, *end_sample, *delta_samples)),
            CommandAction::MoveRegion { track_id, region_id, start } => native_result(core.move_region_diagnostic_json(*track_id, *region_id, *start)),
            CommandAction::SplitRegion { track_id, region_id, beat } => native_result(core.split_region_diagnostic_json(*track_id, *region_id, *beat)),
            CommandAction::SplitRegionWithCrossfade { track_id, region_id, beat, ratio } => {
                if core.split_region_with_auto_crossfade(*track_id, *region_id, *beat, *ratio) {
                    Ok(json!({"ok":true,"operation":"split_region_with_crossfade","track_id":track_id,"region_id":region_id,"beat":beat,"ratio":ratio}))
                } else { Err(BridgeError::new("crossfade_split_rejected", "region split or automatic crossfade was rejected")) }
            }
            CommandAction::DuplicateRegion { track_id, region_id, start } => {
                let new_id = core.duplicate_region(*track_id, *region_id, *start);
                if new_id != 0 {
                    native_result(serde_json::json!({"ok":true,"operation":"duplicate_region","track_id":track_id,"source_region_id":region_id,"region_id":new_id,"start":start}).to_string())
                } else {
                    Err(BridgeError::new("region_duplicate_rejected", "region duplication was rejected"))
                }
            }
            CommandAction::RemoveRegion { track_id, region_id } => {
                if core.remove_region(*track_id, *region_id) {
                    native_result(serde_json::json!({"ok":true,"operation":"remove_region","track_id":track_id,"region_id":region_id}).to_string())
                } else {
                    Err(BridgeError::new("region_remove_rejected", "region removal was rejected"))
                }
            }
            CommandAction::SetRegionFades { track_id, region_id, fade_in, fade_out } => native_result(core.set_region_fades_diagnostic_json(*track_id, *region_id, *fade_in, *fade_out)),
            CommandAction::SetRegionTrim { track_id, region_id, start, end } => native_result(core.set_region_trim_diagnostic_json(*track_id, *region_id, *start, *end)),
            CommandAction::SetRegionLoop { track_id, region_id, count } => native_result(core.set_region_loop_diagnostic_json(*track_id, *region_id, *count)),
            CommandAction::SetRegionReverse { track_id, region_id, reverse } => native_result(core.set_region_reverse_diagnostic_json(*track_id, *region_id, *reverse)),
            CommandAction::SetRegionMuted { track_id, region_id, muted } => native_result(core.set_region_muted_diagnostic_json(*track_id, *region_id, *muted)),
            CommandAction::SetRegionWarp { track_id, region_id, ratio } => native_result(core.set_region_warp_ratio_diagnostic_json(*track_id, *region_id, *ratio)),
            CommandAction::SetRegionGain { track_id, region_id, gain_db } => native_result(core.set_region_gain_diagnostic_json(*track_id, *region_id, *gain_db)),
            CommandAction::SetRegionPitch { track_id, region_id, semitones } => native_result(core.set_region_pitch_diagnostic_json(*track_id, *region_id, *semitones)),
            CommandAction::SetRegionAudioNoteSegment { track_id, region_id, start_seconds, end_seconds, pitch_offset_cents, formant_offset_cents } => native_result(core.set_region_audio_note_segment_diagnostic_json(*track_id, *region_id, *start_seconds, *end_seconds, *pitch_offset_cents, *formant_offset_cents)),
            CommandAction::ClearRegionAudioNoteSegments { track_id, region_id } => native_result(core.clear_region_audio_note_segments_diagnostic_json(*track_id, *region_id)),
            CommandAction::WarpRegionAudioNoteSegment { track_id, region_id, segment_start_seconds, new_start_seconds, new_end_seconds } => native_result(core.warp_region_audio_note_segment_diagnostic_json(*track_id, *region_id, *segment_start_seconds, *new_start_seconds, *new_end_seconds)),
            CommandAction::RemoveRegionAudioNoteSegment { track_id, region_id, segment_start_seconds } => native_result(core.remove_region_audio_note_segment_diagnostic_json(*track_id, *region_id, *segment_start_seconds)),
            CommandAction::Undo => native_result(core.undo_diagnostic_json()),
            CommandAction::Redo => native_result(core.redo_diagnostic_json()),
            CommandAction::ProjectLoad { path } => core
                .load_project_v2(path)
                .map(|_| json!({"ok":true,"operation":"project_load","path":path}))
                .map_err(|error| BridgeError::new("project_load_failed", error.to_string())),
            CommandAction::SaveProject { path } => {
                if core.save_project(path) {
                    Ok(json!({"ok":true,"operation":"save_project","path":path}))
                } else {
                    Err(BridgeError::new("project_save_failed", "native project save was rejected"))
                }
            }
            CommandAction::BounceProject { path, format } => {
                native_result(core.bounce_project_diagnostic_json(path, *format))
            }
            CommandAction::BounceStems { output_dir, format, track_ids, tail_seconds, pre_fader, include_inserts } => {
                native_result(core.bounce_stems_diagnostic_json(
                    output_dir,
                    *format,
                    track_ids,
                    *tail_seconds,
                    *pre_fader,
                    *include_inserts,
                ))
            }
            CommandAction::RecordArm { sample_rate, channels, max_frames } => core
                .arm_recording_capture(*sample_rate, *channels, *max_frames as usize)
                .map(|_| json!({"ok":true,"operation":"record_arm"}))
                .map_err(|error| BridgeError::new("record_arm_failed", error.to_string())),
            CommandAction::RecordStart { sample_rate, channels, max_frames, start_sample, count_in_frames } => core
                .start_recording_capture_with_count_in(
                    *sample_rate, *channels, *max_frames as usize,
                    *start_sample, *count_in_frames,
                )
                .map(|_| json!({"ok":true,"operation":"record_start","start_sample":start_sample,"count_in_frames":count_in_frames}))
                .map_err(|error| BridgeError::new("record_start_failed", error.to_string())),
            CommandAction::RecordStop => core
                .stop_recording_preview()
                .map(|region| json!({"ok":true,"operation":"record_stop","frames":region.frame_count()}))
                .map_err(|error| BridgeError::new("record_stop_failed", error.to_string())),
            CommandAction::RecordCommit { track_id, project_path } => core
                .commit_recording_capture_to_track(*track_id, project_path.as_deref())
                .map(|frames| json!({"ok":true,"operation":"record_commit","track_id":track_id,"frames":frames}))
                .map_err(|error| BridgeError::new("record_commit_failed", error.to_string())),
            CommandAction::SelectRecordingTake { index } => native_result(core.select_recording_take_diagnostic_json(*index as usize)),
            CommandAction::RegisterCompTake { take_id, name, start_sample, end_sample } => {
                if core.register_comp_take(*take_id, name, *start_sample, *end_sample) {
                    Ok(json!({"ok":true,"operation":"register_comp_take","take_id":take_id}))
                } else { Err(BridgeError::new("comp_take_rejected", "recording take was rejected")) }
            }
            CommandAction::SelectCompTake { take_id } => {
                if core.select_comp_take(*take_id) {
                    Ok(json!({"ok":true,"operation":"select_comp_take","take_id":take_id}))
                } else { Err(BridgeError::new("comp_take_selection_rejected", "comp take selection was rejected")) }
            }
            CommandAction::RemoveCompTake { take_id } => {
                if core.remove_comp_take(*take_id) {
                    Ok(json!({"ok":true,"operation":"remove_comp_take","take_id":take_id}))
                } else { Err(BridgeError::new("comp_take_remove_rejected", "take is missing or still referenced by the active comp")) }
            }
            CommandAction::SetCompSegments { segments } => {
                let packed = segments.iter().map(|segment| (
                    segment.take_id, segment.start_sample, segment.length_samples,
                    segment.crossfade_samples,
                )).collect::<Vec<_>>();
                if core.set_comp_segments(&packed) {
                    Ok(json!({"ok":true,"operation":"set_comp_segments","count":segments.len()}))
                } else { Err(BridgeError::new("comp_segments_rejected", "comp segments were rejected")) }
            }
        };
        match result {
            Ok(value) => results.push(value),
            Err(error) => {
                let mut rolled_back = false;
                while core.undo_depth() > undo_before {
                    core.undo();
                    rolled_back = true;
                }
                let stack_rolled_back = core.restore_track_stacks_json(&stack_snapshot_before);
                let markers_rolled_back = core.restore_markers_json(&marker_snapshot_before);
                let mut extensions_rolled_back = true;
                for (root, extension_id, enabled) in &extension_activation_before {
                    if crate::extensions::set_enabled(root, extension_id, *enabled).is_err() {
                        extensions_rolled_back = false;
                    }
                }
                return Err(BridgeError::new(
                    "transaction_apply_failed",
                    format!("{}; compensating rollback attempted={} stack_restore={stack_rolled_back} marker_restore={markers_rolled_back} extension_restore={extensions_rolled_back}", error.message, rolled_back),
                ));
            }
        }
    }
    Ok(ExecutionReport {
        transaction: command.transaction.clone(),
        applied: results.len(),
        results,
        rolled_back: false,
    })
}

#[cfg(test)]
mod tests {
    use super::execute;
    use crate::command_api::{validate, CommandAction, CommandDocument, Permission};
    use crate::AuraCore;

    #[test]
    fn project_inspect_exposes_midi_and_chord_state() {
        let core = AuraCore::new().expect("core must initialize");
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "inspect-project".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::ProjectInspect],
        })
        .expect("project inspection must validate");
        let report = execute(&core, &command).expect("project inspection must execute");
        let result = &report.results[0];
        assert!(result
            .get("midi_notes")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("chord_track")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("vca_groups")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("track_stacks")
            .is_some_and(|value| value.is_array()));
        assert!(result
            .get("macro_mappings")
            .is_some_and(|value| value.is_array()));
    }

    #[test]
    fn set_midi_note_applies_vocal_articulation_metadata() {
        let core = AuraCore::new().expect("core must initialize");
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "vocal-note".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(core.project_generation()),
            expected_audio_generation: Some(core.audio_config_generation()),
            actions: vec![CommandAction::SetMidiNote {
                track_id: 1,
                pitch: 60,
                velocity: 100,
                start_sample: 0,
                length_samples: 48_000,
                lyric: "la".into(),
                phoneme: "a".into(),
                pitch_curve_cents: vec![0, 20, -10],
                vibrato_depth_cents: 32,
                portamento_samples: 960,
            }],
        })
        .expect("vocal note command must validate");
        execute(&core, &command).expect("vocal note command must execute");
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(notes[0]["phoneme"], "a");
        assert_eq!(
            notes[0]["pitch_curve_cents"],
            serde_json::json!([0, 20, -10])
        );
        assert_eq!(notes[0]["vibrato_depth_cents"], 32);
        assert_eq!(notes[0]["portamento_samples"], 960);
        core.undo();
        let after_undo: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_ne!(after_undo[0]["phoneme"], "a");
        core.redo();
        let after_redo: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(after_redo[0]["phoneme"], "a");
        assert_eq!(after_redo[0]["vibrato_depth_cents"], 32);
    }

    #[test]
    fn logical_editor_rule_executes_through_command_and_undo() {
        let core = AuraCore::new().expect("core must initialize");
        let add = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "logical-editor-seed".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(core.project_generation()),
            expected_audio_generation: Some(core.audio_config_generation()),
            actions: vec![CommandAction::SetMidiNote {
                track_id: 1,
                pitch: 60,
                velocity: 40,
                start_sample: 0,
                length_samples: 480,
                lyric: String::new(),
                phoneme: String::new(),
                pitch_curve_cents: Vec::new(),
                vibrato_depth_cents: 0,
                portamento_samples: 0,
            }],
        })
        .unwrap();
        execute(&core, &add).unwrap();
        let rule = crate::midi_logical_editor::MidiLogicalRule {
            predicate: crate::midi_logical_editor::MidiNotePredicate {
                track_id: Some(1),
                ..Default::default()
            },
            transforms: vec![
                crate::midi_logical_editor::MidiNoteTransform::Transpose { semitones: 12 },
                crate::midi_logical_editor::MidiNoteTransform::SetVelocity { velocity: 100 },
            ],
        };
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "logical-editor".into(),
            permission: Permission::ProjectWrite,
            expected_generation: Some(core.project_generation()),
            expected_audio_generation: Some(core.audio_config_generation()),
            actions: vec![CommandAction::ApplyMidiLogicalRule { rule }],
        })
        .unwrap();
        let report = execute(&core, &command).unwrap();
        assert_eq!(report.results[0]["changed"], 1);
        let notes: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(notes[0]["pitch"], 72);
        assert_eq!(notes[0]["velocity"], 100);
        core.undo();
        let restored: serde_json::Value = serde_json::from_str(&core.midi_notes_json()).unwrap();
        assert_eq!(restored[0]["pitch"], 60);
        assert_eq!(restored[0]["velocity"], 40);
    }

    #[test]
    fn mix_snapshot_commands_capture_and_diff_states() {
        let core = AuraCore::new().expect("core must initialize");
        let capture = |name: &str, states| {
            validate(CommandDocument {
                schema_version: 1,
                command_version: 1,
                transaction: format!("snapshot-{name}"),
                permission: Permission::ProjectWrite,
                expected_generation: Some(core.project_generation()),
                expected_audio_generation: Some(core.audio_config_generation()),
                actions: vec![CommandAction::TakeMixSnapshot {
                    name: name.into(),
                    states,
                }],
            })
            .unwrap()
        };
        execute(
            &core,
            &capture("A", std::collections::HashMap::from([(1, 0.5)])),
        )
        .unwrap();
        execute(
            &core,
            &capture("B", std::collections::HashMap::from([(1, 0.8), (2, 0.2)])),
        )
        .unwrap();
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "snapshot-diff".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::DiffMixSnapshots {
                first: 0,
                second: 1,
            }],
        })
        .unwrap();
        let report = execute(&core, &command).unwrap();
        assert_eq!(report.results[0]["diff"].as_array().unwrap().len(), 2);
        let recall = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "snapshot-recall".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::RecallMixSnapshot { index: 1 }],
        })
        .unwrap();
        let recalled = execute(&core, &recall).unwrap();
        assert!((recalled.results[0]["states"]["2"].as_f64().unwrap() - 0.2).abs() < 1.0e-5);
    }

    #[test]
    fn vocal_pitch_preview_is_read_only_and_bounded() {
        let core = AuraCore::new().expect("core must initialize");
        let command = validate(CommandDocument {
            schema_version: 1,
            command_version: 1,
            transaction: "vocal-preview".into(),
            permission: Permission::ReadOnly,
            expected_generation: None,
            expected_audio_generation: None,
            actions: vec![CommandAction::PreviewVocalPitchCorrection {
                samples: vec![0.1, -0.1, 0.1, -0.1],
                sample_rate: 48_000.0,
                speed: 0.75,
                timing_ratio: 2.0,
            }],
        })
        .expect("vocal preview must validate");
        let before = core.project_generation();
        let report = execute(&core, &command).expect("vocal preview must execute");
        assert_eq!(core.project_generation(), before);
        assert_eq!(
            report.results[0]["operation"],
            "preview_vocal_pitch_correction"
        );
        assert_eq!(report.results[0]["timing_ratio"], 2.0);
        assert_eq!(report.results[0]["samples"].as_array().unwrap().len(), 8);
    }
}
