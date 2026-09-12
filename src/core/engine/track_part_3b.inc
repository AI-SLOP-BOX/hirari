    void recordCurrentState(uint64_t samplePosition) noexcept {
        const uint64_t write = m_recordedAutomationWrite.load(std::memory_order_relaxed);
        const uint64_t read = m_recordedAutomationRead.load(std::memory_order_acquire);
        if (write - read >= kMaxRecordedAutomationPoints) {
            m_recordedAutomationDrops.fetch_add(1, std::memory_order_relaxed);
            return;
        }
        auto& point = m_recordedAutomation[(write % kMaxRecordedAutomationPoints)];
        point.time = static_cast<double>(samplePosition);
        point.volume = m_volume.load(std::memory_order_relaxed);
        point.pan = m_pan.load(std::memory_order_relaxed);
        auto& recorder = AutomationRecorder::getInstance();
        recorder.recordValue(m_id, 0u, point.volume, samplePosition);
        recorder.recordValue(m_id, 1u, point.pan, samplePosition);
        m_recordedAutomationWrite.store(write + 1, std::memory_order_release);
    }

    // Control-thread consumer. It publishes immutable automation lists only
    // after draining the callback queue, keeping serialization and UI reads
    // consistent with what was actually recorded.
    uint32_t flushRecordedAutomation() {
        const uint64_t write = m_recordedAutomationWrite.load(std::memory_order_acquire);
        uint64_t read = m_recordedAutomationRead.load(std::memory_order_relaxed);
        if (read == write) return 0;
        const uint64_t drained = write - read;
        auto volume = getVolumeAutomationPoints();
        auto pan = getPanAutomationPoints();
        while (read < write) {
            const auto& point = m_recordedAutomation[read % kMaxRecordedAutomationPoints];
            volume.push_back({point.time, point.volume, 0.0f});
            pan.push_back({point.time, point.pan, 0.0f});
            ++read;
        }
        m_recordedAutomationRead.store(read, std::memory_order_release);
        setVolumeAutomation(volume);
        setPanAutomation(pan);
        return static_cast<uint32_t>(std::min<uint64_t>(drained, UINT32_MAX));
    }
    void setVolumeAutomation(const std::vector<Aura::Core::ProjectSerializer::AutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_volumeAutomationHistory.push_back(std::move(copy));
        m_volumeAutomation.store(rawPtr, std::memory_order_release);
    }
    void setPanAutomation(const std::vector<Aura::Core::ProjectSerializer::AutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_panAutomationHistory.push_back(std::move(copy));
        m_panAutomation.store(rawPtr, std::memory_order_release);
    }
    void setTrackDelayAutomation(const std::vector<Aura::Core::ProjectSerializer::AutomationPoint>& points) {
        invalidateFrozenForEdit();
        auto copy = std::make_unique<AutomationList>(points);
        std::sort(copy->begin(), copy->end(), [](const auto& a, const auto& b) { return a.time < b.time; });
        const auto* rawPtr = copy.get();
        m_trackDelayAutomationHistory.push_back(std::move(copy));
        m_trackDelayAutomation.store(rawPtr, std::memory_order_release);
    }
    std::vector<Aura::Core::ProjectSerializer::AutomationPoint> getVolumeAutomationPoints() const {
        const auto* points = m_volumeAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Aura::Core::ProjectSerializer::AutomationPoint>{};
    }
    std::vector<Aura::Core::ProjectSerializer::AutomationPoint> getPanAutomationPoints() const {
        const auto* points = m_panAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Aura::Core::ProjectSerializer::AutomationPoint>{};
    }
    std::vector<Aura::Core::ProjectSerializer::AutomationPoint> getTrackDelayAutomationPoints() const {
        const auto* points = m_trackDelayAutomation.load(std::memory_order_acquire);
        return points ? *points : std::vector<Aura::Core::ProjectSerializer::AutomationPoint>{};
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
