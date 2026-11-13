//! Tests split out of effect_chain_host.rs.
    use super::*;
    use crate::audio_buffer_ops::{
        hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_reserve_buffer,
    };

    fn chain() -> *mut c_void {
        { hirari_effect_chain_create() }
    }

    #[test]
    fn builtins_keep_order_kind_and_bypass() {
        let state = chain();
        assert!(!state.is_null());
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 1, false, false) });
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 4, false, false) });
        assert!(!unsafe { hirari_effect_chain_add_builtin(state, 99, false, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 2);
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 0) },
            1
        );
        assert!(unsafe { hirari_effect_chain_move(state, 0, 1) });
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 1) },
            1
        );
        assert!(unsafe { hirari_effect_chain_set_bypass(state, 0, true) });
        assert!(unsafe { hirari_effect_chain_bypass(state as *const _, 0) });
        assert_eq!(
            unsafe { hirari_effect_chain_midi_instrument_count(state as *const _) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_sandbox_slot_count(state as *const _) },
            0
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn the_bus_flag_only_diverges_for_the_limiter_slot() {
        let state = chain();
        // Slot 0 asks the catalog for Hirari/BusLimiter, which no DSP implements
        // yet, so a bus limiter insert is refused rather than silently inserted.
        assert!(!unsafe { hirari_effect_chain_add_builtin(state, 0, true, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 0);
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 0, false, false) });
        // Every later slot resolves to the same path for a bus and a track.
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 2, true, true) });
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 2, false, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 3);
        assert!(unsafe { hirari_effect_chain_bypass(state as *const _, 1) == false });
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn removed_node_returns_through_a_detach_token() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 2, false, false) });
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 3, false, true) });
        let mut bypassed = true;
        let mut parallel = false;
        let mut kind = 0;
        let mut token = 0;
        assert!(unsafe {
            hirari_effect_chain_remove(
                state,
                1,
                &mut bypassed,
                &mut parallel,
                &mut kind,
                &mut token,
            )
        });
        assert_eq!(kind, 3);
        assert!(parallel);
        assert!(!bypassed);
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 1);
        assert!(unsafe { hirari_effect_chain_restore(state, 1, token, true, false) });
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 2);
        assert!(unsafe { hirari_effect_chain_bypass(state as *const _, 1) });
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 1) },
            3
        );
        assert!(!unsafe { hirari_effect_chain_restore(state, 0, token + 500, false, false) });
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn parameters_and_state_round_trip_through_a_builtin_node() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 1, false, false) });
        let count = unsafe { hirari_effect_chain_parameter_count(state as *const _, 0) };
        assert!(count > 0);
        assert!(unsafe { hirari_effect_chain_set_parameter(state, 0, 0, 0.75) });
        assert_eq!(
            unsafe { hirari_effect_chain_parameter(state as *const _, 0, 0) },
            0.75
        );
        assert!(!unsafe { hirari_effect_chain_set_parameter(state, 0, 0, f32::NAN) });
        let mut length = 0;
        let bytes = unsafe { hirari_effect_chain_processor_state(state, 0, &mut length) };
        assert!(!bytes.is_null());
        assert!(length > 0);
        let saved = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
        unsafe { hirari_effect_chain_buffer_free(bytes, length) };
        assert!(unsafe { hirari_effect_chain_set_parameter(state, 0, 0, 0.1) });
        assert_eq!(
            unsafe { hirari_effect_chain_parameter(state as *const _, 0, 0) },
            0.1
        );
        assert!(unsafe {
            hirari_effect_chain_processor_state_restore(state, 0, saved.as_ptr(), saved.len())
        });
        assert_eq!(
            unsafe { hirari_effect_chain_parameter(state as *const _, 0, 0) },
            0.75
        );
        unsafe { hirari_effect_chain_reset(state) };
        let mut gui_len = 0usize;
        assert!(
            unsafe { hirari_effect_chain_processor_gui_state(state, 0, &mut gui_len) }.is_null()
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn traversal_runs_the_published_nodes() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 7, false, false) });
        assert!(unsafe { hirari_effect_chain_set_sample_rate(state, 48_000.0, 256) });
        let storage = { hirari_audio_buffer_storage_create() };
        assert_eq!(
            unsafe { hirari_audio_buffer_storage_reserve_buffer(storage, 2, 256) },
            0
        );
        assert!(unsafe { hirari_audio_buffer_storage_resize(storage, 2, 256) });
        let planes =
            unsafe { hirari_audio_buffer_storage_channel_pointers(storage as *const _) };
        for plane in 0..2 {
            let pointer = unsafe { *planes.add(plane) };
            assert!(!pointer.is_null());
            for frame in 0..256 {
                unsafe { pointer.add(frame).write(0.25) };
            }
        }
        let midi_state = { crate::midi_buffer_ops::hirari_midi_buffer_create() };
        unsafe {
            hirari_effect_chain_process(
                state,
                storage,
                midi_state,
                120.0,
                std::ptr::null_mut(),
                0,
            )
        };
        for plane in 0..2 {
            for frame in 0..256 {
                let sample = unsafe { (*planes.add(plane)).add(frame).read() };
                assert!(sample.is_finite(), "chain output went non-finite: {sample}");
            }
        }
        unsafe {
            crate::midi_buffer_ops::hirari_midi_buffer_destroy(midi_state);
            hirari_audio_buffer_storage_destroy(storage);
            hirari_effect_chain_destroy(state);
        }
    }

    #[test]
    fn clear_retires_nodes_and_metrics_stay_readable() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 0, false, false) });
        assert_eq!(
            unsafe { hirari_effect_chain_node_kind(state as *const _, 0) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_metric(state as *const _, 0) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_metric(state as *const _, 1) },
            0
        );
        assert_eq!(unsafe { hirari_effect_chain_metric(state as *const _, 9) }, 0);
        let mut value = 0_u64;
        assert!(unsafe { hirari_effect_chain_metric_try(state as *const _, 2, &mut value) });
        unsafe { hirari_effect_chain_reclaim(state) };
        unsafe { clear(state) };
        assert_eq!(unsafe { hirari_effect_chain_count(state as *const _) }, 0);
        assert_eq!(
            unsafe { hirari_effect_chain_total_latency_samples(state as *const _) },
            0
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn diagnostics_never_block_on_control_thread_mutation() {
        let state = chain();
        assert!(unsafe { hirari_effect_chain_add_builtin(state, 1, false, false) });
        let address = state as usize;
        let stop = Arc::new(AtomicU8::new(0));
        let hog_stop = stop.clone();
        let hog = std::thread::spawn(move || {
            let state = address as *mut c_void;
            while hog_stop.load(Ordering::Acquire) == 0 {
                unsafe { hirari_effect_chain_set_sample_rate(state, 48_000.0, 512) };
            }
        });
        let mut worst = Duration::ZERO;
        for _ in 0..300 {
            let started = Instant::now();
            let mut value = 0_u64;
            unsafe {
                let _ = hirari_effect_chain_metric_try(state as *const _, 2, &mut value);
                let _ = hirari_effect_chain_metric_try(state as *const _, 1, &mut value);
                let _ = hirari_effect_chain_non_finite_samples(state as *const _);
            }
            worst = worst.max(started.elapsed());
            std::thread::yield_now();
        }
        stop.store(1, Ordering::Release);
        hog.join();
        assert!(
            worst < Duration::from_millis(5),
            "diagnostic reader blocked on the control thread: {worst:?}"
        );
        assert_eq!(
            unsafe { hirari_effect_chain_non_finite_samples(state as *const _) },
            unsafe { hirari_effect_chain_metric(state as *const _, 1) },
            "the cached non-finite poll must report the same count as the blocking read"
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }

    #[test]
    fn unknown_sandbox_paths_are_rejected_with_a_stable_text() {
        let state = chain();
        let path = b"/nonexistent/definitely-not-a-plugin.vst3";
        assert!(!unsafe {
            hirari_effect_chain_add_sandbox(state, path.as_ptr(), path.len())
        });
        let text =
            unsafe { hirari_effect_chain_last_sandbox_failure_text(state as *const _) };
        assert!(!text.is_null());
        let actual = unsafe { CStr::from_ptr(text) }.to_str().unwrap().to_string();
        assert!(!actual.is_empty());
        assert_eq!(
            unsafe { hirari_effect_chain_count(state as *const _) },
            0
        );
        assert_eq!(
            unsafe { hirari_effect_chain_sandbox_state_error(state as *const _, 0) },
            STATE_ERROR_UNAVAILABLE
        );
        unsafe { hirari_effect_chain_destroy(state) };
    }
