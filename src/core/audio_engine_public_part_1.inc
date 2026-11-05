public:
    AudioEngine();
    explicit AudioEngine(bool startDevice);
    ~AudioEngine();

    // Transport start/stop is Rust-owned (`engine_transport.rs`); the facade
    // keeps no relay, only the engine-side safety stops below.
    bool transport_playing() const {
        return m_engine &&
               hirari_engine_block_is_playing(
                   reinterpret_cast<const void*>(m_engine->engine_block_state())) != 0;
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
    uintptr_t engine_block_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->engine_block_state()) : 0;
    }
    bool is_audio_device_ready() const {
        std::lock_guard<std::mutex> lock(m_configMutex);
        const bool ready = is_driver_running();
        if (!ready && m_engine && transport_playing()) {
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
    // The configuration transaction is Rust-owned (`engine_config.rs`). Device
    // selection, reconfiguration and the graph rebuild all serialize through
    // this session lock, so Rust takes and releases it around one transaction.
    void lock_config() const { m_configMutex.lock(); }
    void unlock_config() const { m_configMutex.unlock(); }
    uint64_t get_audio_config_generation() const {
        return m_engine ? hirari_engine_block_generation(
                             reinterpret_cast<const void*>(m_engine->engine_block_state()))
                        : 0;
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

    // The whole configuration transaction — validity gate, transport guard,
    // driver reconfigure and graph rebuild under the session lock — is
    // Rust-owned (`engine_config.rs`), which drives the driver host, the tempo
    // map and prepareToPlay through their own entry points. No EngineConfig
    // struct and no apply/try pair survives on this side.
