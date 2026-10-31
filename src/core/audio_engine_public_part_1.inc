public:
    AudioEngine();
    explicit AudioEngine(bool startDevice);
    ~AudioEngine();

    void set_playing(bool playing) const {
        (void)try_set_playing(playing);
    }
    bool try_set_playing(bool playing) const {
        if (!m_engine) return false;
        if (playing && (!m_driver || !m_driver->is_running())) return false;
        m_engine->set_playing(playing);
        return true;
    }
    void set_loop(bool enabled) const { if (m_engine) { m_engine->set_loop(enabled); m_engine->mark_project_state_changed(); } }
    bool set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled) const { if (!m_engine || !m_engine->set_cycle_range(start_sample, end_sample, enabled)) return false; m_engine->mark_project_state_changed(); return true; }


    void set_metronome_enabled(bool enabled) const {
        if (m_engine) { m_engine->set_metronome_enabled(enabled); m_engine->mark_project_state_changed(); }
    }

    bool is_playing() const { return m_engine && m_engine->is_playing(); }
    void set_playhead(uint64_t pos) const { if (m_engine) m_engine->set_playhead(pos); }



    // Tempo and time-signature editing moved to Rust with the objects it
    // touches (hirari_core_engine_tempo.rs), which reach them through the
    // three handles above; nothing about it is relayed through here now. The
    // two entry points below still cross C++ on purpose: the native contract
    // tests drive exactly this path, and the driver config reads BPM through
    // it. Both land in the same Rust implementation.
    float get_tempo() const { return m_engine ? m_engine->get_tempo() : 120.0f; }
    bool set_tempo(float bpm) const { if (!m_engine || !m_engine->set_tempo(bpm)) return false; m_engine->mark_project_state_changed(); return true; }
    void midi_clock_tick(uint64_t timestamp) const { if (m_engine) m_engine->midi_clock_tick(timestamp); }



    void set_midi_clock_rate(double bpm) const { if (m_engine) m_engine->set_midi_clock_rate(bpm); }
    bool set_master_gain(float value) const {
        if (!m_engine || !m_engine->set_master_gain(value)) return false;
        m_engine->mark_project_state_changed(); return true;
    }
    uintptr_t control_room_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->control_room_state()) : 0;
    }
    // Addresses of the Rust-owned tempo map, undo history and diagnostic ring.
    // Rust reads them once at construction; from then on an edit reaches those
    // objects directly instead of coming back through these shims. A zero means
    // "no engine", which is the same branch these shims used to take themselves.
    uintptr_t tempo_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->tempo_state()) : 0;
    }
    uintptr_t undo_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->undo_state()) : 0;
    }
    uintptr_t log_state() const {
        return m_engine ? reinterpret_cast<uintptr_t>(m_engine->log_state()) : 0;
    }
    void mark_project_state_changed() const {
        if (m_engine) m_engine->mark_project_state_changed();
    }


    bool set_offline_render_target(uint32_t tid) const {
        return m_engine && m_engine->set_offline_render_target(tid);
    }
    void clear_offline_render_target() const noexcept {
        if (m_engine) m_engine->clear_offline_render_target();
    }
    void set_offline_render_tail_seconds(float seconds) const noexcept {
        if (m_engine) m_engine->set_offline_render_tail_seconds(seconds);
    }
    void set_offline_render_options(bool preFader, bool includeInserts) const noexcept {
        if (m_engine) m_engine->set_offline_render_options(preFader, includeInserts);
    }
    bool set_track_input_monitor(uint32_t tid, bool enabled) const {
        return m_engine && m_engine->set_track_input_monitor(tid, enabled);
    }
    bool is_audio_device_ready() const {
        std::lock_guard<std::mutex> lock(m_configMutex);
        const bool ready = m_driver && m_driver->is_running();
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
        return m_driver && m_driver->is_running();
    }
    bool is_silent_fallback() const noexcept {
#if defined(__APPLE__)
        if (!m_driver) return true;
        const char* status = m_driver->status();
        return status == nullptr || std::strcmp(status, "unavailable") == 0 ||
               std::strcmp(status, "start-failed") == 0;
#else
        return !m_driver || m_driver->isSilentFallback();
#endif
    }
    rust::String audio_driver_status() const {
        if (!m_driver) return rust::String("unavailable");
#if defined(__APPLE__)
        return rust::String(m_driver->status());
#else
        if (m_driver->isSilentFallback()) return rust::String("silent-fallback");
#if defined(HIRARI_ENABLE_JACK)
        return rust::String(m_driver->status());
#endif
#endif
        return rust::String("stopped");
    }
    int32_t audio_driver_error_code() const noexcept {
#if defined(__APPLE__)
        return m_driver ? m_driver->last_error_code() : 0;
#else
        return 0;
#endif
    }
    float get_audio_output_peak() const {
        return m_driver ? m_driver->output_peak() : 0.0f;
    }
    uint64_t get_audio_callback_count() const { return m_driver ? m_driver->callback_count() : 0; }
    uint64_t get_dropped_input_blocks() const { return m_driver ? m_driver->dropped_input_blocks() : 0; }
    void discard_pending_audio_input() const;
    uint32_t get_audio_input_channel_count() const { return m_driver ? m_driver->input_channel_count() : 0; }
    uint32_t get_audio_output_channel_count() const { return m_driver ? m_driver->output_channel_count() : 0; }
    rust::Vec<float> poll_audio_input(uint32_t& channelCount,
                                      uint64_t& droppedInputBlocks) const;
    void try_reconnect_audio_device() const {
#if defined(__APPLE__)
        if (hirari_native_test_isolation()) return;
#endif
        std::lock_guard<std::mutex> lock(m_configMutex);
        if (m_driver) {
            m_driver->try_reconnect();
            if (!m_driver->is_running() && m_engine) m_engine->set_playing(false);
        }
    }
    double get_sample_rate() const { return m_engine ? m_engine->get_sample_rate() : 44100.0; }
    rust::String list_audio_devices_json() const {
#if defined(__APPLE__)
        return rust::String(m_driver ? m_driver->list_devices_json() : "[]");
#else
        return rust::String("[]");
#endif
    }
    rust::String audio_input_device_uid() const {
#if defined(__APPLE__)
        return rust::String(m_driver ? m_driver->selected_device_uid() : std::string{});
#else
        return rust::String("");
#endif
    }
    rust::String list_midi_devices_json() const {
        return rust::String(::Hirari::Core::Driver::list_core_midi_devices_json());
    }
    bool send_midi_message(uint32_t uniqueId, rust::Slice<const uint8_t> data) const {
        return ::Hirari::Core::Driver::send_core_midi_message(uniqueId, data.data(), data.size());
    }
    bool start_midi_input() const { return ::Hirari::Core::Driver::start_core_midi_input(); }
    void stop_midi_input() const { ::Hirari::Core::Driver::stop_core_midi_input(); }
    rust::String poll_midi_input_json() const { return rust::String(::Hirari::Core::Driver::poll_core_midi_input_json()); }
    uint64_t midi_host_time_now() const { return ::Hirari::Core::Driver::core_midi_host_time_now(); }
    uint64_t midi_host_time_delta_samples(uint64_t start, uint64_t event, double sampleRate) const {
        return ::Hirari::Core::Driver::core_midi_host_time_delta_samples(start, event, sampleRate);
    }
    uint64_t midi_dropped_input_events() const {
        return ::Hirari::Core::Driver::core_midi_dropped_input_events();
    }
    bool select_audio_device(uint32_t deviceId, double sampleRate, uint32_t bufferSize) const {
#if defined(__APPLE__) || defined(HIRARI_ENABLE_JACK)
        if (!m_driver || !m_engine ||
            !m_driver->select_device(deviceId, sampleRate, bufferSize) ||
            !m_driver->is_running()) return false;
        applyNegotiatedDriverConfig();
        return std::abs(m_engine->get_sample_rate() - m_driver->sample_rate()) < 0.5 &&
               m_engine->get_block_size() == m_driver->buffer_size();
#else
        (void)deviceId; (void)sampleRate; (void)bufferSize; return false;
#endif
    }
    uint64_t get_audio_config_generation() const {
        return m_engine ? m_engine->get_audio_config_generation() : 0;
    }
    uint32_t get_block_size() const { return m_engine ? m_engine->get_block_size() : 0; }

    void set_test_tone(bool enabled) const { if (m_engine) m_engine->set_test_tone(enabled); }
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
#if !defined(__APPLE__) && !defined(HIRARI_ENABLE_JACK)
        // Keep the non-hardware fallback on the same callback boundary as a
        // real driver. The dispatcher invokes process_driver_block, which
        // owns the single native graph render; do not render it a second time
        // below.
        if (m_driver && m_driver->dispatch_process_callback(
                left.data(), right.data(), static_cast<uint32_t>(left.size()))) return;
