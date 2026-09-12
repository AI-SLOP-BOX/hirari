
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ProtocolRequest {
    pub protocol: String,
    pub request_id: String,
    pub client: String,
    pub command: CommandDocument,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProtocolResponse<T: Serialize> {
    pub protocol: String,
    pub request_id: String,
    pub ok: bool,
    pub result: Option<T>,
    pub error: Option<ProtocolError>,
}

/// Protocol errors and runtime/FFI errors intentionally share one wire type.
/// This prevents code/message/retry/generation context from being lost at
/// adapter boundaries.
pub type ProtocolError = BridgeError;

pub const PROTOCOL_VERSION: &str = "aura.command.v1";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CompSegmentCommand {
    pub take_id: u32,
    pub start_sample: u64,
    pub length_samples: u64,
    #[serde(default)]
    pub crossfade_samples: u32,
}

/// Validate the envelope before inspecting or applying its command. Keeping
/// this at the wire boundary prevents adapters from accidentally accepting a
/// command from a different protocol revision or an untraceable client.
pub fn validate_request_envelope(request: &ProtocolRequest) -> Result<(), BridgeError> {
    if request.protocol != PROTOCOL_VERSION {
        return Err(BridgeError::new(
            "unsupported_protocol",
            format!("expected {PROTOCOL_VERSION}, got {}", request.protocol),
        ));
    }
    if !valid_token(&request.request_id, 128) {
        return Err(BridgeError::new(
            "invalid_request_id",
            "request_id must be 1..=128 characters",
        ));
    }
    if !valid_token(&request.client, 128) {
        return Err(BridgeError::new(
            "invalid_client",
            "client must be 1..=128 characters",
        ));
    }
    Ok(())
}

fn valid_token(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value == value.trim()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

/// Stable generation for the exact project snapshot exposed to UI/CLI
/// clients.  Keep this in the protocol module so every adapter uses the same
/// scope and hashing rule instead of inventing a second generation source.
pub fn snapshot_generation(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn capabilities() -> serde_json::Value {
    let mut advertised = serde_json::json!({
        "protocol": PROTOCOL_VERSION,
        "schema_versions": [1],
        "command_versions": [1],
        "modes": ["dry_run", "apply"],
        "permissions": ["read_only", "project_write", "system_write"],
        "privileged_permissions": [{"name": "unrestricted", "requires": "explicit_cli_flag_and_trusted_client", "audit_required": true}],
        "operations": ["project.init", "project.inspect", "project.search", "control.inspect", "analyze_dynamics", "analyze_mix", "analyze_silence", "apply_dynamics_suggestion", "generate_chord", "describe_drum_lane", "inspect_chord_track", "add_chord_event", "place_generated_chord", "remove_chord_events_range", "clear_chord_track", "suggest_next_chords", "generate_arpeggio", "place_arpeggio", "preview_vocal_pitch_correction", "extension.catalog", "extension.validate", "extension.invoke", "extension.set_enabled", "project.load", "add_track", "add_aux_track", "duplicate_track", "remove_track", "add_plugin", "remove_plugin", "move_plugin", "insert_named_plugin", "insert_plugin_path", "set_plugin_parameter", "set_plugin_bypass", "set_plugin_favorite", "plugin_search", "freeze_track", "freeze_track_to_project_end", "unfreeze_track", "track_freeze_status", "set_macro_value", "add_macro_mapping", "remove_macro_mapping", "add_midi_learn_mapping", "remove_midi_learn_mapping", "humanize_midi", "apply_midi_swing", "quantize_midi", "apply_midi_logical_rule", "open_utau_import", "open_utau_notes", "open_utau_import_midi", "add_audio_region", "replace_region_audio", "set_volume", "set_master_gain", "set_track_delay", "set_track_delay_automation", "set_track_stack", "delete_track_stack", "set_track_stack_gain", "set_track_stack_collapsed", "add_vca_group", "assign_track_to_vca", "set_vca_group_gain", "upsert_marker", "delete_marker", "set_pan", "set_mute", "set_solo", "set_track_armed", "set_phase_invert", "set_automation", "set_route", "set_route_gain", "set_feedback_route", "set_sidechain_link", "set_midi_note", "clear_midi_notes", "remove_midi_notes_range", "transpose_midi_notes_range", "move_midi_notes_range", "move_region", "split_region", "duplicate_region", "remove_region", "set_region_warp", "warp_region_audio_note_segment", "remove_region_audio_note_segment", "set_region_gain", "set_region_pitch", "set_track_name", "set_time_signature", "select_recording_take", "register_comp_take", "select_comp_take", "remove_comp_take", "set_comp_segments", "split_region_with_crossfade", "transport_play", "transport_pause", "transport_stop", "set_playhead", "set_loop", "set_cycle_range", "set_metronome", "set_tempo", "record_arm", "record_start", "record_stop", "record_commit", "save_project", "bounce_project", "bounce_stems", "render_target_catalog", "undo", "redo", "project_inspect", "plugin_catalog", "history.status", "history.log", "history.diff", "history.commit", "history.branch", "history.checkout", "history.tag", "history.revert", "history.cherry_pick"],
        "history_operations_scope": "project_history_cli",
        "command_action_operations_scope": "validated_project_actions",
        "mix_assistant_operations": ["analyze_dynamics", "apply_gain_staging", "analyze_mix"],
        "standard_effect_operations": ["set_eq"],
        "dynamics_effect_operations": ["add_plugin", "Aura/Limiter", "Aura/Compressor", "Aura/Gate", "Aura/Saturation", "Aura/Transient", "Aura/DeEsser", "Aura/Delay", "Aura/Reverb", "Aura/DynamicEQ", "Aura/MidSide", "Aura/Width"],
        "comping_operations": ["select_recording_take", "register_comp_take", "select_comp_take", "remove_comp_take", "set_comp_segments"],
        "plugin_operations": ["plugin_catalog", "plugin_search", "set_plugin_favorite", "cache_invalidation_by_binary_hash"],
        "vca_operations": ["add_vca_group", "assign_track_to_vca", "set_vca_group_gain"],
        "routing_operations": ["add_track", "add_aux_track", "set_route", "set_sidechain_link"],
        "latency_operations": ["set_low_latency_mode"],
        "tonal_operations": ["set_tonal_scale"],
        "composition_operations": ["generate_chord", "suggest_next_chords", "generate_arpeggio", "inspect_chord_track", "add_chord_event", "place_generated_chord", "remove_chord_events_range", "clear_chord_track"],
        "vocal_operations": ["preview_vocal_pitch_correction", "open_utau_import", "set_midi_note_articulation"],
        "audio_edit_operations": ["analyze_silence", "split_region", "split_region_with_crossfade", "set_region_fades", "set_region_warp", "set_region_pitch"],
        "mix_analysis_operations": ["analyze_mix", "analyze_dynamics", "analyze_silence"],
        "mix_snapshot_operations": ["take_mix_snapshot", "capture_mix_snapshot", "diff_mix_snapshots", "recall_mix_snapshot", "apply_mix_snapshot"],
        "midi_editor_operations": ["inspect_midi_notes", "describe_drum_lane"],
        "midi_read_operations": ["inspect_midi_notes"],
        "render_capabilities": {
            "backend": "native_audio_engine",
            "supported_codecs": ["wav_pcm16", "wav_rf64", "wave64_float32", "aiff_pcm16", "flac", "mp3"],
            "unsupported_codecs": ["aac"],
            "external_codec_provider": "ffmpeg",
            "supported_import_formats": ["wav", "rf64", "mp3", "flac", "aif", "aiff", "m4a", "ogg", "aac"],
            "status": "available",
            "async": true,
            "cancellation": true,
            "generation_guarded_publication": true
        },
        "transport_capabilities": {
            "play": true,
            "pause": true,
            "stop": true,
            "seek": true,
            "pause_preserves_playhead": true
        },
        "history_capabilities": {
            "schema_version": 1,
            "content_hash": "sha256",
            "entity_level_diff": true,
            "field_level_diff": true,
            "large_state_fields_hashed_only": true,
            "supported_sections": ["metadata", "tracks", "regions", "plugin_instances", "midi_learn_mappings", "midi_notes", "chord_track", "macro_mappings", "warp_markers", "render_targets", "openutau_vocals", "freeze_artifacts", "sidechain_routes", "feedback_routes", "audio_routes"]
        },
        "integration_capabilities": {
            "ara2": {
                "status": "protocol_contract",
                "timeline_random_access": true,
                "plugin_protocol_bridge": true,
                "document_lifecycle": true,
                "analysis_lifecycle": true,
                "note_segment_sync": true,
                "external_sdk_required": true,
                "verified": false
            },
            "hardware_controllers": {
                "status": "protocol_core",
                "protocols": ["mcu", "hui", "eucon", "osc"],
                "device_driver_integration": false,
                "verified": false
            },
            "factory_content": {
                "status": "project_local",
                "content_delivery": false,
                "license_manifest": false,
                "verified": false
            },
            "immersive_audio": {
                "status": "engine_scaffold",
                "layouts": ["stereo", "surround", "7.1.4"],
                "dolby_renderer": false,
                "sony_360_renderer": false,
                "metadata_export": false,
                "hardware_renderer_link": false,
                "verified": false
            }
        },
        "extensions": {
            "status": "manifest_discovery_and_bounded_trusted_invocation",
            "catalog_includes": ["commands", "panels", "menus"],
            "execution_modes": ["sandboxed", "trusted_explicit_approval"],
            "permissions": ["project_read", "project_write", "audio_process", "filesystem_external", "network", "process_spawn"],
            "symlink_following": false,
            "code_execution": "trusted_only_with_explicit_process_spawn",
            "trusted_activation": "explicit_user_approval_and_audit_log"
        },
        "transport": { "input": "jsonl", "output": "jsonl", "one_request_per_line": true },
        "midi_capabilities": {
            "midi_1_cc": true,
            "midi_1_sysex": true,
            "midi_2_channel_voice": true,
            "max_sysex_payload_bytes": 4096,
            "midi_2_value_bits": 32
        },
        "clients": {
            "computer_use": {
                "supported": true,
                "interaction": "ui_or_command_api",
                "power_profile": "unrestricted_explicit",
                "requires": ["request_id", "transaction", "generation", "audit_log"]
            }
        },
        "safety": {
            "approval_required_for_apply": true,
            "unknown_operations_rejected": true,
            "computer_use_may_bypass_project_root_only_with_unrestricted": true
        }
    });
    // History is exposed by ProjectHistoryStore/CLI, not by CommandAction.
    // Keep it discoverable without claiming it can be submitted through the
    // validated project-mutation transaction envelope.
    if let Some(operations) = advertised["operations"].as_array_mut() {
        operations.retain(|operation| {
            !operation
                .as_str()
                .is_some_and(|name| name.starts_with("history."))
        });
    }
    advertised["history_operations"] = serde_json::json!([
        "history.status", "history.log", "history.diff", "history.commit",
        "history.branch", "history.checkout", "history.tag", "history.revert",
        "history.cherry_pick"
    ]);
    advertised
}
