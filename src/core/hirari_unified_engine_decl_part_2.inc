    // Transport
    // The playhead, loop, cycle and test-tone transport answers come straight
    // from the Rust engine-block store (engine_transport.rs); only the
    // device-callback join and the stop-time path stay here.
    bool is_playing() const;
    void join_audio_callbacks() const noexcept { waitForAudioCallbacks(); }
    bool audio_range_overflowed() const noexcept {
        return hirari_engine_block_range_overflowed(m_engineBlock) != 0;
    }
    uint32_t get_block_size() const noexcept;
    float get_latency_ms() const noexcept;
    float get_track_latency_ms(uint32_t tid) const noexcept;
    float get_track_pdc_compensation_ms(uint32_t tid) const noexcept;
    void set_playing(bool playing);
    // Tempo editing moved to Rust; this write survives for the native contract
    // test that drives the engine singleton directly.
    bool set_tempo(float bpm) const;
    void midi_clock_tick(uint64_t timestamp) const noexcept { hirari_external_sync_tick(m_externalSync, timestamp); }
    uint64_t midi_clock_ticks() const noexcept { return hirari_external_sync_ticks(m_externalSync); }
    uint64_t midi_clock_last_tick() const noexcept { return hirari_external_sync_last_tick(m_externalSync); }
    double midi_clock_rate() const noexcept { return hirari_external_sync_rate(m_externalSync); }
    void set_midi_clock_rate(double bpm) const noexcept { hirari_external_sync_set_rate(m_externalSync, bpm); }
    bool set_master_gain(float value);
    void* control_room_state() const noexcept { return m_controlRoomState; }
    // Rust owns the tempo map, the undo history and the diagnostic ring; these
    // hand those addresses to Rust callers so an edit can work on the very
    // objects this engine's render path and history read, instead of crossing
    // into C++ only to be sent straight back.
    void* tempo_state() const noexcept { return m_tempoState; }
    void* undo_state() const noexcept { return m_undoState; }
    void* log_state() const noexcept {
        return hirari_log_state();
    }
    size_t midi_snapshot_store() const noexcept { return reinterpret_cast<size_t>(m_midiSnapshotStore); }
    size_t midi_learn_state() const noexcept { return reinterpret_cast<size_t>(m_midiLearnState); }
    size_t preview_synth_state() const noexcept { return reinterpret_cast<size_t>(m_previewSynthState); }
    size_t pdc_state() const noexcept { return reinterpret_cast<size_t>(m_pdcState); }
    size_t preview_sampler() const noexcept { return reinterpret_cast<size_t>(m_previewSampler); }
    size_t metronome_state() const noexcept { return reinterpret_cast<size_t>(m_metronome); }
    size_t routing_engine_state() const noexcept { return reinterpret_cast<size_t>(m_routingEngine); }
    bool bus_track_exists(uint32_t busTrackId) const {
        if (busTrackId == 0) return false;
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        Track* track = findTrackLocked(busTrackId);
        return track && dynamic_cast<BusTrack*>(track) != nullptr;
    }
    float get_master_gain() const noexcept;
    uint32_t get_track_delay_samples(uint32_t tid) const;
    double get_sample_rate() const { return m_sampleRate.load(std::memory_order_relaxed); }
    uint64_t get_audio_config_generation() const noexcept {
        return hirari_engine_block_generation(m_engineBlock);
    }
    // Rust owns the playhead, loop, cycle and test tone in the engine-block
    // store (engine_transport.rs); the deleted accessors were one-line
    // forwarders onto hirari_engine_block_* that Rust now calls directly.
    // The preview-sampler quiescence protocol is Rust-side too: the delegate
    // calls the sampler store externs, so no engine forwarder is left.
