//! Tests split out of bounce_driver.rs.
    use super::*;
    use crate::bounce_plan::BOUNCE_PLAN_INVALID_RATE;
    use crate::engine_block_host::{
        hirari_engine_block_create, hirari_engine_block_destroy, HirariEngineBlockHandles,
    };
    use crate::master_limiter::{hirari_master_limiter_create, hirari_master_limiter_destroy};

    struct Handle(*mut c_void);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { hirari_bounce_session_destroy(self.0) };
        }
    }

    fn new_session() -> Handle {
        Handle(hirari_bounce_session_create())
    }

    fn borrow(handle: &Handle) -> &'static BounceSession {
        unsafe { &*(handle.0.cast::<BounceSession>()) }
    }

    /// A host that owns the Rust stores the plan walk reads, so the extent can
    /// be checked against real handles instead of a scripted callback.
    struct FakeEngine {
        block: *mut c_void,
        limiter: *mut c_void,
    }

    impl FakeEngine {
        fn new() -> Self {
            Self {
                // SAFETY: the handles are read once and all null.
                block: unsafe { hirari_engine_block_create(HirariEngineBlockHandles::default()) },
                limiter: hirari_master_limiter_create(44_100.0),
            }
        }

        fn host(&self) -> *mut c_void {
            self as *const Self as *mut c_void
        }

        fn plan(&self, session: &BounceSession) -> Result<HirariBouncePlan, i32> {
            gather_plan(self.host(), fake_store, session)
        }
    }

    impl Drop for FakeEngine {
        fn drop(&mut self) {
            unsafe {
                hirari_engine_block_destroy(self.block);
                hirari_master_limiter_destroy(self.limiter);
            }
        }
    }

    /// The graph stores stay absent: an empty arrangement is the fallback case.
    unsafe extern "C" fn fake_store(host: *mut c_void, kind: u32) -> *mut c_void {
        let engine = unsafe { &*(host as *const FakeEngine) };
        match kind {
            STORE_ENGINE_BLOCK => engine.block,
            STORE_MASTER_LIMITER => engine.limiter,
            _ => std::ptr::null_mut(),
        }
    }

    /// The fake host doubles as the session: the worker body only publishes.
    fn start(handle: &Handle, ops: &HirariBounceHostOps, path: &[u8], format: u32) -> bool {
        unsafe {
            hirari_bounce_start_async(
                handle.0,
                handle.0,
                ops,
                path.as_ptr(),
                path.len(),
                format,
            )
        }
    }

    #[test]
    fn thread_tokens_are_stable_per_thread_and_unique_across_threads() {
        let first = hirari_callback_thread_token();
        assert_eq!(first, hirari_callback_thread_token());
        let other = std::thread::spawn(|| hirari_callback_thread_token())
            .join()
            .expect("token thread");
        assert_ne!(first, other);
    }

    #[test]
    fn missing_operations_fail_closed() {
        let ops = HirariBounceHostOps {
            run: None,
            reset: None,
            store: None,
            render: None,
            render_chunk: None,
            read_transport: None,
            set_transport: None,
        };
        assert!(missing(&ops));
        let guard = new_session();
        let path = b"/tmp/hirari-bounce.wav";
        let ok = unsafe {
            hirari_bounce_render_driver(
                &mut 0_u8 as *mut u8 as *mut c_void,
                guard.0,
                &ops,
                path.as_ptr(),
                path.len(),
                0,
            )
        };
        assert!(!ok);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_IDLE);
    }

    #[test]
    fn the_plan_extends_the_device_rate_with_the_session_and_limiter_tails() {
        let engine = FakeEngine::new();
        let guard = new_session();
        let session = borrow(&guard);
        let baseline = engine.plan(session).expect("empty arrangement");
        assert_eq!(baseline.output_rate, 44_100);
        assert_eq!(baseline.playhead_start, 0);
        // The fallback one second plus the master limiter's own release tail.
        assert_eq!(
            baseline.total_samples,
            44_100 + u64::from(unsafe { hirari_master_limiter_tail(engine.limiter as *const c_void) })
        );
        hirari_bounce_set_tail_seconds(guard.0, 0.5);
        let extended = engine.plan(session).expect("tailed plan");
        assert_eq!(extended.total_samples, baseline.total_samples + 22_050);
    }

    #[test]
    fn an_unsupported_device_rate_rejects_the_render() {
        unsafe extern "C" fn zero_rate(_: *mut c_void, _: u32) -> *mut c_void {
            std::ptr::null_mut()
        }
        let guard = new_session();
        // No engine block means no known rate, which is a rejected plan rather
        // than a silent one-second render.
        let error = gather_plan(
            &mut 0_u8 as *mut u8 as *mut c_void,
            zero_rate,
            borrow(&guard),
        )
        .expect_err("rate must be read from the graph");
        assert_eq!(error, BOUNCE_PLAN_INVALID_RATE);
    }

    #[test]
    fn unsupported_output_publishes_a_failed_state() {
        unsafe extern "C" fn run(_: *mut c_void, _: *const u8, _: usize, _: u32) -> u8 {
            1
        }
        unsafe extern "C" fn reset(_: *mut c_void) {}
        unsafe extern "C" fn store(_: *mut c_void, _: u32) -> *mut c_void {
            std::ptr::null_mut()
        }
        unsafe extern "C" fn render(_: *mut c_void, _: *mut f32, _: *mut f32, _: u32) {}
        unsafe extern "C" fn chunk(_: *mut c_void) -> u32 {
            64
        }
        unsafe extern "C" fn read(_: *mut c_void, playhead: *mut u64, playing: *mut u8) {
            unsafe {
                *playhead = 7;
                *playing = 0;
            }
        }
        unsafe extern "C" fn set(_: *mut c_void, _: u64, _: u8) {}
        let ops = HirariBounceHostOps {
            run: Some(run),
            reset: Some(reset),
            store: Some(store),
            render: Some(render),
            render_chunk: Some(chunk),
            read_transport: Some(read),
            set_transport: Some(set),
        };
        let guard = new_session();
        let path = b"/tmp/hirari-bounce.txt";
        let ok = unsafe {
            hirari_bounce_render_driver(
                &mut 0_u8 as *mut u8 as *mut c_void,
                guard.0,
                &ops,
                path.as_ptr(),
                path.len(),
                0,
            )
        };
        assert!(!ok);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_FAILED);
        assert_eq!(hirari_bounce_progress(guard.0), 0.0);
        // The rejected render never took graph ownership.
        assert_eq!(hirari_bounce_offline_active(guard.0), 0);
    }

    #[test]
    fn ownership_gate_publishes_progress_state_and_protects_the_graph() {
        let guard = new_session();
        let state = borrow(&guard);
        assert_eq!(hirari_bounce_offline_blocks(guard.0, 1), 0);
        assert_eq!(state.begin(), OWNERSHIP_ACQUIRED);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_RENDERING);
        assert_eq!(hirari_bounce_offline_active(guard.0), 1);
        assert_eq!(hirari_bounce_offline_thread(), 1);
        assert_eq!(hirari_bounce_offline_engaged(guard.0), 1);
        // The owning thread is never blocked by its own render.
        assert_eq!(hirari_bounce_offline_blocks(guard.0, hirari_callback_thread_token()), 0);
        // Any other thread must keep out of the temporary offline graph.
        let address = guard.0 as usize;
        let foreign = std::thread::spawn(move || {
            hirari_bounce_offline_blocks(address as *mut c_void, 9_999)
        })
        .join()
        .expect("callback thread");
        assert_eq!(foreign, 1);
        state.end();
        assert_eq!(hirari_bounce_offline_thread(), 0);
        assert_eq!(hirari_bounce_offline_active(guard.0), 0);
    }

    #[test]
    fn a_cancel_requested_while_the_graph_is_busy_abandons_the_render() {
        let guard = new_session();
        let state = borrow(&guard);
        // A queued async render keeps its cancellation request; begin() only
        // discards a stale flag for a fresh synchronous bounce.
        state.running.store(true, Ordering::Release);
        assert_eq!(state.begin(), OWNERSHIP_ACQUIRED);
        // A second render on another thread can only wait for this one.
        let address = guard.0 as usize;
        let other = std::thread::spawn(move || {
            let other = unsafe { &*(address as *const BounceSession) };
            other.cancel.store(true, Ordering::Release);
            other.begin()
        })
        .join()
        .expect("waiting render");
        assert_eq!(other, OWNERSHIP_CANCELLED);
        // The abandoned render must not have released the first one's gate.
        assert_eq!(hirari_bounce_offline_active(guard.0), 1);
        state.end();
    }

    #[test]
    fn control_requests_follow_the_lifecycle() {
        let guard = new_session();
        let state = borrow(&guard);
        assert!(!hirari_bounce_pause(guard.0));
        assert!(!hirari_bounce_resume(guard.0));
        assert!(!hirari_bounce_cancel(guard.0));
        state.publish(0.4, BOUNCE_STATE_RENDERING);
        assert!(hirari_bounce_progress(guard.0) > 0.39);
        assert!(hirari_bounce_pause(guard.0));
        assert_eq!(hirari_bounce_control_bits(guard.0), CONTROL_PAUSE);
        state.publish(0.4, BOUNCE_STATE_PAUSED);
        assert!(hirari_bounce_resume(guard.0));
        assert_eq!(hirari_bounce_control_bits(guard.0), 0);
        // Cancelling a running render clears a pending pause so the worker
        // cannot stall on the pause gate before it sees the cancellation.
        state.publish(0.4, BOUNCE_STATE_RENDERING);
        assert!(hirari_bounce_pause(guard.0));
        assert!(hirari_bounce_cancel(guard.0));
        assert_eq!(hirari_bounce_control_bits(guard.0), CONTROL_CANCEL);
        // A finished render accepts no further control requests.
        state.publish(1.0, BOUNCE_STATE_COMPLETED);
        assert!(!hirari_bounce_cancel(guard.0));
        assert!(!hirari_bounce_pause(guard.0));
    }

    #[test]
    fn cancellation_beats_the_publish_boundary() {
        let guard = new_session();
        let state = borrow(&guard);
        // A stream that cannot be finished reports a publish failure.
        assert_eq!(state.finish_stream(std::ptr::null_mut()), 2);
        state.cancel.store(true, Ordering::Release);
        assert_eq!(state.finish_stream(std::ptr::null_mut()), 0);
    }

    #[test]
    fn offline_target_options_round_trip() {
        let guard = new_session();
        hirari_bounce_set_offline_target(guard.0, 12);
        assert_eq!(hirari_bounce_target_active(guard.0), 1);
        assert_eq!(hirari_bounce_target_id(guard.0), 12);
        hirari_bounce_set_tail_seconds(guard.0, 1.5);
        assert_eq!(hirari_bounce_tail_millis(guard.0), 1_500);
        hirari_bounce_set_render_options(guard.0, 1, 0);
        assert_eq!(hirari_bounce_pre_fader(guard.0), 1);
        assert_eq!(hirari_bounce_include_inserts(guard.0), 0);
        hirari_bounce_clear_offline_target(guard.0);
        assert_eq!(hirari_bounce_target_active(guard.0), 0);
        assert_eq!(hirari_bounce_target_id(guard.0), 0);
        assert_eq!(hirari_bounce_tail_millis(guard.0), 0);
        assert_eq!(hirari_bounce_pre_fader(guard.0), 0);
        assert_eq!(hirari_bounce_include_inserts(guard.0), 1);
    }

    #[test]
    fn an_async_render_is_retired_before_the_next_start() {
        static DONE: AtomicU32 = AtomicU32::new(0);
        static RELEASE: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn run(host: *mut c_void, _: *const u8, _: usize, _: u32) -> u8 {
            while !RELEASE.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            DONE.fetch_add(1, Ordering::SeqCst);
            // Stand in for the synchronous driver: publish a terminal state.
            let session = unsafe { &*(host as *const BounceSession) };
            session.publish(1.0, BOUNCE_STATE_COMPLETED);
            1
        }
        static OPS: HirariBounceHostOps = HirariBounceHostOps {
            run: Some(run),
            reset: None,
            store: None,
            render: None,
            render_chunk: None,
            read_transport: None,
            set_transport: None,
        };
        DONE.store(0, Ordering::SeqCst);
        RELEASE.store(false, Ordering::SeqCst);
        let guard = new_session();
        // A rejected path or format publishes the failure without a worker.
        assert!(!start(&guard, &OPS, b"", 0));
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_FAILED);
        let path = b"/tmp/hirari-async.wav";
        assert!(start(&guard, &OPS, path, 0));
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_QUEUED);
        // A running render is refused until it retires.
        assert!(!start(&guard, &OPS, path, 0));
        RELEASE.store(true, Ordering::SeqCst);
        hirari_bounce_join(guard.0);
        assert_eq!(DONE.load(Ordering::SeqCst), 1);
        assert!(!borrow(&guard).running.load(Ordering::Acquire));
        // The retired slot accepts the next render.
        RELEASE.store(false, Ordering::SeqCst);
        assert!(start(&guard, &OPS, path, 0));
        RELEASE.store(true, Ordering::SeqCst);
        hirari_bounce_join(guard.0);
        assert_eq!(DONE.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_completed_render_publishes_the_whole_file() {
        unsafe extern "C" fn run(_: *mut c_void, _: *const u8, _: usize, _: u32) -> u8 {
            1
        }
        unsafe extern "C" fn reset(_: *mut c_void) {}
        unsafe extern "C" fn render(_: *mut c_void, left: *mut f32, right: *mut f32, frames: u32) {
            for index in 0..frames as usize {
                unsafe {
                    *left.add(index) = 0.25;
                    *right.add(index) = -0.25;
                }
            }
        }
        unsafe extern "C" fn chunk(_: *mut c_void) -> u32 {
            1_024
        }
        unsafe extern "C" fn read(_: *mut c_void, playhead: *mut u64, playing: *mut u8) {
            unsafe {
                *playhead = 3;
                *playing = 0;
            }
        }
        unsafe extern "C" fn set(_: *mut c_void, playhead: u64, _: u8) {
            LAST_TRANSPORT.store(playhead, Ordering::SeqCst);
        }
        static LAST_TRANSPORT: AtomicU64 = AtomicU64::new(0);
        let engine = FakeEngine::new();
        let ops = HirariBounceHostOps {
            run: Some(run),
            reset: Some(reset),
            store: Some(fake_store),
            render: Some(render),
            render_chunk: Some(chunk),
            read_transport: Some(read),
            set_transport: Some(set),
        };
        let guard = new_session();
        let plan = engine
            .plan(borrow(&guard))
            .expect("the fake graph plans a render");
        let directory = std::env::temp_dir().join(format!("hirari-bounce-{:p}", guard.0));
        std::fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("mix.wav");
        let raw = path.to_string_lossy().into_owned();
        let ok = unsafe {
            hirari_bounce_render_driver(
                engine.host(),
                guard.0,
                &ops,
                raw.as_ptr().cast(),
                raw.len(),
                0,
            )
        };
        assert!(ok);
        assert_eq!(hirari_bounce_state(guard.0), BOUNCE_STATE_COMPLETED);
        assert_eq!(hirari_bounce_progress(guard.0), 1.0);
        assert_eq!(hirari_bounce_offline_active(guard.0), 0);
        assert_eq!(hirari_bounce_offline_thread(), 0);
        // The transport is handed back to whatever the engine had before.
        assert_eq!(LAST_TRANSPORT.load(Ordering::SeqCst), 3);
        // The published file is exactly the extent the Rust plan measured.
        let payload = plan.total_samples * 4;
        let written = std::fs::metadata(&path).expect("rendered file").len();
        assert!(
            written >= payload && written <= payload + 4_096,
            "{written} bytes for {payload} samples of payload"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
