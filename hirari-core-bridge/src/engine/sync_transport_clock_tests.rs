#[cfg(test)]
mod system_link_tests {
    use super::*;

    fn settings(id: u32, name: &str, offset_samples: i32) -> VstSystemLinkSettings {
        VstSystemLinkSettings {
            device_id: id,
            device_name: name.into(),
            active: true,
            online: false,
            asio_input: "ADAT In 8".into(),
            asio_output: "ADAT Out 8".into(),
            data_only: false,
            offset_samples,
            transfer_bits: SystemLinkTransferBits::Bits24,
            midi_inputs: 2,
            midi_outputs: 2,
        }
    }

    fn peer(id: u32, name: &str) -> SystemLinkPeer {
        SystemLinkPeer {
            id,
            name: name.into(),
            online: true,
            sample_rate: 48_000,
            tempo_micros_bpm: 120_000_000,
        }
    }

    fn online_runtime(id: u32, name: &str, offset_samples: i32) -> VstSystemLinkRuntime {
        let mut runtime = settings(id, name, offset_samples).runtime().unwrap();
        runtime.set_clock_locked(true);
        assert!(runtime.set_online(true));
        runtime
    }

    #[test]
    fn settings_round_trip_and_reject_invalid_limits() {
        let original = settings(1, "Studio A", -32);
        let restored = VstSystemLinkSettings::from_json(&original.to_json().unwrap()).unwrap();
        assert_eq!(restored, original);

        let mut invalid = original.clone();
        invalid.active = false;
        invalid.online = true;
        assert!(!invalid.validate());
        invalid.online = false;
        invalid.midi_outputs = 17;
        assert!(!invalid.validate());
        invalid.midi_outputs = 2;
        invalid.offset_samples = i32::MIN;
        assert!(!invalid.validate());
    }

    #[test]
    fn transport_is_peer_to_peer_sample_accurate_and_rejects_stale_packets() {
        let mut sender = online_runtime(1, "Studio A", -32);
        let mut receiver = online_runtime(2, "Studio B", 16);
        assert!(sender.upsert_peer(peer(2, "Studio B")));
        assert!(receiver.upsert_peer(peer(1, "Studio A")));

        let packet = sender
            .send_transport(SystemLinkTransport::Play, 48_000, 48_000, 120.0)
            .unwrap();
        assert_eq!(sender.sample_position, 47_968);
        assert!(receiver.receive_transport(packet));
        assert_eq!(receiver.transport, SystemLinkTransport::Play);
        assert_eq!(receiver.sample_position, 48_016);
        assert!(!receiver.receive_transport(packet));

        let mut mismatch = packet;
        mismatch.sequence += 1;
        mismatch.tempo_micros_bpm = 121_000_000;
        assert!(!receiver.receive_transport(mismatch));
    }

    #[test]
    fn midi_ports_validate_messages_and_complete_self_test() {
        let mut sender = online_runtime(1, "Studio A", -32);
        let mut receiver = online_runtime(2, "Studio B", 16);
        assert!(sender.upsert_peer(peer(2, "Studio B")));
        assert!(receiver.upsert_peer(peer(1, "Studio A")));

        let packet = sender.send_midi(1, 1_000, &[0x90, 60, 100]).unwrap();
        assert_eq!(packet.sample_position, 968);
        assert!(sender.send_midi(2, 1_000, &[0x90, 60, 100]).is_none());
        assert!(sender.send_midi(0, 1_000, &[0x90, 0x80, 100]).is_none());
        assert!(receiver.receive_midi(&packet));
        assert!(sender.self_test());
        assert!(!receiver.self_test());
        assert!(receiver.send_midi(0, 1_000, &[0x80, 60, 0]).is_some());
        assert!(receiver.self_test());

        let mut invalid_port = packet.clone();
        invalid_port.port = 2;
        assert!(!receiver.receive_midi(&invalid_port));
        let mut invalid_message = packet;
        invalid_message.bytes = vec![60, 100];
        assert!(!receiver.receive_midi(&invalid_message));
    }

    #[test]
    fn clock_loss_forces_offline_and_clears_activity() {
        let mut runtime = online_runtime(1, "Studio A", 0);
        assert!(runtime.upsert_peer(peer(2, "Studio B")));
        assert!(runtime.send_midi(0, 0, &[0x90, 60, 100]).is_some());
        assert!(runtime.self_test());

        runtime.set_clock_locked(false);
        assert!(!runtime.settings.online);
        assert!(!runtime.receiving);
        assert!(!runtime.sending);
        assert!(!runtime.self_test());
        assert!(!runtime.set_online(true));
    }
}



#[cfg(test)]
mod pro_sync_tests {
    use super::*;

