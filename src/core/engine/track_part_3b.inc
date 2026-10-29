    void recordCurrentState(uint64_t samplePosition) noexcept {
        const bool recorded = hirari_track_automation_capture_record(
            m_trackAutomationCaptureState,
            hirari_automation_recorder_instance(),
            m_id, samplePosition,
            m_volume.load(std::memory_order_relaxed),
            m_pan.load(std::memory_order_relaxed));
        if (recorded && m_layoutRevision) {
            m_layoutRevision->realtimeAutomationDirty.store(true, std::memory_order_release);
        }
    }

    // Control-thread consumer. It publishes immutable automation lists only
    // after draining the callback queue, keeping serialization and UI reads
    // consistent with what was actually recorded.
    uint32_t flushRecordedAutomation() {
        size_t drained = 0;
        auto* captured = hirari_track_automation_capture_drain(
            m_trackAutomationCaptureState, &drained);
        if (!captured) return 0;
        auto volume = getVolumeAutomationPoints();
        auto pan = getPanAutomationPoints();
        for (size_t index = 0; index < drained; ++index) {
            volume.push_back({captured[index].time, captured[index].volume, 0.0f});
            pan.push_back({captured[index].time, captured[index].pan, 0.0f});
        }
        hirari_track_automation_capture_points_free(captured, drained);
        setVolumeAutomation(volume);
        setPanAutomation(pan);
        return static_cast<uint32_t>(std::min<size_t>(drained, UINT32_MAX));
    }
    void setVolumeAutomation(const std::vector<Hirari::Core::ProjectAutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_volumeAutomationHistory.push_back(std::move(copy));
        m_volumeAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    void setPanAutomation(const std::vector<Hirari::Core::ProjectAutomationPoint>& points) {
        // Pan automation is stored in the live -1..1 fader domain.
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_panAutomationHistory.push_back(std::move(copy));
        m_panAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    void setTrackDelayAutomation(const std::vector<Hirari::Core::ProjectAutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_trackDelayAutomationHistory.push_back(std::move(copy));
        m_trackDelayAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    std::vector<Hirari::Core::ProjectAutomationPoint> getVolumeAutomationPoints() const {
        const auto* points = m_volumeAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Hirari::Core::ProjectAutomationPoint>{};
    }
    std::vector<Hirari::Core::ProjectAutomationPoint> getPanAutomationPoints() const {
        const auto* points = m_panAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Hirari::Core::ProjectAutomationPoint>{};
    }
    std::vector<Hirari::Core::ProjectAutomationPoint> getTrackDelayAutomationPoints() const {
        const auto* points = m_trackDelayAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Hirari::Core::ProjectAutomationPoint>{};
    }
    void setPluginAutomation(const PluginAutomationList& lanes) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<PluginAutomationSnapshot>();
        copy->lanes = lanes;
        copy->views.reserve(copy->lanes.size());
        for (const auto& lane : copy->lanes) {
            copy->views.push_back({
                lane.pluginIndex, lane.parameterId,
                lane.points.data(), lane.points.size()});
        }
        const auto* rawPtr = copy.get();
        m_pluginAutomationHistory.push_back(std::move(copy));
        m_pluginAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    PluginAutomationList getPluginAutomation() const {
        const auto* lanes = m_pluginAutomation.load(std::memory_order_acquire);
        return lanes ? lanes->lanes : PluginAutomationList{};
    }
    bool canProcess(uint32_t sz) const { return m_activeBuffer.getNumSamples() >= sz; }

    AudioBuffer& getWorkBuffer(uint32_t) { return m_activeBuffer; }
    double getSampleRate() const noexcept { return m_sampleRate; }
    struct AnalysisEngine {
        explicit AnalysisEngine(const AudioBuffer* buffer) : m_buffer(buffer) {}

        // Range splitting, the rms/peak mix and the clamp are Rust-owned; this
        // wrapper only supplies the first channel and sizes the result.
        std::vector<float> getSpectrogramL() const {
            constexpr uint32_t kMaxBins = 32;
            if (m_buffer == nullptr || m_buffer->getNumChannels() == 0 ||
                m_buffer->getNumSamples() == 0) {
                return {};
            }
            const uint32_t samples = m_buffer->getNumSamples();
            std::vector<float> features(std::min(kMaxBins, samples), 0.0f);
            const size_t written = hirari_track_analysis_spectrogram(
                m_buffer->getReadPointer(0), samples, features.data(), features.size());
            features.resize(written);
            return features;
        }

    private:
        const AudioBuffer* m_buffer;
    };
    const AnalysisEngine& getAnalysisEngine() const { return m_analysis; }

private:
