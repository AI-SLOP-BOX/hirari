use aura_core_bridge::AuraCore;

mod support {
    include!("recording_support.rs");
}
use support::*;

#[test]
fn take_comping_is_core_owned_and_resolves_crossfades() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    assert!(core.register_comp_take(1, "Verse take", 0, 1_000));
    assert!(core.register_comp_take(2, "Chorus take", 0, 1_000));
    assert!(!core.register_comp_take(1, "Duplicate", 0, 1_000));

    assert!(core.set_comp_segments(&[(1, 0, 500, 32), (2, 500, 500, 32),]));
    assert_eq!(core.resolve_comp_at(100), (1, 0));
    assert_eq!(core.resolve_comp_at(10), (1, 32));
    assert_eq!(core.resolve_comp_at(510), (2, 32));
    assert_eq!(core.resolve_comp_at(1_100), (0, 0));

    let verse = vec![0.2_f32; 1_000];
    let chorus = vec![0.8_f32; 1_000];
    let rendered = core.render_comped_audio(&[(1, verse.as_slice()), (2, chorus.as_slice())]);
    assert_eq!(rendered.len(), 1_000);
    assert!((rendered[100] - 0.2).abs() < 0.001);
    assert!((rendered[900] - 0.8).abs() < 0.001);
    assert!(rendered[501] > 0.2 && rendered[501] < 0.8);

    // Overlapping segments are rejected without replacing the valid comp.
    assert!(!core.set_comp_segments(&[(1, 0, 600, 32), (2, 500, 500, 32)]));
    assert_eq!(core.resolve_comp_at(510), (2, 32));

    let project_path = std::env::temp_dir().join(format!(
        "aura-comping-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    assert!(core.save_project(project_path.to_str().unwrap()));
    let restored = AuraCore::new().expect("second core must initialize");
    assert!(restored.load_project(project_path.to_str().unwrap()));
    assert_eq!(restored.resolve_comp_at(510), (2, 32));
    let _ = std::fs::remove_file(&project_path);
    let _ = std::fs::remove_file(format!("{}.comping.json", project_path.display()));
}

#[test]
fn midi_expression_events_transform_and_survive_project_reload() {
    let _guard = native_engine_test_guard();
    let core = AuraCore::new().expect("core must initialize");
    let events = r#"[
        {"beat":0.0,"channel":0,"kind":{"ControlChange":{"controller":1,"value":64}}},
        {"beat":0.5,"channel":1,"kind":{"PitchBend":{"value":0.0}}},
        {"beat":1.0,"channel":2,"kind":{"ChannelAftertouch":{"pressure":0.5}}}
    ]"#;
    assert!(core.set_midi_events_json(events));
    assert!(core.apply_midi_swing(0.5, 0.5));
    assert!(core.humanize_midi(0.02, 8, 7));
    let transformed = core.midi_events_json();
    assert_ne!(transformed, events);

    let project_path = std::env::temp_dir().join(format!(
        "aura-midi-events-{}-{}.aura",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos()
    ));
    assert!(core.save_project(project_path.to_str().unwrap()));
    let restored = AuraCore::new().expect("second core must initialize");
    assert!(restored.load_project(project_path.to_str().unwrap()));
    assert_eq!(restored.midi_events_json(), transformed);
    let _ = std::fs::remove_file(&project_path);
    let _ = std::fs::remove_file(format!("{}.comping.json", project_path.display()));
    let _ = std::fs::remove_file(format!("{}.midi-events.json", project_path.display()));
}
