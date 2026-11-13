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
    let legacy = r#"{"time":48000,"value":0.5,"curve":0.5}"#;
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
        (r#"{"time":1,"value":0.25,"curve":{"kind":"linear"}}"#, Curve::Shaped(Shape::Linear)),
        (r#"{"time":1,"value":0.25,"curve":{"kind":"step"}}"#, Curve::Shaped(Shape::Step)),
        (
            r#"{"time":1,"value":0.25,"curve":{"kind":"bezier","p":[{"x":0.25,"y":0.1},{"x":0.75,"y":0.9}]}}"#,
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
