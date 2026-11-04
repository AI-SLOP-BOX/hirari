    // Audio-thread capture. Rust gates the write on the recorder mode and the
    // punch range, so the callback only publishes the current fader values.
    void recordCurrentState(uint64_t samplePosition) noexcept {
        const bool recorded = hirari_track_automation_store_record(
            m_automationStore, hirari_automation_recorder_instance(),
            m_id, samplePosition,
            m_volume.load(std::memory_order_relaxed),
            m_pan.load(std::memory_order_relaxed));
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
    AutomationList automationPoints(uint32_t kind) const {
        size_t count = 0;
        const auto* points = static_cast<const Hirari::Core::ProjectAutomationPoint*>(
            hirari_track_automation_store_points(m_automationStore, kind, &count));
        return points && count != 0 ? AutomationList(points, points + count) : AutomationList{};
    }
    AutomationList getVolumeAutomationPoints() const { return automationPoints(0); }
    AutomationList getPanAutomationPoints() const { return automationPoints(1); }
    AutomationList getTrackDelayAutomationPoints() const { return automationPoints(2); }
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
    // Replaces (or removes, for an empty list) the lane for one plugin
    // parameter; Rust owns the merge.
    void replacePluginAutomationLane(uint32_t pluginIndex, uint32_t parameterId,
                                     const AutomationList& points) {
        invalidateFrozenForEdit();
        hirari_track_automation_store_replace_plugin_lane(
            m_automationStore, pluginIndex, parameterId, points.data(), points.size());
        markProjectLayoutChanged();
    }
    PluginAutomationList getPluginAutomation() const {
        PluginAutomationList lanes;
        const size_t laneCount =
            hirari_track_automation_store_plugin_lane_count(m_automationStore);
        for (size_t index = 0; index < laneCount; ++index) {
            uint32_t pluginIndex = 0;
            uint32_t parameterId = 0;
            size_t pointCount = 0;
            const auto* points = static_cast<const Hirari::Core::ProjectAutomationPoint*>(
                hirari_track_automation_store_plugin_lane(
                    m_automationStore, index, &pluginIndex, &parameterId, &pointCount));
            lanes.push_back({pluginIndex, parameterId,
                             pointCount != 0 ? AutomationList(points, points + pointCount)
                                             : AutomationList{}});
        }
        return lanes;
    }
    bool canProcess(uint32_t sz) const { return m_activeBuffer.getNumSamples() >= sz; }

    AudioBuffer& getWorkBuffer(uint32_t) { return m_activeBuffer; }
    double getSampleRate() const noexcept { return m_sampleRate; }

private:
