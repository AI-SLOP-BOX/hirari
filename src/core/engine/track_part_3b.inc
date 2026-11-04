    // Audio-thread capture. Rust gates the write on the recorder mode and the
    // punch range, so the callback only publishes the current fader values.
    void recordCurrentState(uint64_t samplePosition) noexcept {
        const bool recorded = hirari_track_automation_store_record(
            m_automationStore, hirari_automation_recorder_instance(),
            m_id, samplePosition,
            hirari_channel_strip_gain(m_strip), hirari_channel_strip_pan(m_strip));
        if (recorded && m_layoutRevision) {
            m_layoutRevision->realtimeAutomationDirty.store(true, std::memory_order_release);
        }
    }

    // Control-thread consumer. Rust drains the callback queue and appends the
    // captured points to the published volume/pan lists.
    uint32_t flushRecordedAutomation() {
        const uint32_t drained =
            hirari_track_automation_store_flush_recorded(m_automationStore);
        if (drained != 0) markProjectLayoutChanged();
        return drained;
    }
    bool canProcess(uint32_t sz) const {
        return hirari_audio_buffer_storage_samples(m_activeBuffer) >= sz;
    }

    /// The Rust runtime this track renders through; the engine's block tail
    /// addresses the track's taps through it.
    void* realtimeRuntime() const noexcept { return m_runtime; }

    /// The other Rust storages the track owns. A Rust control walk reads the
    /// track model through these handles instead of a per-field C++ forwarder.
    void* channelStripHandle() const noexcept { return m_strip; }
    void* effectChainHandle() const noexcept { return m_chain; }
    void* automationStoreHandle() const noexcept { return m_automationStore; }
    void* frozenStoreHandle() const noexcept { return m_frozenStore; }
    void* nameStoreHandle() const noexcept { return m_nameStore; }

    /// The track's work block. The engine reads and writes its channels
    /// through the storage FFI rather than a C++ buffer type.
    void* workBufferStorage() noexcept { return m_activeBuffer; }

private:
