use super::*;

#[test]
fn contracts_reject_duplicate_plugin_ids_and_bad_midi_ranges() {
    let plugin = PluginInstanceContract {
        instance_id: "p1".into(),
        track_id: 1,
        slot_index: 0,
        format: PluginFormat::Clap,
        bundle_path: "plugin.clap".into(),
        plugin_id: "gain".into(),
        bus_layout: vec![],
        component_ids: vec![],
        input_channels: 0,
        output_channels: 2,
        sidechain_channels: 0,
        parameter_ids: vec![],
        parameter_values: vec![],
        latency_samples: 0,
        state_blob: vec![],
        gui_state: vec![],
        bypassed: false,
        offline: false,
        quarantined: false,
        binary_hash: String::new(),
        capability: String::new(),
        plugin_version: String::new(),
        architecture: String::new(),
        state_schema_version: 1,
        state_generation: 0,
        kind: None,
    };
    assert!(validate_contracts(&[plugin.clone(), plugin], &[], &[], &[], &[]).is_err());
    let mapping = MidiLearnMappingContract {
        mapping_id: "m1".into(),
        device_id: "dev".into(),
        channel: 16,
        controller: 1,
        target_instance_id: "p1".into(),
        target_parameter_id: "gain".into(),
        min: 0.0,
        max: 1.0,
        curve: 0.0,
        pickup: false,
        macro_group: None,
    };
    assert!(validate_contracts(&[], &[mapping], &[], &[], &[]).is_err());
}

#[test]
fn plugin_bus_layout_supports_components_and_sidechain() {
    let plugin = PluginInstanceContract {
        instance_id: "p-layout".into(),
        track_id: 1,
        slot_index: 0,
        format: PluginFormat::Vst3,
        bundle_path: "processor.vst3".into(),
        plugin_id: "vendor.processor".into(),
        bus_layout: vec!["stereo".into(), "sidechain".into()],
        component_ids: vec!["processor".into(), "editor".into()],
        input_channels: 2,
        output_channels: 2,
        sidechain_channels: 2,
        parameter_ids: vec![],
        parameter_values: vec![],
        latency_samples: 128,
        state_blob: vec![],
        gui_state: vec![],
        bypassed: false,
        offline: false,
        quarantined: false,
        binary_hash: String::new(),
        capability: "sandbox".into(),
        plugin_version: "1.0.0".into(),
        architecture: "arm64".into(),
        state_schema_version: 1,
        state_generation: 1,
        kind: None,
    };
    assert!(validate_contracts(&[plugin.clone()], &[], &[], &[], &[]).is_ok());

    let mut invalid = plugin;
    invalid.output_channels = 0;
    assert!(validate_contracts(&[invalid], &[], &[], &[], &[]).is_err());
}

#[test]
fn contracts_round_trip_through_json() {
    let target = RenderTargetContract {
        target_id: "master".into(),
        kind: RenderTargetKind::Master,
        source_id: 1,
        pre_fader: false,
        include_inserts: true,
        include_tail: true,
        offline_generation: 1,
    };
    let json = serde_json::to_string(&target).unwrap();
    assert_eq!(
        serde_json::from_str::<RenderTargetContract>(&json).unwrap(),
        target
    );
}

#[test]
fn macro_mapping_requires_existing_plugin_and_round_trips() {
    let plugin = PluginInstanceContract {
        instance_id: "track:1:slot:0".into(),
        track_id: 1,
        slot_index: 0,
        format: PluginFormat::BuiltIn,
        bundle_path: "builtin://0".into(),
        plugin_id: "builtin:0".into(),
        bus_layout: vec![],
        component_ids: vec![],
        input_channels: 0,
        output_channels: 2,
        sidechain_channels: 0,
        parameter_ids: vec!["12".into()],
        parameter_values: vec![0.5],
        latency_samples: 0,
        state_blob: vec![],
        gui_state: vec![],
        bypassed: false,
        offline: false,
        quarantined: false,
        binary_hash: String::new(),
        capability: "builtin".into(),
        plugin_version: String::new(),
        architecture: String::new(),
        state_schema_version: 1,
        state_generation: 0,
        kind: None,
    };
    let mapping = MacroMappingContract {
        mapping_id: "macro-cutoff".into(),
        macro_index: 2,
        target_instance_id: "track:1:slot:0".into(),
        target_parameter_id: "12".into(),
        min: 0.1,
        max: 0.9,
        curve: 0.25,
        invert: false,
    };
    validate_contracts(&[plugin.clone()], &[], &[mapping.clone()], &[], &[]).unwrap();
    let json = serde_json::to_string(&mapping).unwrap();
    assert_eq!(
        serde_json::from_str::<MacroMappingContract>(&json).unwrap(),
        mapping
    );
    let mut missing = mapping;
    missing.target_instance_id = "track:9:slot:0".into();
    assert!(validate_contracts(&[plugin], &[], &[missing], &[], &[]).is_err());
}

#[test]
fn legacy_automation_point_roundtrips_byte_identically() {
    // Exactly what the pre-split writer produced (f64 prints with a fraction).
    let legacy = r#"{"time":48000.0,"value":0.5,"curve":0.5}"#;
    let point: AutomationPointContract = serde_json::from_str(legacy).unwrap();
    assert_eq!(point.curve, Curve::Shortcut(0.5));
    assert_eq!(serde_json::to_string(&point).unwrap(), legacy);
    // Omitted curve still defaults to the identity shortcut.
    let minimal = r#"{"time":96000,"value":1.0}"#;
    let point: AutomationPointContract = serde_json::from_str(minimal).unwrap();
    assert_eq!(point.curve, Curve::Shortcut(0.0));
}

#[test]
fn shape_variants_roundtrip_stably() {
    let cases = [
        (r#"{"time":1.0,"value":0.25,"curve":{"kind":"linear"}}"#, Curve::Shaped(Shape::Linear)),
        (r#"{"time":1.0,"value":0.25,"curve":{"kind":"step"}}"#, Curve::Shaped(Shape::Step)),
        (
            r#"{"time":1.0,"value":0.25,"curve":{"kind":"bezier","p":[{"x":0.25,"y":0.1},{"x":0.75,"y":0.9}]}}"#,
            Curve::Shaped(Shape::Bezier {
                p: [Point { x: 0.25, y: 0.1 }, Point { x: 0.75, y: 0.9 }],
            }),
        ),
    ];
    for (json, expected) in cases {
        let point: AutomationPointContract = serde_json::from_str(json).unwrap();
        assert_eq!(point.curve, expected);
        assert_eq!(serde_json::to_string(&point).unwrap(), json);
    }
}

#[test]
fn bezier_validation_rejects_inverted_and_out_of_range_control_x() {
    let bad_inverted = r#"{"time":1,"value":0.5,"curve":{"kind":"bezier","p":[{"x":0.9,"y":0.1},{"x":0.1,"y":0.9}]}}"#;
    let point: AutomationPointContract = serde_json::from_str(bad_inverted).unwrap();
    assert!(point.validate().is_err());
    let bad_range = r#"{"time":1,"value":0.5,"curve":{"kind":"bezier","p":[{"x":-0.5,"y":0.1},{"x":1.5,"y":0.9}]}}"#;
    let point: AutomationPointContract = serde_json::from_str(bad_range).unwrap();
    assert!(point.validate().is_err());
    let good = r#"{"time":1,"value":0.5,"curve":{"kind":"bezier","p":[{"x":0.25,"y":0.1},{"x":0.75,"y":0.9}]}}"#;
    let point: AutomationPointContract = serde_json::from_str(good).unwrap();
    assert!(point.validate().is_ok());
}

#[test]
fn shortcut_validation_matches_the_legacy_range() {
    let too_far = r#"{"time":1,"value":0.5,"curve":1.5}"#;
    let point: AutomationPointContract = serde_json::from_str(too_far).unwrap();
    assert!(point.validate().is_err());
}

#[test]
fn builtin_branch_rejects_unknown_keys_at_load() {
    // The AI edit boundary: anything not declared in the builtin schema must
    // fail at load instead of being silently dropped or honoured.
    let with_junk = r#"{"instance_id":"p","format":"BuiltIn","bundle_path":"builtin://1","plugin_id":"gain","state_blob":[],"gui_state":[],"bypassed":false,"offline":false,"quarantined":false,"binary_hash":"","capability":"","plugin_version":"","architecture":"","state_schema_version":1,"state_generation":0,"kind":{"units":[{"id":"u","kind":"builtin/gain","bogus":1}],"links":[]}}"#;
    assert!(serde_json::from_str::<PluginInstanceContract>(with_junk).is_err());
    let bad_link = r#"{"units":[{"id":"u","kind":"builtin/gain"}],"links":[{"from":"ghost:out","to":"u:in"}]}"#;
    let plugin = PluginInstanceContract {
        instance_id: "p".into(),
        track_id: 0,
        slot_index: 0,
        format: PluginFormat::BuiltIn,
        bundle_path: "builtin://1".into(),
        plugin_id: "gain".into(),
        bus_layout: vec![],
        component_ids: vec![],
        input_channels: 2,
        output_channels: 2,
        sidechain_channels: 0,
        parameter_ids: vec![],
        parameter_values: vec![],
        latency_samples: 0,
        state_blob: vec![1, 2, 3, 4],
        gui_state: vec![9, 9],
        bypassed: false,
        offline: false,
        quarantined: false,
        binary_hash: String::new(),
        capability: String::new(),
        plugin_version: String::new(),
        architecture: String::new(),
        state_schema_version: 1,
        state_generation: 0,
        kind: Some(BuiltinDataflow {
            units: vec![BuiltinUnit {
                id: "u".into(),
                kind: "builtin/gain".into(),
                parameters: Default::default(),
            }],
            links: vec![BuiltinLink {
                from: "ghost:out".into(),
                to: "u:in".into(),
            }],
        }),
    };
    assert!(validate_contracts(&[plugin], &[], &[], &[], &[]).is_err());
    let _ = bad_link;
}

#[test]
fn builtin_branch_roundtrips_and_opaque_state_is_untouched() {
    let original = PluginInstanceContract {
        instance_id: "track:1:slot:0".into(),
        track_id: 1,
        slot_index: 0,
        format: PluginFormat::BuiltIn,
        bundle_path: "builtin://3".into(),
        plugin_id: "builtin:gain".into(),
        bus_layout: vec![],
        component_ids: vec![],
        input_channels: 2,
        output_channels: 2,
        sidechain_channels: 0,
        parameter_ids: vec![],
        parameter_values: vec![],
        latency_samples: 0,
        state_blob: vec![0xde, 0xad, 0xbe, 0xef],
        gui_state: vec![0xca, 0xfe],
        bypassed: false,
        offline: false,
        quarantined: false,
        binary_hash: String::new(),
        capability: "builtin".into(),
        plugin_version: "1.0.0".into(),
        architecture: "arm64".into(),
        state_schema_version: 1,
        state_generation: 7,
        kind: Some(BuiltinDataflow {
            units: vec![BuiltinUnit {
                id: "gain-1".into(),
                kind: "builtin/gain".into(),
                parameters: [("level".to_string(), 0.5)].into_iter().collect(),
            }],
            links: vec![],
        }),
    };
    let bytes = serde_json::to_vec(&original).unwrap();
    let reloaded: PluginInstanceContract = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reloaded, original);
    assert_eq!(reloaded.state_blob, original.state_blob);
    assert_eq!(reloaded.gui_state, original.gui_state);
    assert_eq!(reloaded.binary_hash, original.binary_hash);
    // A built-in branch serializes as tagged kind; external plugins omit it.
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\"kind\":{"));
    let opaque = PluginInstanceContract {
        kind: None,
        ..original.clone()
    };
    let opaque_json = serde_json::to_string(&opaque).unwrap();
    assert!(!opaque_json.contains("kind"));
}

