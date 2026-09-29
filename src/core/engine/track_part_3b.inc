    void recordCurrentState(uint64_t samplePosition) noexcept {
        const bool recorded = hirari_track_automation_capture_record(
            m_trackAutomationCaptureState,
            AutomationRecorder::getInstance().stateForTrackCapture(),
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
        size_t capacity = hirari_track_automation_capture_flush(
            m_trackAutomationCaptureState, nullptr, 0);
        if (capacity == 0) return 0;
        std::vector<HirariTrackAutomationCapturePoint> captured;
        size_t drained = 0;
        for (;;) {
            captured.resize(capacity);
            drained = hirari_track_automation_capture_flush(
                m_trackAutomationCaptureState, captured.data(), captured.size());
            if (drained <= captured.size()) break;
            capacity = drained;
        }
        captured.resize(drained);
        auto volume = getVolumeAutomationPoints();
        auto pan = getPanAutomationPoints();
        for (const auto& point : captured) {
            volume.push_back({point.time, point.volume, 0.0f});
            pan.push_back({point.time, point.pan, 0.0f});
        }
        setVolumeAutomation(volume);
        setPanAutomation(pan);
        return static_cast<uint32_t>(std::min<size_t>(drained, UINT32_MAX));
    }
    void setVolumeAutomation(const std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_volumeAutomationHistory.push_back(std::move(copy));
        m_volumeAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    void setPanAutomation(const std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>& points) {
        // Pan automation is stored in the live -1..1 fader domain.
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_panAutomationHistory.push_back(std::move(copy));
        m_panAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    void setTrackDelayAutomation(const std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_trackDelayAutomationHistory.push_back(std::move(copy));
        m_trackDelayAutomation.store(rawPtr, std::memory_order_release);
        markProjectLayoutChanged();
    }
    std::vector<Hirari::Core::ProjectSerializer::AutomationPoint> getVolumeAutomationPoints() const {
        const auto* points = m_volumeAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>{};
    }
    std::vector<Hirari::Core::ProjectSerializer::AutomationPoint> getPanAutomationPoints() const {
        const auto* points = m_panAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>{};
    }
    std::vector<Hirari::Core::ProjectSerializer::AutomationPoint> getTrackDelayAutomationPoints() const {
        const auto* points = m_trackDelayAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>{};
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

        std::vector<float> getSpectrogramL() const {
            constexpr uint32_t kMaxBins = 32;
            if (m_buffer == nullptr || m_buffer->getNumChannels() == 0 ||
                m_buffer->getNumSamples() == 0) {
                return {};
            }

            const uint32_t samples = m_buffer->getNumSamples();
            const uint32_t bins = std::min(kMaxBins, samples);
            std::vector<float> features(bins, 0.0f);
            const float* left = m_buffer->getReadPointer(0);
            for (uint32_t bin = 0; bin < bins; ++bin) {
                const uint32_t begin = (bin * samples) / bins;
                const uint32_t end = ((bin + 1) * samples) / bins;
                const uint32_t count = end > begin ? end - begin : 1;
                double energy = 0.0;
                float peak = 0.0f;
                for (uint32_t i = begin; i < end; ++i) {
                    const float sample = std::isfinite(left[i]) ? left[i] : 0.0f;
                    energy += static_cast<double>(sample) * sample;
                    peak = std::max(peak, std::abs(sample));
                }
                const float rms = static_cast<float>(std::sqrt(energy / count));
                features[bin] = std::isfinite(rms) && std::isfinite(peak)
                    ? std::clamp(0.5f * (rms + peak), 0.0f, 1.0f)
                    : 0.0f;
            }
            return features;
        }

    private:
        const AudioBuffer* m_buffer;
    };
    const AnalysisEngine& getAnalysisEngine() const { return m_analysis; }

private:
