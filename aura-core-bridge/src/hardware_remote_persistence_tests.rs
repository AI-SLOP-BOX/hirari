    use super::*;

    #[test]
    fn midi_remote_round_trip_excludes_transient_pickup_state() {
        let mut engine = MidiRemoteEngine::new(55, 8).unwrap();
        assert!(engine.upsert_page(MappingPage { id: 1, name: "Mix".into(), factory: false,
            scope: MappingScope::Project, mappings: vec![RemoteMapping { control_id: 1,
                target: "track/{track}/volume".into(), value_mode: RemoteValueMode::Pickup,
                input_mode: RemoteInputMode::Absolute, focus_mode: RemoteFocusMode::TrackSelection,
                minimum: 0.0, maximum: 1.0, inverted: false, transmit_feedback: true, bank_slot: None }] }));
        assert!(engine.set_selected_track(Some(3)));
        assert!(engine.set_target_value("track/3/volume", 0.8));
        let _ = engine.process(ControllerEvent { controller_id: 55, control_id: 1, value: 0.1 });
        assert!(!engine.takeover.is_empty());
        let json = engine.to_json().unwrap();
        let restored = MidiRemoteEngine::from_json(&json).unwrap();
        assert!(restored.takeover.is_empty());
        assert_eq!(restored.pages, engine.pages);
        assert_eq!(restored.target_values, engine.target_values);
    }

    #[test]
    fn legacy_focus_lock_reconstructs_selected_track_target() {
        let mut engine = MidiRemoteEngine::new(55, 8).unwrap();
        assert!(engine.upsert_page(MappingPage { id: 1, name: "Quick".into(), factory: false,
            scope: MappingScope::Global, mappings: vec![RemoteMapping { control_id: 1,
                target: "{focus}/quick-control/1".into(), value_mode: RemoteValueMode::Jump,
                input_mode: RemoteInputMode::Absolute, focus_mode: RemoteFocusMode::FocusQuickControl,
                minimum: 0.0, maximum: 1.0, inverted: false, transmit_feedback: true, bank_slot: None }] }));
        assert!(engine.set_selected_track(Some(8)));
        assert!(engine.set_quick_control_lock(true));
        let mut json: serde_json::Value = serde_json::from_str(&engine.to_json().unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("active_plugin");
        json.as_object_mut().unwrap().remove("quick_control_policy");
        json.as_object_mut().unwrap().remove("focused_window");
        json.as_object_mut().unwrap().remove("locked_quick_control");

        let restored = MidiRemoteEngine::from_json(&json.to_string()).unwrap();
        assert_eq!(restored.locked_quick_control, Some(QuickControlTarget::Track(8)));
        assert!(restored.focus_locked);
        assert!(restored.audit());
    }
