//! Tests split out of track_midi_articulation.rs.

    use super::*;

    fn set_output(state: &ArticulationRuntime, id: u8, slot: usize, values: [u32; 5]) {
        for (atomic, value) in state.outputs[id as usize][slot].iter().zip(values) {
            atomic.store(value, Ordering::Relaxed);
        }
    }

    fn finish(state: &ArticulationRuntime, id: u8, count: u8) {
        state.output_counts[id as usize].store(count, Ordering::Release);
    }

    #[test]
    fn note_on_emits_keyswitch_program_and_note_with_packed_mapping_order() {
        let state = ArticulationRuntime::new();
        set_output(&state, 7, 0, [1, 2, 60, 100, 0]);
        set_output(&state, 7, 1, [2, 1, 12, 0, 32]);
        finish(&state, 7, 2);
        let mut runtime = RealtimeState {
            active_articulation_by_group: [0; GROUPS],
            pending_note_offs: [PendingNoteOff::default(); PENDING_NOTE_OFFS],
            pending_note_off_count: 0,
            scheduled_note_counts: [[0; 128]; 16],
        };
        let mut events = [GeneratedMidiEvent::default(); 8];
        let count = generate_note_on(&state, &mut runtime, 3, 64, 99, 12, 7, &mut events);
        assert_eq!(count, 5);
        assert_eq!(events[0].data, [0x92, 60, 100]);
        assert_eq!(events[1].data, [0xb1, 0, 0]);
        assert_eq!(events[2].data, [0xb1, 32, 32]);
        assert_eq!(events[3].data, [0xc1, 12, 0]);
        assert_eq!(events[4].data, [0x92, 64, 99]);
        assert_eq!(runtime.scheduled_note_counts[2][64], 1);
    }

    #[test]
    fn same_group_articulation_suppresses_repeated_keyswitch_and_transition_releases_previous() {
        let state = ArticulationRuntime::new();
        set_output(&state, 1, 0, [11, 4, 45, 0, 240]);
        set_output(&state, 1, 1, [16, 0, 2, 0, 0]);
        set_output(&state, 2, 0, [1, 3, 70, 100, 0]);
        set_output(&state, 2, 1, [16, 0, 2, 0, 0]);
        finish(&state, 1, 2);
        finish(&state, 2, 2);
        let mut runtime = RealtimeState {
            active_articulation_by_group: [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            pending_note_offs: [PendingNoteOff::default(); PENDING_NOTE_OFFS],
            pending_note_off_count: 0,
            scheduled_note_counts: [[0; 128]; 16],
        };
        let mut events = [GeneratedMidiEvent::default(); 8];
        let count = generate_note_on(&state, &mut runtime, 1, 60, 100, 4, 2, &mut events);
        assert_eq!(count, 3);
        assert_eq!(events[0].data, [0x94, 45, 0]);
        assert_eq!(events[1].data, [0x93, 70, 100]);
        assert_eq!(events[2].data, [0x90, 60, 100]);
        assert_eq!(runtime.active_articulation_by_group[2], 2);
        let repeated = generate_note_on(&state, &mut runtime, 1, 61, 90, 5, 2, &mut events);
        assert_eq!(repeated, 1);
        assert_eq!(events[0].data, [0x90, 61, 90]);
    }

    #[test]
    fn scheduled_note_offs_flush_sample_accurately_and_all_notes_off_clears_counts() {
        let state = ArticulationRuntime::new();
        let runtime = unsafe { &mut *state.realtime.get() };
        runtime.pending_note_offs[0] = PendingNoteOff {
            channel: 0,
            pitch: 60,
            sample: 105,
        };
        runtime.pending_note_offs[1] = PendingNoteOff {
            channel: 1,
            pitch: 62,
            sample: 200,
        };
        runtime.pending_note_off_count = 2;
        runtime.scheduled_note_counts[0][60] = 2;
        let mut events = [GeneratedMidiEvent::default(); 16];
        let flushed = unsafe {
            hirari_track_midi_flush_scheduled_note_offs(
                (&state as *const ArticulationRuntime).cast(),
                100,
                16,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        assert_eq!(flushed, 1);
        assert_eq!(events[0].sample_offset, 5);
        assert_eq!(runtime.pending_note_off_count, 1);
        let all_off = unsafe {
            hirari_track_midi_all_notes_off(
                (&state as *const ArticulationRuntime).cast(),
                20,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        assert_eq!(all_off, 16);
        assert_eq!(events[0].data, [0xb0, 123, 0]);
        assert_eq!(events[15].data, [0xbf, 123, 0]);
        assert_eq!(runtime.pending_note_off_count, 0);
        assert_eq!(runtime.scheduled_note_counts[0][60], 0);
    }

    #[test]
    fn all_notes_off_resets_runtime_even_when_the_midi_buffer_is_full() {
        let state = ArticulationRuntime::new();
        let runtime = unsafe { &mut *state.realtime.get() };
        runtime.pending_note_offs[0] = PendingNoteOff {
            channel: 0,
            pitch: 60,
            sample: 105,
        };
        runtime.pending_note_off_count = 1;
        runtime.active_articulation_by_group[1] = 5;
        runtime.scheduled_note_counts[0][60] = 3;
        let count = unsafe {
            hirari_track_midi_all_notes_off(
                (&state as *const ArticulationRuntime).cast(),
                10,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(count, 0);
        assert_eq!(runtime.pending_note_off_count, 0);
        assert_eq!(runtime.active_articulation_by_group[1], 0);
        assert_eq!(runtime.scheduled_note_counts[0][60], 0);
    }

    #[test]
    fn ffi_publishes_expression_map_and_returns_sample_ready_midi_events() {
        let state = hirari_track_midi_articulation_create();
        assert!(!state.is_null());
        unsafe {
            assert!(hirari_track_midi_articulation_begin(state, 3));
            assert!(hirari_track_midi_articulation_set_output(
                state, 3, 0, 1, 2, 40, 100, 120,
            ));
            assert!(hirari_track_midi_articulation_finish(state, 3, 1));

            let required = hirari_track_midi_articulation_pack(state, std::ptr::null_mut(), 0);
            assert_eq!(required, 6);
            let mut packed = [0; 6];
            assert_eq!(
                hirari_track_midi_articulation_pack(state, packed.as_mut_ptr(), 6),
                6
            );
            assert_eq!(packed, [3, 2, 1, 40, 100, 120]);

            let mut timed = [TimedKeySwitchOutput::default(); 16];
            let timed_count =
                hirari_track_midi_timed_outputs(state, 3, false, timed.as_mut_ptr(), timed.len());
            assert_eq!(timed_count, 1);
            assert_eq!(timed[0].channel, 2);
            assert_eq!(timed[0].pitch, 40);
            assert_eq!(timed[0].length_ticks, 120);

            let mut generated = [GeneratedMidiEvent::default(); 128];
            let count = hirari_track_midi_note_on(
                state,
                1,
                60,
                90,
                7,
                3,
                generated.as_mut_ptr(),
                generated.len(),
            );
            assert_eq!(count, 2);
            assert_eq!(generated[0].sample_offset, 7);
            assert_eq!(generated[0].articulation_id, 3);
            assert_eq!(generated[0].data, [0x92, 40, 100]);
            assert_eq!(generated[1].data, [0x90, 60, 90]);
            hirari_track_midi_articulation_destroy(state);
        }
    }

    #[test]
    fn expression_map_take_hands_the_bytes_over_once() {
        let state = ArticulationRuntime::new();
        let raw = &state as *const ArticulationRuntime as *const c_void;
        let json = b"{\"a\":1}";
        assert!(unsafe {
            hirari_track_midi_expression_map_set(raw, 1, json.as_ptr(), json.len())
        });

        let mut size = 0usize;
        let taken = unsafe { hirari_track_midi_expression_map_take(raw, 1, &mut size) };
        assert!(!taken.is_null());
        assert_eq!(unsafe { std::slice::from_raw_parts(taken, size) }, json);
        unsafe { hirari_track_midi_expression_map_take_free(taken, size) };

        let mut again = 4usize;
        assert!(
            unsafe { hirari_track_midi_expression_map_take(std::ptr::null(), 1, &mut again) }
                .is_null()
        );
        assert_eq!(again, 0);
    }
