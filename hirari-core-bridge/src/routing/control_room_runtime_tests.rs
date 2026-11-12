//! Tests split out of control_room_runtime.rs.

    use super::*;

    #[test]
    fn speaker_monitor_and_talkback_state_publish_to_audio_path() {
        let state = hirari_control_room_state_create();
        let room = b"Booth A";
        unsafe {
            assert_eq!(
                hirari_control_room_speaker_snapshot(state, std::ptr::null_mut(), 0),
                1
            );
            assert!(hirari_control_room_add_speaker(
                state,
                room.as_ptr(),
                room.len(),
                1.5
            ));
            assert!(hirari_control_room_select_speaker(state, 1));
            assert!(hirari_control_room_set_dim_db(state, -6.0));
            hirari_control_room_set_dim(state, true);
            assert!(hirari_control_room_is_dimmed(state));
            hirari_control_room_set_talkback(state, true, 2.0);
            assert!(hirari_control_room_talkback_enabled(state));
            assert!(hirari_control_room_set_talkback_channel(state, 3));
            assert_eq!(hirari_control_room_talkback_channel(state), 3);
        }
        let expected_monitor = 1.5_f32 * 10.0_f32.powf(-6.0 / 20.0);
        assert!(
            (unsafe { hirari_control_room_monitor_gain(state) } - expected_monitor).abs() < 1.0e-6
        );
        let input_l = [0.2_f32];
        let input_r = [-0.2_f32];
        let talkback = [0.1_f32];
        let mut output_l = input_l;
        let mut output_r = input_r;
        unsafe {
            hirari_control_room_process_monitor_state(
                state,
                output_l.as_mut_ptr(),
                output_r.as_mut_ptr(),
                talkback.as_ptr(),
                1,
            );
        }
        assert!((output_l[0] - (0.2 * expected_monitor + 0.2)).abs() < 1.0e-6);
        assert!((output_r[0] - (-0.2 * expected_monitor + 0.2)).abs() < 1.0e-6);

        let mut speakers = [SpeakerSnapshot {
            name: [0; 128],
            name_length: 0,
            gain: 0.0,
            enabled: 0,
        }; 2];
        unsafe {
            assert_eq!(
                hirari_control_room_speaker_snapshot(state, speakers.as_mut_ptr(), speakers.len()),
                2
            );
        }
        assert_eq!(&speakers[1].name[..speakers[1].name_length as usize], room);
        unsafe { hirari_control_room_state_destroy(state) };
    }

    #[test]
    fn cue_state_validation_and_project_reset_match_contract() {
        let state = hirari_control_room_state_create();
        unsafe {
            assert!(hirari_control_room_upsert_cue(state, 7, 0.75, true));
            assert!(hirari_control_room_set_cue_bus(state, 7, 42));
            assert!(hirari_control_room_set_cue_output(state, 7, 2));
            assert!(hirari_control_room_set_cue_click(state, 7, true));
            assert!(hirari_control_room_select_cue(state, 7));
            assert!(hirari_control_room_validate(state));
            assert_eq!(hirari_control_room_active_cue_id(state), 7);
            assert_eq!(hirari_control_room_active_cue_bus(state), 42);
            assert_eq!(hirari_control_room_active_cue_output(state), 2);
            assert!(hirari_control_room_active_cue_click(state));
            assert!(hirari_control_room_is_cue_bus(state, 42));
            assert!(!hirari_control_room_upsert_cue(state, 8, f32::NAN, true));
            assert!(hirari_control_room_upsert_cue(state, 8, 1.0, true));
            assert!(!hirari_control_room_set_cue_bus(state, 8, 42));
            assert!(hirari_control_room_remove_cue(state, 7));
            assert_eq!(hirari_control_room_active_cue_id(state), 0);
            hirari_control_room_reset(state);
            assert!(hirari_control_room_validate(state));
            assert_eq!(hirari_control_room_active_cue_id(state), 0);
            assert_eq!(hirari_control_room_monitor_gain(state), 1.0);
            hirari_control_room_state_destroy(state);
        }
    }