    #[test]
    fn mtc_requires_stable_frames_and_inhibits_after_dropout() {
        let mut engine = TimecodeLockEngine::new(TimecodeLockPreferences {
            lock_frames: 3,
            drop_out_frames: 2,
            inhibit_restart_ms: 500,
            auto_detect_frame_rate: false,
            project_fps: 30,
        })
        .unwrap();
        for frame in 0..3 {
            let state = engine
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 0,
                        frames: frame,
                        fps: 30,
                    },
                    frame as u64 * 33,
                )
                .unwrap();
            assert_eq!(
                state,
                if frame < 2 {
                    SyncLockState::Acquiring
                } else {
                    SyncLockState::Locked
                }
            );
        }
        assert_eq!(
            engine.report_missing_frames(3, 100),
            SyncLockState::DroppedOut
        );
        assert_eq!(
            engine
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 1,
                        frames: 0,
                        fps: 30
                    },
                    200
                )
                .unwrap(),
            SyncLockState::Inhibited
        );
        assert_eq!(
            engine
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 1,
                        frames: 1,
                        fps: 30
                    },
                    601
                )
                .unwrap(),
            SyncLockState::Acquiring
        );
        assert!(engine.audit());
    }

    #[test]
    fn frame_rate_mismatch_fails_or_auto_detects() {
        let mut strict = TimecodeLockEngine::new(TimecodeLockPreferences::default()).unwrap();
        assert!(strict
            .ingest(
                Timecode {
                    hours: 0,
                    minutes: 0,
                    seconds: 0,
                    frames: 0,
                    fps: 25
                },
                0
            )
            .is_err());
        let mut automatic = TimecodeLockEngine::new(TimecodeLockPreferences {
            auto_detect_frame_rate: true,
            ..TimecodeLockPreferences::default()
        })
        .unwrap();
        assert_eq!(
            automatic
                .ingest(
                    Timecode {
                        hours: 0,
                        minutes: 0,
                        seconds: 0,
                        frames: 0,
                        fps: 25
                    },
                    0
                )
                .unwrap(),
            SyncLockState::Acquiring
        );
        assert_eq!(automatic.detected_fps, Some(25));
    }

    #[test]
    fn midi_clock_master_emits_spp_continue_and_stop_mode_clock() {
        let mut master = MidiClockMaster {
            preferences: MidiClockPreferences {
                follows_project_position: true,
                always_send_start: false,
                send_clock_in_stop: true,
            },
            destinations: vec![MidiClockDestination {
                port_id: 7,
                enabled: true,
            }],
            running: false,
            beat: 0.0,
        };
        assert_eq!(
            master.transport_start(4.0).unwrap(),
            vec![vec![0xf2, 16, 0], vec![0xfb]]
        );
        assert_eq!(master.transport_stop(), vec![vec![0xfc]]);
        assert_eq!(master.clock_pulse(), Some(vec![0xf8]));
        assert_eq!(master.active_ports(), vec![7]);
        assert!(master.audit());
    }

    #[test]
    fn midi_clock_follower_smooths_tempo_and_times_out() {
        let mut follower = MidiClockFollower::default();
        follower.start();
        for pulse in 0..24 {
            assert!(follower.pulse(pulse * 20_833));
        }
        assert!(follower.locked);
        assert!((follower.bpm.unwrap() - 120.0).abs() < 0.1);
        assert!(follower.set_song_position_pointer(8, 0));
        assert_eq!(follower.beat, 2.0);
        assert!(follower.poll_timeout(3_000_000));
        assert!(!follower.running);
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn sync_settings_round_trip_recreates_clean_runtime_engines() {
        let settings = SyncProjectSettings {
            source: SyncSource::MidiPort(7),
            protocol: SyncProtocol::MidiClock,
            enabled: true,
            timecode: TimecodeLockPreferences::default(),
            midi_clock: MidiClockPreferences {
                follows_project_position: true,
                always_send_start: false,
                send_clock_in_stop: true,
            },
            midi_destinations: vec![MidiClockDestination {
                port_id: 7,
                enabled: true,
            }],
            mmc_device_id: 0x7f,
        };
        let json = settings.to_json().unwrap();
        let restored = SyncProjectSettings::from_json(&json).unwrap();
        assert_eq!(restored, settings);
        let master = restored.create_midi_clock_master().unwrap();
        assert!(!master.running);
        assert_eq!(master.beat, 0.0);
        assert_eq!(master.active_ports(), vec![7]);
    }

    #[test]
    fn sync_settings_reject_duplicate_ports_and_protocol_mismatch() {
        let mut settings = SyncProjectSettings {
            source: SyncSource::MidiPort(1),
            protocol: SyncProtocol::Mtc,
            enabled: true,
            timecode: TimecodeLockPreferences::default(),
            midi_clock: MidiClockPreferences {
                follows_project_position: false,
                always_send_start: false,
                send_clock_in_stop: false,
            },
            midi_destinations: vec![],
            mmc_device_id: 0,
        };
        assert!(settings.create_timecode_lock().is_ok());
        settings.midi_destinations = vec![
            MidiClockDestination {
                port_id: 1,
                enabled: true,
            },
            MidiClockDestination {
                port_id: 1,
                enabled: false,
            },
        ];
        assert!(settings.to_json().is_err());
    }
}
