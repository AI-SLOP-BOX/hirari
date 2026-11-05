    // Transport
    // The playhead, loop, cycle and test-tone transport answers come straight
    // from the Rust engine-block store (engine_transport.rs). The device
    // format, the master fader, the record-arm gate and the in-flight callback
    // count are its state as well, so latency and block-size reads are Rust's
    // (engine_analysis_reads.rs) and only the stop-time path stays here.
    void join_audio_callbacks() const noexcept { hirari_engine_block_join_callbacks(handle(kHirariHandleEngineBlock)); }
    void set_playing(bool playing);
    // Tempo editing moved to Rust; this write survives for the native contract
    // test that drives the engine singleton directly.
    bool set_tempo(float bpm) const;
    void midi_clock_tick(uint64_t timestamp) const noexcept { hirari_external_sync_tick(handle(kHirariHandleExternalSync), timestamp); }
    uint64_t midi_clock_ticks() const noexcept { return hirari_external_sync_ticks(handle(kHirariHandleExternalSync)); }
    uint64_t midi_clock_last_tick() const noexcept { return hirari_external_sync_last_tick(handle(kHirariHandleExternalSync)); }
    double midi_clock_rate() const noexcept { return hirari_external_sync_rate(handle(kHirariHandleExternalSync)); }
    void set_midi_clock_rate(double bpm) const noexcept { hirari_external_sync_set_rate(handle(kHirariHandleExternalSync), bpm); }
    bool set_master_gain(float value);
    void* control_room_state() const noexcept { return handle(kHirariHandleControlRoom); }
    // The driver callback stages its hardware buffers straight into this
    // Rust-owned input store, so the facade reads the handle rather than
    // forwarding a publish per path.
    void* input_blocks() const noexcept { return handle(kHirariHandleInputBlocks); }
    // Rust owns the tempo map, the undo history and the diagnostic ring; these
    // hand those addresses to Rust callers so an edit can work on the very
    // objects this engine's render path and history read, instead of crossing
    // into C++ only to be sent straight back.
    void* tempo_state() const noexcept { return handle(kHirariHandleTempo); }
    void* undo_state() const noexcept { return handle(kHirariHandleUndo); }
    void* log_state() const noexcept {
        return hirari_log_state();
    }
    size_t midi_snapshot_store() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandleMidiSnapshotStore)); }
    size_t midi_learn_state() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandleMidiLearn)); }
    size_t preview_synth_state() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandlePreviewSynth)); }
    size_t pdc_state() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandlePdc)); }
    size_t preview_sampler() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandlePreviewSampler)); }
    size_t metronome_state() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandleMetronome)); }
    size_t routing_engine_state() const noexcept { return reinterpret_cast<size_t>(handle(kHirariHandleRouting)); }
    bool bus_track_exists(uint32_t busTrackId) const {
        if (busTrackId == 0) return false;
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        void* track = findTrackLocked(busTrackId);
        return track && hirari_track_type(track) == kHirariTrackKindBus;
    }
    float get_master_gain() const noexcept;
    double get_sample_rate() const { return hirari_engine_block_device_sample_rate(handle(kHirariHandleEngineBlock)); }
    // Rust owns the playhead, loop, cycle and test tone in the engine-block
    // store (engine_transport.rs); the deleted accessors were one-line
    // forwarders onto hirari_engine_block_* that Rust now calls directly.
    // The preview-sampler quiescence protocol is Rust-side too: the delegate
    // calls the sampler store externs, so no engine forwarder is left.
