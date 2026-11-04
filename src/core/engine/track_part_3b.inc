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
    void setAutomationPoints(uint32_t kind, const AutomationList& points) {
        static_assert(sizeof(Hirari::Core::ProjectAutomationPoint) == 16);
        invalidateFrozenForEdit();
        hirari_track_automation_store_set_points(
            m_automationStore, kind, points.data(), points.size());
        markProjectLayoutChanged();
    }
    void setVolumeAutomation(const AutomationList& points) { setAutomationPoints(0, points); }
    void setPanAutomation(const AutomationList& points) { setAutomationPoints(1, points); }
    void setTrackDelayAutomation(const AutomationList& points) { setAutomationPoints(2, points); }
    void setPluginAutomation(const PluginAutomationList& lanes) {
        invalidateFrozenForEdit();
        std::vector<HirariPluginAutomationLaneView> views;
        views.reserve(lanes.size());
        for (const auto& lane : lanes) {
            views.push_back({lane.pluginIndex, lane.parameterId,
                             lane.points.data(), lane.points.size()});
        }
        hirari_track_automation_store_set_plugin_lanes(
            m_automationStore, views.data(), views.size());
        markProjectLayoutChanged();
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
