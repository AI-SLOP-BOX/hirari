#[cfg(test)]
mod console_tests {
    use super::*;

    #[test]
    fn talkback_dims_cues_and_auto_disables_for_recording() {
        let mut room = ControlRoomConsole::default();
        assert!(room.upsert_cue(CueMix {
            id: 1,
            name: "Artist".into(),
            gain_db: -3.0,
            talkback_send_db: -6.0,
            dim_during_talkback: true,
            talkback_enabled: true,
            source: CueSource::CueSends,
            click_enabled: true,
            click_level_db: -12.0,
            click_pan: 0.0,
            enabled: true
        }));
        room.auto_disable_talkback = AutoDisableTalkback::Recording;
        assert!(room.set_talkback(true, false));
        assert_eq!(room.effective_cue_level_db(1), Some(-23.0));
        assert_eq!(room.effective_talkback_send_db(1), Some(-6.0));
        room.set_transport(TransportActivity::Recording);
        assert!(!room.talkback);
        assert!(!room.set_talkback(true, false));
        assert!(room.audit());
    }

    #[test]
    fn cue_channels_follow_cubase_four_channel_limit_and_validate_sources() {
        let mut room = ControlRoomConsole::default();
        room.sources.push(MonitorSource {
            id: 9,
            name: "Live Room".into(),
            channels: 2,
        });
        for id in 1..=4 {
            assert!(room.upsert_cue(CueMix {
                id,
                name: format!("Cue {id}"),
                gain_db: 0.0,
                talkback_send_db: -6.0,
                dim_during_talkback: true,
                talkback_enabled: true,
                source: if id == 1 {
                    CueSource::External(9)
                } else {
                    CueSource::CueSends
                },
                click_enabled: id == 1,
                click_level_db: -6.0,
                click_pan: 0.0,
                enabled: true,
            }));
        }
        let mut fifth = room.cues[0].clone();
        fifth.id = 5;
        fifth.name = "Cue 5".into();
        assert!(!room.upsert_cue(fifth));

        room.cues[0].source = CueSource::External(99);
        assert!(!room.audit());
    }

