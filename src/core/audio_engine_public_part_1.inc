public:
    AudioEngine();
    explicit AudioEngine(bool startDevice);
    ~AudioEngine();

    void set_playing(bool playing) const {
        (void)try_set_playing(playing);
    }
    bool try_set_playing(bool playing) const {
        if (!m_engine) return false;
        if (playing && !is_driver_running()) return false;
        m_engine->set_playing(playing);
        return true;
    }
    uintptr_t control_room_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->control_room_state()) : 0;
    }
    uintptr_t tempo_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->tempo_state()) : 0;
    }
    uintptr_t undo_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->undo_state()) : 0;
    }
    uintptr_t log_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->log_state()) : 0;
    }
    bool is_audio_device_ready() const {
        std::lock_guard<std::mutex> lock(m_configMutex);
        const bool ready = is_driver_running();
        if (!ready && m_engine && m_engine->is_playing()) {
            // Device loss is a transport safety boundary, not merely a UI
            // status change. Stop the graph even when no UI poll is running.
            m_engine->set_playing(false);
        }
        return ready;
    }
    bool is_driver_running() const noexcept {
        // Pure, side-effect-free view of the same driver lifecycle that
        // is_audio_device_ready() inspects. Elected by the telemetry cluster
        // (get_runtime_health_v path) so the native health snapshot reports
        // the real connection state without driving any transport action.
        return m_driver && hirari_audio_driver_host_is_running(m_driver);
    }
    // The status, telemetry and device catalog the UI polls are Rust-owned
    // (`mac_audio_driver.rs`) and read through `audio_driver_view` against this
    // handle, which the engine's lifetime bounds. A zero handle means "no host"
    // and answers exactly as the deleted null-`m_driver` forwarders did.
    uintptr_t audio_driver_host() const noexcept {
        return reinterpret_cast<uintptr_t>(m_driver);
    }
    void try_reconnect_audio_device() const {
        if (hirari_native_test_isolation()) return;
        std::lock_guard<std::mutex> lock(m_configMutex);
        if (m_driver) {
            hirari_audio_driver_host_try_reconnect(m_driver);
            if (!is_driver_running() && m_engine) m_engine->set_playing(false);
        }
    }
    bool select_audio_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize) const {
        if (!m_driver || !m_engine ||
            !hirari_audio_driver_host_select_device(m_driver, deviceId, sampleRate, bufferSize) ||
            !is_driver_running()) return false;
        applyNegotiatedDriverConfig();
        return std::abs(m_engine->get_sample_rate() -
                            hirari_audio_driver_host_sample_rate(m_driver)) < 0.5 &&
               m_engine->get_block_size() == hirari_audio_driver_host_buffer_size(m_driver);
    }
    uint64_t get_audio_config_generation() const {
        return m_engine ? m_engine->get_audio_config_generation() : 0;
    }
    uint32_t get_block_size() const { return m_engine ? m_engine->get_block_size() : 0; }

    void set_preview_sample(rust::Slice<const float> samples, double sourceRate) const {
        if (m_engine) m_engine->set_preview_sample(samples.data(), samples.size(), sourceRate);
    }
    /// Processes one bounded stereo block through the C++ graph. Rust must
    /// not maintain a second production audio graph.
    void process_audio_block(rust::Slice<float> left, rust::Slice<float> right) const noexcept {
        // The native graph is prepared for bounded realtime blocks. Reject
        // oversized FFI slices before narrowing to uint32_t; otherwise a
        // hostile or malformed caller could wrap the frame count and reach
        // the DSP with a different size than the slice actually contains.
        if (!m_engine || left.empty() || right.empty() || left.size() != right.size() ||
            left.size() > ::Hirari::Core::Engine::HirariUnifiedEngine::kMaxAudioBlockSize) {
            return;
        }
        // A host with no device to drive its callback dispatches the block
        // through the same boundary a real driver would use; a running device
        // already drives it, so the dispatcher answers false and the graph
        // renders here.
        if (hirari_audio_driver_host_dispatch_process(
                m_driver, left.data(), right.data(), static_cast<uint32_t>(left.size()))) {
            return;
        }
        float* channels[2] = {left.data(), right.data()};
        if (m_engine) m_engine->processBlockDirect(channels, 2, static_cast<uint32_t>(left.size()));
    }

    void apply_config(float tempo, uint32_t sr, uint32_t bs) const {
        std::lock_guard<std::mutex> lock(m_configMutex);
        (void)apply_config_locked(tempo, sr, bs);
    }

    // Configuration is a control-plane transaction.  Keeping the driver
    // transition and graph rebuild under the same session lock prevents two
    // UI/CLI callers from interleaving a device reconfigure with a second
    // prepareToPlay and publishing a generation that describes neither one.
    bool apply_config_locked(float tempo, uint32_t sr, uint32_t bs) const {
        // Keep the legacy void ABI, but make every entrypoint obey the same
        // configuration contract as try_apply_config. Invalid direct callers
        // must not quiesce the callback or advance the audio generation; the
        // bounds live in Rust (audio_config_rules.rs).
        if (!m_engine || !hirari_audio_config_valid(tempo, static_cast<double>(sr), bs)) return false;
        // A configuration transition is a stop-the-world control operation.
        // Do not silently stop a live transport here: callers must perform
        // the explicit transport transition first, and the check is made
        // while m_configMutex is held so a concurrent start cannot slip
        // between the UI preflight and driver reconfiguration.
        if (m_engine->is_playing()) return false;
        ::Hirari::Core::Engine::EngineConfig cfg;
        cfg.tempo = tempo; cfg.sampleRate = sr; cfg.blockSize = bs;
        // The graph and the device must be rebuilt as one transaction. Updating
        // only the graph leaves the device callback on the previous format.
        if (!hirari_native_test_isolation() && is_driver_running()) {
            if (!hirari_audio_driver_host_reconfigure(m_driver, static_cast<double>(sr), bs)) {
                return false;
            }
        } else {
            // A running device host must be rebuilt together with the graph so
            // the callback never keeps the previous format. An engine that owns
            // no running device (CLI renders, automation, isolated tests, a
            // platform with no backend) has nothing to reconfigure, and routing
            // it through the driver made every reconfigure fail and silently
            // leave the graph at its default 44100. Apply the config to the
            // detached graph directly, the branch the HIRARI_NATIVE_TEST_ISOLATION
            // sandbox already used.
            m_engine->waitForAudioCallbacks();
            m_engine->apply_config(cfg);
        }
        return true;
    }
    bool try_apply_config(float tempo, uint32_t sr, uint32_t bs) const {
        if (!m_engine || !hirari_audio_config_valid(tempo, static_cast<double>(sr), bs)) return false;
        std::lock_guard<std::mutex> lock(m_configMutex);
        const uint64_t before = m_engine->get_audio_config_generation();
        if (!apply_config_locked(tempo, sr, bs)) return false;
        // No separate device-reconciliation step: apply_config_locked already
        // rebuilds a running CoreAudio host together with the graph, and an
        // engine that owns no device has nothing left to reconcile. An earlier
        // guard here undid the just-applied config and returned false whenever
        // no driver was running, which left offline/CLI/test engines stuck at
        // their default 44100 and made them reject every project saved at a
        // different rate.
        return m_engine->get_audio_config_generation() != before &&
               std::abs(m_engine->get_sample_rate() - static_cast<double>(sr)) < 0.5 &&
               m_engine->get_block_size() == bs;
    }
