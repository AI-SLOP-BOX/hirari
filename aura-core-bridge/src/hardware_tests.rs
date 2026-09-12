    use super::*;

    #[test]
    fn learned_controls_support_feedback_lookup() {
        let mut map = LearnedControlMap::default();
        assert!(map.learn(LearnedControl { controller_id: 1, control_id: 7, target: "track/volume".into() }));
        assert_eq!(map.target_for(1, 7), Some("track/volume"));
        assert_eq!(map.controls_for_target("track/volume"), vec![(1, 7)]);
        assert!(map.validate());
    }
    #[test]
    fn parses_remote_transport_commands() { assert_eq!(parse_remote_transport(" REC "), Some(RemoteTransport::Record)); assert_eq!(parse_remote_transport("home"), Some(RemoteTransport::LocateStart)); assert_eq!(parse_remote_transport("bad"), None); }

    #[test]
    fn dispatches_and_normalizes_supported_controller_events() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(7, ControllerProtocol::MCU);
        let event = hardware.dispatch_event(ControllerEvent {
            controller_id: 7,
            control_id: 3,
            value: 2.0,
        });
        assert_eq!(event.unwrap().value, 1.0);
        assert!(hardware.audit_hardware());
    }

    #[test]
    fn rejects_unknown_and_non_finite_events() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(7, ControllerProtocol::OSC);
        assert!(hardware
            .dispatch_event(ControllerEvent {
                controller_id: 8,
                control_id: 0,
                value: 0.5,
            })
            .is_none());
        assert!(hardware
            .dispatch_event(ControllerEvent {
                controller_id: 7,
                control_id: 0,
                value: f32::NAN,
            })
            .is_none());
    }

    #[test]
    fn osc_messages_are_bounded_and_normalized() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(7, ControllerProtocol::OSC);
        let event = hardware.parse_osc_message(7, "/aura/track/42 1.5").unwrap();
        assert_eq!(event.control_id, 42);
        assert_eq!(event.value, 1.0);
        assert!(hardware.parse_osc_message(7, "/aura/track/not-a-number 0.5").is_none());
        assert!(hardware.parse_osc_message(7, "/other/track/42 0.5").is_none());
        assert_eq!(hardware.parse_osc_message(7, "/aura/track/42 true").unwrap().value, 1.0);
        assert_eq!(hardware.parse_osc_message(7, "/aura/track/42 false").unwrap().value, 0.0);
    }

    #[test]
    fn eucon_frames_are_fixed_width_and_normalized() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(7, ControllerProtocol::EuCon);
        let mut frame = Vec::new();
        frame.extend_from_slice(&42u32.to_le_bytes());
        frame.extend_from_slice(&1.5f32.to_le_bytes());
        let event = hardware.parse_eucon_frame(7, &frame).unwrap();
        assert_eq!(event.control_id, 42);
        assert_eq!(event.value, 1.0);
        assert!(hardware.parse_eucon_frame(7, &frame[..7]).is_none());
        let feedback = hardware.encode_eucon_frame(&ControllerEvent { controller_id: 7, control_id: 42, value: 0.25 }).unwrap();
        let decoded = hardware.parse_eucon_frame(7, &feedback).unwrap();
        assert!((decoded.value - 0.25).abs() < f32::EPSILON);
        assert!(hardware.encode_eucon_frame(&ControllerEvent { controller_id: 8, control_id: 1, value: 0.5 }).is_none());
    }

    #[test]
    fn mcu_and_hui_surface_frames_are_bounded_and_normalized() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(1, ControllerProtocol::MCU);
        hardware.controllers.insert(2, ControllerProtocol::HUI);
        let frame = [7, 0, 0xff, 0x3f];
        assert_eq!(hardware.parse_surface_frame(1, &frame).unwrap().control_id, 7);
        assert_eq!(hardware.parse_surface_frame(2, &frame).unwrap().value, 1.0);
        assert!(hardware.parse_surface_frame(1, &[0, 0, 0]).is_none());
    }

    #[test]
    fn osc_feedback_is_bounded_and_normalized() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(7, ControllerProtocol::OSC);
        let event = ControllerEvent { controller_id: 7, control_id: 42, value: 2.0 };
        assert_eq!(hardware.encode_osc_message(&event, "track"), Some("/aura/track/42 1.000000".into()));
        assert!(hardware.encode_osc_message(&event, "bad/path").is_none());
    }

    #[test]
    fn mcu_and_hui_feedback_frames_round_trip() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(1, ControllerProtocol::MCU);
        hardware.controllers.insert(2, ControllerProtocol::HUI);
        for id in [1, 2] {
            let event = ControllerEvent { controller_id: id, control_id: 9, value: 0.5 };
            let frame = hardware.encode_surface_frame(&event).unwrap();
            let decoded = hardware.parse_surface_frame(id, &frame).unwrap();
            assert!((decoded.value - 0.5).abs() < 0.001);
        }
    }

    #[test]
    fn identifies_ssl_and_avid_controller_families() {
        assert_eq!(controller_family("SSL UF8"), Some(ControllerFamily::Ssl));
        assert_eq!(controller_family("Avid Artist Mix"), Some(ControllerFamily::Avid));
        assert_eq!(controller_family("Mackie Control"), Some(ControllerFamily::Generic));
        assert!(controller_family("").is_none());
    }

    #[test]
    fn vendor_feedback_is_isolated_by_controller_protocol() {
        let mut hardware = HardwareOrchestrator::new();
        hardware.controllers.insert(10, ControllerProtocol::Ssl);
        hardware.controllers.insert(11, ControllerProtocol::Avid);
        let event = ControllerEvent { controller_id: 10, control_id: 3, value: 1.5 };
        let frame = hardware.encode_vendor_event(&event).unwrap();
        let decoded = u32::from_le_bytes(frame[0..4].try_into().unwrap());
        let value = f32::from_le_bytes(frame[4..8].try_into().unwrap());
        assert_eq!(decoded, 3);
        assert_eq!(value, 1.0);
        assert!(hardware.encode_vendor_event(&ControllerEvent { controller_id: 12, control_id: 3, value: 0.5 }).is_none());
    }