    #[test]
    fn cue_click_uses_equal_power_pan_and_talkback_can_be_disabled_per_cue() {
        let mut room = ControlRoomConsole::default();
        assert!(room.upsert_cue(CueMix {
            id: 1,
            name: "Singer".into(),
            gain_db: 0.0,
            talkback_send_db: -3.0,
            dim_during_talkback: true,
            talkback_enabled: false,
            source: CueSource::Mix,
            click_enabled: true,
            click_level_db: 0.0,
            click_pan: 0.0,
            enabled: true,
        }));
        let gains = room.cue_click_gains(1).unwrap();
        assert!((gains[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.0001);
        assert!((gains[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.0001);
        assert!(room.set_talkback(true, false));
        assert_eq!(room.effective_talkback_send_db(1), None);
        room.cues[0].talkback_enabled = true;
        assert_eq!(room.effective_talkback_send_db(1), Some(-3.0));
    }

    #[test]
    fn monitor_processing_applies_level_and_talkback_on_separate_path() {
        let mut room = ControlRoomConsole::default();
        room.control_level_db = -6.0;
        assert!(room.set_talkback(true, false));
        let (left, right) = room
            .process_monitor_stereo(
                &[1.0, 1.0],
                &[1.0, 1.0],
                Some(&[0.5, 0.5]),
                Some(&[0.5, 0.5]),
            )
            .unwrap();
        assert!(left[0] > 0.0 && left[0] < 1.0);
        assert_eq!(left, right);
        assert!(room
            .process_monitor_stereo(&[1.0], &[1.0, 1.0], None, None)
            .is_none());
    }

    #[test]
    fn reference_dim_and_listen_bus_are_independent() {
        let mut room = ControlRoomConsole::default();
        room.reference_level_active = true;
        room.reference_level_db = -18.0;
        room.dim = true;
        room.main_dim_db = -12.0;
        room.listen_dim_db = -20.0;
        room.listen_level_db = 3.0;
        assert!(room.set_listen(7, true));
        assert_eq!(room.effective_main_level_db(), -50.0);
        assert_eq!(room.effective_listen_level_db(), Some(-27.0));
        assert!(room.audit());
    }

    #[test]
    fn rejects_shared_monitor_ports_when_exclusive() {
        let mut room = ControlRoomConsole::default();
        room.monitors.push(MonitorDestination {
            id: 2,
            name: "Nearfield".into(),
            channels: 2,
            device_ports: vec!["Out 1".into(), "Out 3".into()],
        });
        assert!(!room.audit());
        room.exclusive_monitor_ports = false;
        assert!(room.audit());
    }

    #[test]
    fn applies_monitor_specific_stereo_to_mono_downmix() {
        let mut room = ControlRoomConsole::default();
        room.monitors[0].channels = 1;
        room.monitors[0].device_ports = vec!["Mono Out".into()];
        assert!(room.upsert_downmix(DownmixPreset {
            id: 7,
            name: "Mono Check".into(),
            monitor_id: 1,
            source_channels: 2,
            output: SpeakerConfiguration::Mono,
            coefficients: vec![0.5, 0.5]
        }));
        assert!(room.select_downmix(Some(7)));
        assert_eq!(
            room.render_downmix(&[1.0, -1.0, 0.5, 0.5]).unwrap(),
            vec![0.0, 0.5]
        );
        assert!(room.audit());
    }

    #[test]
    fn phones_selects_cue_without_following_main_dim() {
        let mut room = ControlRoomConsole::default();
        assert!(room.upsert_cue(CueMix {
            id: 3,
            name: "Band".into(),
            gain_db: -2.0,
            talkback_send_db: -6.0,
            dim_during_talkback: true,
            talkback_enabled: true,
            source: CueSource::CueSends,
            click_enabled: false,
            click_level_db: 0.0,
            click_pan: 0.0,
            enabled: true
        }));
        assert!(room.set_phones(Some(PhonesChannel {
            enabled: true,
            device_ports: ["HP L".into(), "HP R".into()],
            source: PhonesSource::Cue(3),
            level_db: -8.0,
            click_enabled: true,
            click_level_db: -12.0,
            click_pan: 0.0,
            listen_enabled: true,
            listen_level_db: -3.0,
            use_as_preview: true
        })));
        room.dim = true;
        assert_eq!(room.phones.as_ref().unwrap().level_db, -8.0);
        assert!(room.audit());
    }

    #[test]
    fn rejects_invalid_downmix_shape_and_missing_phones_source() {
        let mut room = ControlRoomConsole::default();
        assert!(!room.upsert_downmix(DownmixPreset {
            id: 1,
            name: "Broken".into(),
            monitor_id: 1,
            source_channels: 6,
            output: SpeakerConfiguration::Stereo,
            coefficients: vec![1.0; 3]
        }));
        assert!(!room.set_phones(Some(PhonesChannel {
            enabled: true,
            device_ports: ["HP L".into(), "HP R".into()],
            source: PhonesSource::Cue(99),
            level_db: 0.0,
            click_enabled: false,
            click_level_db: 0.0,
            click_pan: 0.0,
            listen_enabled: false,
            listen_level_db: 0.0,
            use_as_preview: false
        })));
    }

    #[test]
    fn monitor_calibration_applies_gain_and_per_speaker_phase() {
        let room = ControlRoomConsole::default();
        let mut controls = MonitorChannelControls::default();
        assert!(controls.set_calibration(
            &room,
            MonitorCalibration {
                monitor_id: 1,
                input_gain_db: 6.0206,
                phase_inverted: vec![false, true]
            }
        ));
        let output = controls.process(&room, &[0.25, 0.25]).unwrap();
        assert!((output[0] - 0.5).abs() < 0.001);
        assert!((output[1] + 0.5).abs() < 0.001);
        let json = controls.to_json(&room).unwrap();
        assert_eq!(
            MonitorChannelControls::from_json(&json, &room).unwrap(),
            controls
        );
    }

    #[test]
    fn speaker_solo_can_be_auditioned_on_stereo_center_fallback() {
        let room = ControlRoomConsole::default();
        let mut controls = MonitorChannelControls::default();
        assert!(controls.set_speaker_solo(&room, 1, true));
        controls.solo_to_center = true;
        assert_eq!(
            controls.process(&room, &[0.2, 0.8]).unwrap(),
            vec![0.4, 0.4]
        );
        controls.clear_speaker_solos();
        assert_eq!(
            controls.process(&room, &[0.2, 0.8]).unwrap(),
            vec![0.2, 0.8]
        );
    }

    #[test]
    fn surround_solo_routes_rears_to_front_for_speaker_checks() {
        let mut room = ControlRoomConsole::default();
        room.monitors[0].channels = 6;
        room.monitors[0].device_ports = (1..=6).map(|index| format!("Out {index}")).collect();
        let mut controls = MonitorChannelControls::default();
        assert!(controls.set_speaker_solo(&room, 4, true));
        controls.surround_to_front = true;
        assert_eq!(
            controls
                .process(&room, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
                .unwrap(),
            vec![0.0, 5.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert!(!controls.set_speaker_solo(&room, 6, true));
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn control_room_round_trips_and_rejects_invalid_port_conflicts() {
        let mut room = ControlRoomConsole::default();
        room.cues.push(CueMix {
            id: 1,
            name: "Artist".into(),
            gain_db: -3.0,
            talkback_send_db: -6.0,
            dim_during_talkback: true,
            talkback_enabled: true,
            source: CueSource::CueSends,
            click_enabled: true,
            click_level_db: -12.0,
            click_pan: 0.0,
            enabled: true,
        });
        let json = room.to_json().unwrap();
        assert_eq!(ControlRoomConsole::from_json(&json).unwrap(), room);
        room.monitors.push(MonitorDestination {
            id: 2,
            name: "Nearfield".into(),
            channels: 2,
            device_ports: vec!["Out 1".into(), "Out 4".into()],
        });
        assert!(room.to_json().is_err());
    }
}
