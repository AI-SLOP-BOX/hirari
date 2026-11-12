//! Tests split out of mac_audio_driver.rs.
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// Real-device contract for the CoreAudio lifecycle, ported from the
    /// `tests/macos_audio_device_contract.cpp` binary that used to drive it
    /// through the C++ adapter.
    ///
    /// `#[ignore]`d because CI hosts have no usable default output device;
    /// `scripts/run_macos_audio_device_contract.sh` runs it explicitly. Every
    /// rejection names itself: a silent failure is indistinguishable from "no
    /// device on this host".
    #[test]
    #[ignore]
    fn real_device_starts_renegotiates_and_stops() {
        static CALLBACKS: AtomicU64 = AtomicU64::new(0);
        static FRAMES: AtomicU64 = AtomicU64::new(0);

        unsafe extern "C" fn render(
            _inputs: *const *const f32,
            _input_channels: u32,
            outputs: *const *mut f32,
            output_channels: u32,
            frames: u32,
            _context: *mut c_void,
        ) {
            CALLBACKS.fetch_add(1, Ordering::Release);
            FRAMES.fetch_add(u64::from(frames), Ordering::Release);
            if outputs.is_null() {
                return;
            }
            for channel in 0..output_channels {
                let destination = *outputs.add(channel as usize);
                if destination.is_null() {
                    continue;
                }
                std::slice::from_raw_parts_mut(destination, frames as usize).fill(0.0);
            }
        }

        fn rejection(host: &MacAudioHost, reason: &str) -> String {
            format!(
                "macOS CoreAudio contract: {reason} (status={}, error={})",
                host.status(),
                host.last_error_code()
            )
        }

        // A rejection stops the device before naming itself, so the reported
        // status describes the settled driver and the host never leaves a live
        // AudioUnit behind for the next process.
        fn require(host: &MacAudioHost, accepted: bool, reason: &str) {
            if accepted {
                return;
            }
            host.stop();
            panic!("{}", rejection(host, reason));
        }

        let host = MacAudioHost::new(Box::new(CoreAudioBackend::new()), None, ptr::null_mut());
        host.set_process_callback(Some(render), ptr::null_mut());

        // First generation: negotiate 48k/128 against the real device.
        require(
            &host,
            host.reconfigure(FormatRequest {
                sample_rate: 48_000.0,
                buffer_size: 128,
            }),
            "cannot open the default output device at 48000/128",
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while CALLBACKS.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        require(
            &host,
            host.is_running()
                && CALLBACKS.load(Ordering::Acquire) > 0
                && FRAMES.load(Ordering::Acquire) > 0,
            "the realtime callback never ran",
        );
        require(
            &host,
            host.sample_rate() == 48_000.0
                && host.buffer_size() == 128
                && host.output_channel_count() > 0,
            "the negotiated generation does not match the request",
        );

        // Second generation: renegotiate while the driver is live.
        require(
            &host,
            host.reconfigure(FormatRequest {
                sample_rate: 96_000.0,
                buffer_size: 512,
            }),
            "cannot renegotiate the default output device to 96000/512",
        );
        require(
            &host,
            host.sample_rate() == 96_000.0 && host.buffer_size() == 512,
            "the second generation does not match the request",
        );
        require(&host, host.is_running(), "the driver stopped after renegotiation");

        host.stop();
        require(&host, !host.is_running(), "stop left the driver running");
    }

    #[derive(Default)]
    struct FakeStats {
        opens: AtomicU32,
        closes: AtomicU32,
        starts: AtomicU32,
        default_changed: AtomicBool,
        input_changed: AtomicBool,
    }

    struct FakeBackend {
        stats: Arc<FakeStats>,
        default_device: Result<u32, i32>,
        nominal: Option<f64>,
        selected_nominal: Option<f64>,
        buffer: Option<u32>,
        channels: (u32, u32),
        opened: bool,
        realtime: Option<Arc<RealtimeState>>,
        fail_open: Option<DriverError>,
    }

    impl FakeBackend {
        fn healthy() -> (Self, Arc<FakeStats>, Arc<Mutex<Vec<String>>>) {
            let stats = Arc::new(FakeStats::default());
            let logs = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    stats: Arc::clone(&stats),
                    default_device: Ok(7),
                    nominal: Some(48_000.0),
                    selected_nominal: Some(96_000.0),
                    buffer: Some(256),
                    channels: (2, 2),
                    opened: false,
                    realtime: None,
                    fail_open: None,
                },
                stats,
                logs,
            )
        }
    }

    impl AudioBackend for FakeBackend {
        fn default_output_device(&self) -> Result<u32, i32> {
            self.default_device
        }

        fn nominal_sample_rate(&self, device: u32) -> Option<f64> {
            if self.default_device == Ok(device) {
                self.nominal
            } else {
                self.selected_nominal.or(self.nominal)
            }
        }

        fn buffer_frame_size(&self, _device: u32) -> Option<u32> {
            self.buffer
        }

        fn io_latency_seconds(
            &self,
            _device: u32,
            _unit: sys::AudioUnit,
            _sample_rate: f64,
            _buffer_frames: u32,
        ) -> f64 {
            0.0
        }

        fn channel_counts(&self, _device: u32) -> Result<(u32, u32), DriverError> {
            Ok(self.channels)
        }

        fn open(
            &mut self,
            request: FormatRequest,
            _requested_device: u32,
            channels: ChannelMap,
            realtime: Arc<RealtimeState>,
        ) -> Result<u32, DriverError> {
            if let Some(error) = &self.fail_open {
                return Err(error.clone());
            }
            self.stats.opens.fetch_add(1, Ordering::Relaxed);
            self.opened = true;
            self.realtime = Some(Arc::clone(&realtime));
            realtime
                .buffer_size
                .store(request.buffer_size, Ordering::Release);
            realtime
                .output_channels
                .store(channels.outputs, Ordering::Release);
            realtime
                .input_channels
                .store(channels.inputs, Ordering::Release);
            Ok(7)
        }

        fn start(&mut self) -> Result<(), DriverError> {
            self.stats.starts.fetch_add(1, Ordering::Relaxed);
            if let Some(realtime) = &self.realtime {
                realtime.running.store(true, Ordering::Release);
            }
            Ok(())
        }

        fn close(&mut self) {
            if self.opened {
                self.stats.closes.fetch_add(1, Ordering::Relaxed);
            }
            self.opened = false;
            if let Some(realtime) = self.realtime.take() {
                realtime.running.store(false, Ordering::Release);
            }
        }

        fn is_open(&self) -> bool {
            self.opened
        }

        fn device_uid(&self, _device: u32) -> Option<String> {
            Some("fake-uid".to_owned())
        }

        fn devices_json(&self) -> String {
            "[{\"id\":7,\"uid\":\"fake-uid\",\"name\":\"Fake Device\",\"input\":true,\"input_channels\":2,\"input_channel_names\":[\"Input 1\",\"Input 2\"],\"output\":true,\"output_channels\":2,\"output_channel_names\":[\"Output 1\",\"Output 2\"]}]".to_owned()
        }

        fn take_default_device_change(&self) -> bool {
            self.stats.default_changed.swap(false, Ordering::AcqRel)
        }

        fn take_input_endpoint_change(&self) -> bool {
            self.stats.input_changed.swap(false, Ordering::AcqRel)
        }
    }

    unsafe extern "C" fn test_log(
        context: *mut c_void,
        _level: u32,
        message: *const c_char,
    ) {
        if context.is_null() || message.is_null() {
            return;
        }
        let logs = &*(context as *const Mutex<Vec<String>>);
        let message = CStr::from_ptr(message).to_string_lossy().into_owned();
        logs.lock().unwrap().push(message);
    }

    fn fake_host() -> (MacAudioHost, Arc<FakeStats>, Arc<Mutex<Vec<String>>>) {
        let (backend, stats, logs) = FakeBackend::healthy();
        let host = MacAudioHost::new(
            Box::new(backend),
            Some(test_log),
            Arc::as_ptr(&logs) as *mut c_void,
        );
        (host, stats, logs)
    }

    #[test]
    fn start_and_stop_are_idempotent() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        assert!(host.start(None));
        assert!(host.is_running());
        assert_eq!(stats.opens.load(Ordering::Relaxed), 1);
        assert_eq!(stats.starts.load(Ordering::Relaxed), 1);

        host.stop();
        assert!(!host.is_running());
        assert_eq!(host.status(), "stopped");
        host.stop();
        assert_eq!(host.status(), "stopped");
        assert_eq!(stats.closes.load(Ordering::Relaxed), 1);

        assert!(host.start(None));
        assert!(host.is_running());
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn device_removal_during_run_recovers_through_try_reconnect() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        assert!(host.is_running());

        // The CoreAudio alive listener clears `running` when the selected
        // device disappears; the control plane notices and restarts.
        host.realtime.mark_device_lost();
        assert!(!host.is_running());
        host.try_reconnect();
        assert!(host.is_running());
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert_eq!(host.status(), "running");
    }

    #[test]
    fn default_device_change_forces_a_restart() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        stats.default_changed.store(true, Ordering::Release);
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert!(host.is_running());
    }

    #[test]
    fn composed_input_endpoint_change_forces_a_restart() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        stats.input_changed.store(true, Ordering::Release);
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert!(host.is_running());
    }

    #[test]
    fn both_device_change_flags_are_consumed_by_one_reconnect() {
        let (host, stats, _logs) = fake_host();
        assert!(host.start(None));
        stats.default_changed.store(true, Ordering::Release);
        stats.input_changed.store(true, Ordering::Release);
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        // The input flag must not survive to restart the new generation.
        host.try_reconnect();
        assert_eq!(stats.opens.load(Ordering::Relaxed), 2);
        assert!(host.is_running());
    }

    #[test]
    fn invalid_format_is_rejected_with_a_clear_error() {
        let (host, stats, logs) = fake_host();
        assert!(!host.start(Some(FormatRequest {
            sample_rate: f64::NAN,
            buffer_size: 256,
        })));
        assert_eq!(host.status(), "start-failed");
        assert_eq!(host.last_error_code(), -1);
        assert_eq!(stats.opens.load(Ordering::Relaxed), 0);
        {
            let captured = logs.lock().unwrap();
            assert!(
                captured
                    .iter()
                    .any(|message| message.contains("invalid CoreAudio configuration")
                        && message.contains("sample rate")),
                "logs: {captured:?}"
            );
        }

        assert!(!host.start(Some(FormatRequest {
            sample_rate: 48_000.0,
            buffer_size: MAX_FRAMES_PER_SLICE + 1,
        })));
        assert_eq!(host.last_error_code(), -1);
        {
            let captured = logs.lock().unwrap();
            assert!(
                captured
                    .iter()
                    .any(|message| message.contains("buffer size") && message.contains("8192")),
                "logs: {captured:?}"
            );
        }

        // A valid request afterwards recovers the generation.
        assert!(host.start(Some(FormatRequest {
            sample_rate: 48_000.0,
            buffer_size: 256,
        })));
        assert_eq!(host.status(), "running");
        assert_eq!(host.last_error_code(), 0);
    }

    #[test]
    fn odd_frame_counts_split_without_losing_tail_frames() {
        assert_eq!(
            split_frames(1_000, 512).collect::<Vec<_>>(),
            vec![(0, 512), (512, 488)]
        );
        assert_eq!(
            split_frames(1_023, 512).collect::<Vec<_>>(),
            vec![(0, 512), (512, 511)]
        );
        assert_eq!(
            split_frames(513, 512).collect::<Vec<_>>(),
            vec![(0, 512), (512, 1)]
        );
        assert_eq!(split_frames(256, 512).collect::<Vec<_>>(), vec![(0, 256)]);
        assert_eq!(split_frames(0, 512).collect::<Vec<_>>(), Vec::new());
    }

    struct ChunkRecorder {
        chunks: Mutex<Vec<(u32, usize)>>,
    }

    unsafe extern "C" fn record_chunks(
        _inputs: *const *const f32,
        _input_channels: u32,
        outputs: *const *mut f32,
        output_channels: u32,
        frames: u32,
        context: *mut c_void,
    ) {
        let recorder = &*(context as *const ChunkRecorder);
        recorder
            .chunks
            .lock()
            .unwrap()
            .push((frames, output_channels as usize));
        for channel in 0..output_channels as usize {
            let output = *outputs.add(channel);
            if !output.is_null() {
                for frame in 0..frames as usize {
                    output.add(frame).write(0.25);
                }
            }
        }
    }

    #[test]
    fn realtime_dispatch_splits_odd_callbacks_into_bounded_chunks() {
        let realtime = RealtimeState::new();
        realtime.running.store(true, Ordering::Release);
        realtime.buffer_size.store(512, Ordering::Release);
        realtime.output_channels.store(2, Ordering::Release);
        let recorder = Arc::new(ChunkRecorder {
            chunks: Mutex::new(Vec::new()),
        });
        realtime.set_process_callback(
            Some(record_chunks),
            Arc::as_ptr(&recorder) as *mut c_void,
        );

        let mut left = vec![0.0f32; 1_023];
        let mut right = vec![0.0f32; 1_023];
        let mut outputs: [*mut f32; MAX_CHANNELS] = [ptr::null_mut(); MAX_CHANNELS];
        outputs[0] = left.as_mut_ptr();
        outputs[1] = right.as_mut_ptr();
        let dispatched = unsafe {
            dispatch_planar_outputs(
                &realtime,
                ptr::null(),
                0,
                &outputs,
                2,
                1_023,
            )
        };
        assert!(dispatched);
        assert_eq!(
            *recorder.chunks.lock().unwrap(),
            vec![(512, 2), (511, 2)]
        );
        assert_eq!(left[1_022], 0.25);
        assert_eq!(right[1_022], 0.25);
        assert_eq!(realtime.stats.callback_count(), 1);
        assert!(realtime.stats.output_peak() >= 0.25);
    }

    #[test]
    fn channel_map_rejects_the_mono_device_the_deleted_cpp_contract_forced() {
        // The deleted `Hirari::Platform` device contract opened a mono output
        // to prove the backend did not assume two buffers. The live driver
        // negotiates channels from the device instead of letting the caller
        // pick them, and refuses anything that cannot cover the stereo engine
        // boundary, so the assertion now sits on the mapping policy.
        assert_eq!(channel_map(1, 1), Err(DriverError::NoOutputChannels(1)));
        assert_eq!(channel_map(0, 2), Err(DriverError::NoInputChannels(0)));
        assert_eq!(
            channel_map(40, 40),
            Ok(ChannelMap {
                inputs: 32,
                outputs: 32
            })
        );
    }

    // --- duplex composition (private aggregate) -------------------------

    #[test]
    fn a_duplex_output_device_is_opened_without_an_aggregate() {
        assert_eq!(
            plan_duplex_composition(4, 2, 9, 1, Some("BuiltInMic"), Some("Interface")),
            None
        );
        // Even the device being its own default input stays untouched.
        assert_eq!(
            plan_duplex_composition(4, 1, 4, 1, Some("Duplex"), Some("Duplex")),
            None
        );
    }

    #[test]
    fn a_missing_default_input_device_never_composes() {
        assert_eq!(
            plan_duplex_composition(4, 0, 0, 1, None, Some("Speakers")),
            None
        );
        assert_eq!(
            plan_duplex_composition(4, 0, 0, 0, None, Some("Speakers")),
            None
        );
    }

    #[test]
    fn an_input_device_without_channels_never_composes() {
        assert_eq!(
            plan_duplex_composition(4, 0, 9, 0, Some("Useless"), Some("Speakers")),
            None
        );
    }

    #[test]
    fn an_output_device_without_a_uid_never_composes() {
        assert_eq!(
            plan_duplex_composition(4, 0, 9, 1, Some("Mic"), Some("")),
            None
        );
        assert_eq!(
            plan_duplex_composition(4, 0, 9, 1, Some("Mic"), None),
            None
        );
    }

    #[test]
    fn an_output_only_device_composes_with_both_sub_device_uids() {
        let composition = plan_duplex_composition(
            4,
            0,
            9,
            1,
            Some("AppleHDAEngineInput:1B,0,18,1"),
            Some("AppleHDAEngineOutput:1B,0,19,3"),
        )
        .expect("an output-only device plus a real input device must compose");
        assert_eq!(composition.input_uid, "AppleHDAEngineInput:1B,0,18,1");
        assert_eq!(composition.output_uid, "AppleHDAEngineOutput:1B,0,19,3");
    }

    /// Backend whose default output device is output-only (0, 2) and only
    /// becomes openable once `resolve_for_open` has composed a device: the
    /// composed id reports (1, 2), and `open` records what it was handed.
    struct ComposingBackend {
        composed: u32,
        opened_with: Arc<Mutex<Option<u32>>>,
        probed: Arc<Mutex<Vec<u32>>>,
        opened: bool,
        realtime: Option<Arc<RealtimeState>>,
    }

    impl ComposingBackend {
        /// Builds a host plus the probes the assertions read back without
        /// having to downcast the trait object.
        fn host(
            composed: u32,
        ) -> (MacAudioHost, Arc<Mutex<Option<u32>>>, Arc<Mutex<Vec<u32>>>) {
            let opened_with = Arc::new(Mutex::new(None));
            let probed = Arc::new(Mutex::new(Vec::new()));
            let backend = Self {
                composed,
                opened_with: Arc::clone(&opened_with),
                probed: Arc::clone(&probed),
                opened: false,
                realtime: None,
            };
            let host = MacAudioHost::new(Box::new(backend), None, ptr::null_mut());
            (host, opened_with, probed)
        }
    }

    impl AudioBackend for ComposingBackend {
        fn default_output_device(&self) -> Result<u32, i32> {
            Ok(4)
        }

        fn nominal_sample_rate(&self, _device: u32) -> Option<f64> {
            Some(48_000.0)
        }

        fn buffer_frame_size(&self, _device: u32) -> Option<u32> {
            Some(256)
        }

        fn io_latency_seconds(
            &self,
            _device: u32,
            _unit: sys::AudioUnit,
            _sample_rate: f64,
            _buffer_frames: u32,
        ) -> f64 {
            0.0
        }

        fn channel_counts(&self, device: u32) -> Result<(u32, u32), DriverError> {
            self.probed.lock().unwrap().push(device);
            if device == self.composed {
                Ok((1, 2))
            } else {
                Ok((0, 2))
            }
        }

        fn open(
            &mut self,
            _request: FormatRequest,
            requested_device: u32,
            _channels: ChannelMap,
            realtime: Arc<RealtimeState>,
        ) -> Result<u32, DriverError> {
            *self.opened_with.lock().unwrap() = Some(requested_device);
            self.opened = true;
            self.realtime = Some(realtime);
            Ok(requested_device)
        }

        fn start(&mut self) -> Result<(), DriverError> {
            if let Some(realtime) = &self.realtime {
                realtime.running.store(true, Ordering::Release);
            }
            Ok(())
        }

        fn close(&mut self) {
            self.opened = false;
            if let Some(realtime) = self.realtime.take() {
                realtime.running.store(false, Ordering::Release);
            }
        }

        fn is_open(&self) -> bool {
            self.opened
        }

        fn device_uid(&self, device: u32) -> Option<String> {
            Some(format!("uid-{device}"))
        }

        fn devices_json(&self) -> String {
            "[]".to_owned()
        }

        fn take_default_device_change(&self) -> bool {
            false
        }

        fn resolve_for_open(&self, output_device: u32, _request: FormatRequest) -> u32 {
            if self.composed != 0 {
                self.composed
            } else {
                output_device
            }
        }

        fn input_endpoint_for(&self, opened_device: u32) -> u32 {
            if self.composed != 0 && opened_device == self.composed {
                9
            } else {
                opened_device
            }
        }
    }

    #[test]
    fn start_probes_and_opens_the_composed_device() {
        let (host, opened_with, probed) = ComposingBackend::host(42);
        assert!(host.start(None));
        assert_eq!(host.status(), "running");
        // The channel probe must see the composed device, otherwise the
        // output-only default would still reject with NoInputChannels.
        assert_eq!(*probed.lock().unwrap(), vec![42u32]);
        assert_eq!(*opened_with.lock().unwrap(), Some(42));
        host.stop();
        assert_eq!(host.status(), "stopped");
    }

    #[test]
    fn an_explicit_selection_negotiates_its_own_rate_not_the_defaults() {
        let (host, _stats, _logs) = fake_host();
        host.lock_core().selected_device = 11;
        assert!(host.start(None));
        assert_eq!(host.lock_core().sample_rate(), 96_000.0);
    }

    #[test]
    fn the_composed_aggregate_is_never_reported_as_the_input_endpoint() {
        let (host, _opened_with, _probed) = ComposingBackend::host(42);
        assert!(host.start(None));
        // The UI and recording key off the physical capture endpoint; the
        // private per-process aggregate UID would go stale on stop and can
        // never be re-selected or persisted.
        assert_eq!(host.selected_device_uid(), "uid-9");
        host.stop();
        assert_eq!(host.selected_device_uid(), "");
    }

    #[test]
    fn start_still_fails_exactly_as_before_when_nothing_is_composed() {
        let (host, opened_with, _probed) = ComposingBackend::host(0);
        assert!(!host.start(None));
        assert_eq!(host.status(), "start-failed");
        assert_eq!(host.last_error_code(), -1);
        // The identity path must not open anything: this is today's
        // output-only behaviour, rejection included.
        assert_eq!(*opened_with.lock().unwrap(), None);
    }