#endif
        float* channels[2] = {left.data(), right.data()};
        if (m_engine) m_engine->processBlockDirect(channels, 2, static_cast<uint32_t>(left.size()));
    }

    bool set_region_warp_markers(uint32_t tid, uint32_t rid,
                                 rust::Slice<const uint64_t> source_samples,
                                 rust::Slice<const uint64_t> timeline_samples,
                                 rust::Slice<const uint8_t> transient_flags) const {
        // Slice agreement, the marker cap and the conversion to the marker
        // layout are Rust-owned (hirari_region_edit_set_warp_markers).
        return m_engine && hirari_region_edit_set_warp_markers(
            m_engine.get(), tid, rid,
            source_samples.data(), source_samples.size(),
            timeline_samples.data(), timeline_samples.size(),
            transient_flags.data(), transient_flags.size());
    }

    void trigger_preview_sample() const { if (m_engine) m_engine->trigger_preview_sample(); }
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
#if defined(__APPLE__)
        // The graph and CoreAudio must be rebuilt as one transaction. Updating
        // only the graph leaves the device callback on the previous format.
        if (!hirari_native_test_isolation() && m_driver && m_driver->is_running()) {
            if (!m_driver->reconfigure(static_cast<double>(sr), bs)) {
                return false;
            }
        } else {
            // A running CoreAudio host must be rebuilt as one transaction so
            // the device callback never keeps the previous format. An engine
            // that owns no device (CLI renders, automation, isolated tests)
            // has nothing to reconfigure, and routing it through the driver
            // made every reconfigure fail and silently leave the graph at its
            // default 44100. Apply the config to the detached graph directly,
            // the branch the HIRARI_NATIVE_TEST_ISOLATION sandbox already used.
            m_engine->waitForAudioCallbacks();
            m_engine->apply_config(cfg);
        }
#else
        // Device/sample-rate/block-size changes are control-thread events.
        // Quiesce any callback that still owns the previous audio context
        // before rebuilding track processors and PDC state.
#if defined(HIRARI_ENABLE_JACK)
        if (m_driver && m_driver->is_running() &&
            !m_driver->reconfigure(static_cast<double>(sr), bs)) {
            return false;
        }
#endif
        m_engine->waitForAudioCallbacks();
        m_engine->apply_config(cfg);
#endif
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
