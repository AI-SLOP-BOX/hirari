    use super::*;

    fn mapping(control_id: u32, target: &str, mode: RemoteValueMode) -> RemoteMapping {
        RemoteMapping { control_id, target: target.into(), value_mode: mode, input_mode: RemoteInputMode::Absolute,
            focus_mode: RemoteFocusMode::Fixed, minimum: 0.0, maximum: 1.0, inverted: false,
            transmit_feedback: true, bank_slot: None }
    }

    fn engine(mappings: Vec<RemoteMapping>) -> MidiRemoteEngine {
        let mut engine = MidiRemoteEngine::new(1, 8).unwrap();
        assert!(engine.upsert_page(MappingPage { id: 1, name: "Mix".into(), factory: false,
            scope: MappingScope::Global, mappings })); engine
    }

    #[test]
    fn pickup_prevents_parameter_jump_until_hardware_crosses_value() {
        let mut engine = engine(vec![mapping(1, "track/volume", RemoteValueMode::Pickup)]);
        assert!(engine.set_target_value("track/volume", 0.75));
        assert!(engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 0.1 }).is_none());
        assert!(engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 0.5 }).is_none());
        let change = engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 0.8 }).unwrap();
        assert_eq!(change, RemoteParameterChange { target: "track/volume".into(), value: 0.8 });
    }

    #[test]
    fn relative_encoder_toggle_and_feedback_work() {
        let mut encoder = mapping(1, "pan", RemoteValueMode::Jump);
        encoder.input_mode = RemoteInputMode::RelativeTwosComplement;
        let mut engine = engine(vec![encoder, mapping(2, "mute", RemoteValueMode::Toggle)]);
        assert!(engine.set_target_value("pan", 0.5));
        let change = engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 1.0 / 127.0 }).unwrap();
        assert!(change.value > 0.5);
        assert_eq!(engine.process(ControllerEvent { controller_id: 1, control_id: 2, value: 1.0 }).unwrap().value, 1.0);
        assert!(engine.process(ControllerEvent { controller_id: 1, control_id: 2, value: 1.0 }).is_none());
        engine.process(ControllerEvent { controller_id: 1, control_id: 2, value: 0.0 });
        assert_eq!(engine.process(ControllerEvent { controller_id: 1, control_id: 2, value: 1.0 }).unwrap().value, 0.0);
        assert_eq!(engine.feedback_for("pan", 0.25)[0].value, 0.25);
    }

    #[test]
    fn pages_banks_and_track_focus_resolve_dynamic_targets() {
        let mut focused = mapping(1, "track/{track}/volume", RemoteValueMode::Jump);
        focused.focus_mode = RemoteFocusMode::TrackSelection;
        let mut banked = mapping(2, "track/{bank}/mute", RemoteValueMode::Jump); banked.bank_slot = Some(2);
        let mut engine = engine(vec![focused, banked]);
        assert!(engine.set_selected_track(Some(9))); engine.shift_bank(1);
        assert_eq!(engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 0.7 }).unwrap().target, "track/9/volume");
        assert_eq!(engine.process(ControllerEvent { controller_id: 1, control_id: 2, value: 1.0 }).unwrap().target, "track/10/mute");
        assert!(engine.audit());
    }

    #[test]
    fn factory_pages_cannot_be_overwritten_or_removed() {
        let mut engine = MidiRemoteEngine::new(1, 8).unwrap();
        let page = MappingPage { id: 1, name: "Factory".into(), factory: true, scope: MappingScope::Global,
            mappings: vec![mapping(1, "play", RemoteValueMode::Toggle)] };
        assert!(engine.upsert_page(page.clone()));
        assert!(!engine.upsert_page(page)); assert!(!engine.remove_page(1));
    }

    #[test]
    fn focus_quick_controls_follow_project_or_plugin_window() {
        let mut quick = mapping(1, "{focus}/quick-control/1", RemoteValueMode::Jump);
        quick.focus_mode = RemoteFocusMode::FocusQuickControl;
        let mut engine = engine(vec![quick]);
        assert!(engine.set_selected_track(Some(12)));
        assert!(engine.set_active_plugin(Some(77)));

        let event = |value| ControllerEvent { controller_id: 1, control_id: 1, value };
        assert_eq!(engine.process(event(0.25)).unwrap().target, "track/12/quick-control/1");
        engine.set_focused_window(FocusedWindow::Plugin);
        assert_eq!(engine.process(event(0.5)).unwrap().target, "plugin/77/quick-control/1");

        engine.set_quick_control_policy(QuickControlFocusPolicy::TrackOnly);
        assert_eq!(engine.process(event(0.75)).unwrap().target, "track/12/quick-control/1");
        engine.set_quick_control_policy(QuickControlFocusPolicy::PluginWindowOnly);
        assert_eq!(engine.process(event(1.0)).unwrap().target, "plugin/77/quick-control/1");
        assert!(engine.audit());
    }

    #[test]
    fn quick_control_focus_lock_survives_selection_and_window_changes() {
        let mut quick = mapping(1, "{focus}/quick-control/2", RemoteValueMode::Jump);
        quick.focus_mode = RemoteFocusMode::FocusQuickControl;
        let mut engine = engine(vec![quick]);
        assert!(engine.set_selected_track(Some(3)));
        assert!(engine.set_quick_control_lock(true));
        assert!(engine.set_selected_track(Some(4)));
        assert!(engine.set_active_plugin(Some(9)));
        engine.set_focused_window(FocusedWindow::Plugin);
        let change = engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 0.5 }).unwrap();
        assert_eq!(change.target, "track/3/quick-control/2");

        assert!(engine.set_quick_control_lock(false));
        let change = engine.process(ControllerEvent { controller_id: 1, control_id: 1, value: 0.6 }).unwrap();
        assert_eq!(change.target, "plugin/9/quick-control/2");
        assert!(engine.audit());
    }
