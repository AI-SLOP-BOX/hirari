    using RegionSnapshot = std::vector<Region>;

    void publishRegionSnapshot() {
        auto snapshot = std::make_unique<RegionSnapshot>(m_regions);
        const auto timelineEnd = [](const Region& candidate) -> uint64_t {
            if (candidate.loopCount == 0 || candidate.len > UINT64_MAX / candidate.loopCount) {
                return UINT64_MAX;
            }
            const uint64_t length = candidate.len * candidate.loopCount;
            return candidate.start > UINT64_MAX - length
                ? UINT64_MAX
                : candidate.start + length;
        };
        for (size_t index = 0; index < snapshot->size(); ++index) {
            auto& region = (*snapshot)[index];
            const uint64_t regionEnd = timelineEnd(region);
            for (size_t otherIndex = 0; otherIndex < snapshot->size(); ++otherIndex) {
                if (index == otherIndex) continue;
                const auto& other = (*snapshot)[otherIndex];
                if (other.muted || other.len == 0) continue;
                const uint64_t otherEnd = timelineEnd(other);
                if (other.start < region.start && otherEnd > region.start) {
                    region.crossfadeInSamples = std::max(
                        region.crossfadeInSamples,
                        std::min<uint64_t>(otherEnd - region.start, 8192));
                }
                if (other.start > region.start && other.start < regionEnd) {
                    region.crossfadeOutSamples = std::max(
                        region.crossfadeOutSamples,
                        std::min<uint64_t>(regionEnd - other.start, 8192));
                }
            }
        }
        const RegionSnapshot* published = snapshot.get();
        m_regionSnapshots.push_back(std::move(snapshot));
        m_regionGeneration.fetch_add(1, std::memory_order_release);
        m_regionSnapshot.store(published, std::memory_order_release);
    }

    using AutomationList = std::vector<Aura::Core::ProjectSerializer::AutomationPoint>;
    static float evaluateAutomation(const AutomationList& points, double time, size_t& lastIndexHint) {
        if (points.empty()) return 0.0f;
        if (points.size() == 1) return points.front().value;
        if (time <= points.front().time) {
            lastIndexHint = 0;
            return points.front().value;
        }
        if (time >= points.back().time) {
            lastIndexHint = points.size() - 1;
            return points.back().value;
        }

        size_t idx = lastIndexHint;
        if (idx >= points.size() - 1) {
            idx = 0;
        }

        if (points[idx].time <= time && time < points[idx + 1].time) {
            // Fast path: target is within the same segment
        } else if (idx + 2 < points.size() && points[idx + 1].time <= time && time < points[idx + 2].time) {
            // Next segment step
            idx = idx + 1;
            lastIndexHint = idx;
        } else {
            // Backward seek or jump seek: fall back to binary search
            auto it = std::upper_bound(points.begin(), points.end(), time,
                [](double t, const auto& point) { return t < point.time; });
            idx = std::distance(points.begin(), it) - 1;
            lastIndexHint = idx;
        }

        const auto& a = points[idx];
        const auto& b = points[idx + 1];
        const double range = b.time - a.time;
        if (range <= 1e-9) return a.value;
        const float amount = std::clamp(static_cast<float>((time - a.time) / range), 0.0f, 1.0f);
        // Apply the segment's bounded curve without allocation or pow/exp on
        // the audio thread. The midpoint remains stable while the shape
        // changes the acceleration through the segment.
        const float curve = std::clamp(a.curve, -1.0f, 1.0f);
        const float shaped = std::clamp(
            amount + curve * amount * (1.0f - amount) * (1.0f - 2.0f), 0.0f, 1.0f);
        return a.value + (b.value - a.value) * shaped;
    }
    uint32_t m_id;
    std::string m_name;
    Type m_type;
    DSP::Mixing::ChannelStrip m_strip;
    Aura::Core::EffectChain m_effectChain;
    MidiBuffer m_pluginMidi;
    double m_sampleRate = 44100.0;
    std::atomic<double> m_transportBpm{120.0};
    std::atomic<bool> m_transportPlaying{false};
    uint32_t m_blockSize = 0;
    bool m_isPrepared = false;
    std::atomic<float> m_volume{1.0f};
    std::atomic<float> m_pan{0.0f};
    struct RecordedAutomationPoint {
        double time = 0.0;
        float volume = 1.0f;
        float pan = 0.0f;
    };
    static constexpr uint32_t kMaxRecordedAutomationPoints = 16384;
    std::array<RecordedAutomationPoint, kMaxRecordedAutomationPoints> m_recordedAutomation{};
    std::atomic<uint64_t> m_recordedAutomationWrite{0};
    std::atomic<uint64_t> m_recordedAutomationRead{0};
    std::atomic<uint32_t> m_recordedAutomationDrops{0};
    std::atomic<bool> m_muted{false};
    std::atomic<bool> m_phaseInvert{false};
    std::atomic<bool> m_solo{false};
    std::atomic<bool> m_recordArmed{false};
    std::atomic<bool> m_isEcoMode{false};
    std::vector<Region> m_regions;
    mutable std::mutex m_regionEditMutex;
    // Published only by the control thread. Old generations intentionally
    // remain alive until Track destruction, avoiding RT-side shared_ptr release.
    std::vector<std::unique_ptr<RegionSnapshot>> m_regionSnapshots;
    std::atomic<const RegionSnapshot*> m_regionSnapshot{nullptr};
    std::atomic<uint64_t> m_regionGeneration{0};
    std::vector<uint32_t> m_pluginTypes;
    std::vector<std::shared_ptr<Aura::Core::MidiRegion>> m_midiRegions;
    std::vector<std::string> m_sandboxedPluginPaths;
    std::vector<std::vector<uint8_t>> m_pluginGuiStates;
    std::vector<uint32_t> m_sandboxedPluginIndices;
    std::atomic<uint8_t> m_lastSandboxFailure{0};
    std::string m_lastSandboxFailureText = "none";
    RegionProcessor m_regionProcessor;
    AudioBuffer m_activeBuffer;
    AudioBuffer m_preFaderBuffer;
    AudioBuffer m_preInsertBuffer;
    // Control-thread-owned retention prevents an in-flight audio callback
    // from observing freed freeze memory. The callback only loads the raw
    // published pointer and never changes shared_ptr reference counts.
    std::vector<std::shared_ptr<AudioBuffer>> m_frozenBuffers;
    mutable std::mutex m_freezeEditMutex;
    std::atomic<const AudioBuffer*> m_frozenAudio{nullptr};
    std::atomic<bool> m_frozen{false};
    std::atomic<uint64_t> m_frozenTotalSamples{0};
    std::atomic<uint32_t> m_frozenSampleRate{0};
    std::atomic<uint64_t> m_frozenProjectGeneration{0};
    std::atomic<uint64_t> m_frozenAudioGeneration{0};
    std::string m_frozenPath;
    AnalysisEngine m_analysis;
    double m_lastAutomationTime = 0.0;
    float m_lowGain = 1.0f;
    float m_highGain = 1.0f;
    float m_lowStateL = 0.0f;
    float m_lowStateR = 0.0f;
    std::vector<std::unique_ptr<AutomationList>> m_volumeAutomationHistory;
    std::atomic<const AutomationList*> m_volumeAutomation{nullptr};
    size_t m_volumeAutomationHint = 0;
    std::vector<std::unique_ptr<AutomationList>> m_panAutomationHistory;
    std::atomic<const AutomationList*> m_panAutomation{nullptr};
    size_t m_panAutomationHint = 0;
    std::vector<std::unique_ptr<AutomationList>> m_trackDelayAutomationHistory;
    std::atomic<const AutomationList*> m_trackDelayAutomation{nullptr};
    size_t m_trackDelayAutomationHint = 0;
    std::atomic<float> m_spatialX{0.0f}, m_spatialY{0.0f}, m_spatialZ{0.0f};
    std::atomic<uint32_t> m_spatialMode{0};
    ::Aura::DSP::Spatial::HolographicPanner m_spatialPanner;
    static constexpr uint32_t kMaxPdcDelaySamples = 8192;
    std::atomic<uint32_t> m_pdcRequested{0};
    std::atomic<uint32_t> m_trackDelaySamples{0};
    uint32_t m_pdcActive = 0;
    uint32_t m_pdcPrevious = 0;
    uint32_t m_pdcTransitionRemaining = 0;
    uint32_t m_pdcWrite = 0;
    std::array<float, kMaxPdcDelaySamples + 1> m_pdcDelayL{};
    std::array<float, kMaxPdcDelaySamples + 1> m_pdcDelayR{};
};