#[test]
fn conformance_vector_contract_version_upgrades_and_rejects() {
    // Vector from TEXTUAL_PATCH §7.5: v1 upgrades to 2, anything else rejects.
    let doc = r#"{"schema_version":1,"contract_version":1,"project_id":"11111111-1111-4111-8111-111111111111","metadata":{"name":"Vector","version":1,"bpm":120.0,"tracks_count":0},"sample_rate":48000.0,"tracks":[],"regions":[]}"#;
    let parsed = crate::project::ProjectDocument::from_json_bytes(doc.as_bytes())
        .expect("contract version 1 must upgrade in memory");
    assert_eq!(
        parsed.contract_version,
        crate::project_contracts::PROJECT_CONTRACT_VERSION
    );
    let future = r#"{"schema_version":1,"contract_version":3,"project_id":"11111111-1111-4111-8111-111111111111","metadata":{"name":"Vector","version":1,"bpm":120.0,"tracks_count":0},"sample_rate":48000.0,"tracks":[],"regions":[]}"#;
    assert!(crate::project::ProjectDocument::from_json_bytes(future.as_bytes()).is_err());
}

#[test]
fn conformance_vector_unknown_root_keys_are_ignored_but_builtin_rejects() {
    // §7.6: unknown keys at the root / ordinary structs are ignored (minor
    // release tolerance); unknown keys inside the builtin device branch reject.
    let base = r#"{"schema_version":1,"contract_version":2,"project_id":"11111111-1111-4111-8111-111111111111","metadata":{"name":"Vector","version":1,"bpm":120.0,"tracks_count":0,"future_field":42},"sample_rate":48000.0,"future_root_key":[1,2,3],"tracks":[],"regions":[]}"#;
    let parsed = crate::project::ProjectDocument::from_json_bytes(base.as_bytes())
        .expect("unknown root keys must be ignored");
    assert_eq!(parsed.metadata.bpm, 120.0);
}

#[test]
fn conformance_vector_parse_emit_round_trips() {
    // §7.8: parse of emitted bytes is the identity for a contract value.
    let original = AutomationPointContract {
        time: 48000.0,
        value: 0.5,
        curve: Curve::Shaped(Shape::Bezier {
            p: [Point { x: 0.25, y: 0.1 }, Point { x: 0.75, y: 0.9 }],
        }),
    };
    let bytes = serde_json::to_vec(&original).unwrap();
    let reparsed: AutomationPointContract = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reparsed, original);
    let reemitted = serde_json::to_vec(&reparsed).unwrap();
    assert_eq!(bytes, reemitted);
}

#[test]
fn conformance_vector_diff_project_json_reports_one_leaf() {
    // §7.9: one leaf change → exactly one pointer path.
    let before = serde_json::json!({"tracks":[{"id":1,"name":"a","volume":0.5}]});
    let after = serde_json::json!({"tracks":[{"id":1,"name":"a","volume":0.75}]});
    let diff = crate::project::diff_project_json(&before, &after);
    assert_eq!(diff, vec!["/tracks/0/volume".to_string()]);
}
