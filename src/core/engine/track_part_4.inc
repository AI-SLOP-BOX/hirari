    struct RegionSnapshot : std::vector<Region> {
        explicit RegionSnapshot(const std::vector<Region>& source)
            : std::vector<Region>(source) {}
        std::vector<HirariTrackRegionRenderInput> renderInputs;
    };

    void publishRegionSnapshot() {
        // Streaming processors are mutable. Give each published snapshot its
        // own instances so an older callback/render can never advance state
        // that a newer snapshot is using. Allocation and configuration stay
        // on this control-side publication path.
        for (size_t index = 0; index < m_regions.size(); ++index) {
            auto& region = m_regions[index];
            static_assert(sizeof(HirariAudioNoteSegmentRange) == 24);
            static_assert(offsetof(HirariAudioNoteSegmentRange, startSeconds) == 0);
            static_assert(offsetof(HirariAudioNoteSegmentRange, endSeconds) == 8);
            static_assert(offsetof(HirariAudioNoteSegmentRange, detectedPitchCents) == 16);
            region.audioNoteSegmentRanges.resize(region.audioNoteSegments.size());
            for (size_t segmentIndex = 0;
                 segmentIndex < region.audioNoteSegments.size(); ++segmentIndex) {
                region.audioNoteSegmentRanges[segmentIndex] = {
                    region.audioNoteSegments[segmentIndex].startSeconds,
                    region.audioNoteSegments[segmentIndex].endSeconds,
                    region.audioNoteSegments[segmentIndex].detectedPitchCents};
            }
            region.audioNoteCurveViews.resize(region.audioNoteSegments.size());
            for (size_t segmentIndex = 0;
                 segmentIndex < region.audioNoteSegments.size(); ++segmentIndex) {
                const auto& segment = region.audioNoteSegments[segmentIndex];
                region.audioNoteCurveViews[segmentIndex] = {
                    segment.anchors.data(), segment.anchors.size(),
                    segment.pitchRatioIntegralPrefix.data(), segment.startSeconds,
                    segment.endSeconds, segment.pitchOffsetCents,
                    segment.pitchRatioCorrectionBeforeSeconds,
                    segment.formantOffsetCents};
            }
            region.needsSpectralStretch = hirari_region_needs_spectral_stretch(
                region.sourceFramesPerTimelineFrame(), region.warpMarkers.size(),
                region.pitchSemitones, region.audioNoteCurveViews.data(),
                region.audioNoteCurveViews.size());
            if (!region.pitchPreserveWarp) continue;
            region.timeStretch = std::make_shared<RegionTimeStretch>();
            (void)region.timeStretch->prepare(m_sampleRate,
                std::max<uint32_t>(m_blockSize, 512u));
        }
        auto snapshot = std::make_unique<RegionSnapshot>(m_regions);
        for (auto& region : *snapshot) {
            region.audioNoteCurveViews.resize(region.audioNoteSegments.size());
            for (size_t segmentIndex = 0;
                 segmentIndex < region.audioNoteSegments.size(); ++segmentIndex) {
                const auto& segment = region.audioNoteSegments[segmentIndex];
                region.audioNoteCurveViews[segmentIndex] = {
                    segment.anchors.data(), segment.anchors.size(),
                    segment.pitchRatioIntegralPrefix.data(), segment.startSeconds,
                    segment.endSeconds, segment.pitchOffsetCents,
                    segment.pitchRatioCorrectionBeforeSeconds,
                    segment.formantOffsetCents};
            }
        }
        static_assert(sizeof(HirariRegionOverlapView) == 48);
        static_assert(offsetof(HirariRegionOverlapView, muted) == 24);
        static_assert(offsetof(HirariRegionOverlapView, crossfade_in) == 32);
        std::vector<HirariRegionOverlapView> overlapViews;
        overlapViews.reserve(snapshot->size());
        for (const auto& region : *snapshot) {
            overlapViews.push_back({region.start, region.len, region.loopCount,
                static_cast<uint8_t>(region.muted ? 1 : 0),
                region.crossfadeInSamples, region.crossfadeOutSamples});
        }
        if (!hirari_regions_apply_overlap_crossfades(
                overlapViews.data(), overlapViews.size())) {
            return;
        }
        for (size_t index = 0; index < snapshot->size(); ++index) {
            (*snapshot)[index].crossfadeInSamples = overlapViews[index].crossfade_in;
            (*snapshot)[index].crossfadeOutSamples = overlapViews[index].crossfade_out;
        }
        static_assert(sizeof(HirariWarpMarker) == 24);
        static_assert(offsetof(HirariWarpMarker, sourceSample) == 0);
        static_assert(offsetof(HirariWarpMarker, timelineSample) == 8);
        static_assert(offsetof(HirariWarpMarker, transient) == 16);
        static_assert(sizeof(Region::RangeEdit) == sizeof(HirariRegionRangeEdit));
        static_assert(offsetof(Region::RangeEdit, start) == offsetof(HirariRegionRangeEdit, start));
        static_assert(offsetof(Region::RangeEdit, end) == offsetof(HirariRegionRangeEdit, end));
        static_assert(offsetof(Region::RangeEdit, gain) == offsetof(HirariRegionRangeEdit, gain));
        static_assert(offsetof(Region::RangeEdit, fadeIn) == offsetof(HirariRegionRangeEdit, fade_in));
        static_assert(offsetof(Region::RangeEdit, fadeOut) == offsetof(HirariRegionRangeEdit, fade_out));
        static_assert(sizeof(Region::CompRange) == sizeof(HirariRegionCompRange));
        static_assert(offsetof(Region::CompRange, start) == offsetof(HirariRegionCompRange, start));
        static_assert(offsetof(Region::CompRange, end) == offsetof(HirariRegionCompRange, end));
        static_assert(offsetof(Region::CompRange, fadeIn) == offsetof(HirariRegionCompRange, fade_in));
        static_assert(offsetof(Region::CompRange, fadeOut) == offsetof(HirariRegionCompRange, fade_out));
        static_assert(sizeof(HirariRegionBlockConfig) == 88);
        static_assert(sizeof(HirariRegionBlockOutput) == 56);
        static_assert(sizeof(HirariRegionRenderConfig) == 128);
        static_assert(sizeof(HirariAudioNoteCurveView) == 64);
        static_assert(offsetof(HirariAudioNoteCurveView, formantOffsetCents) == 56);
        static_assert(sizeof(HirariTrackRegionRenderInput) == 312);
        static_assert(offsetof(HirariTrackRegionRenderInput, playhead) == 160);
        static_assert(offsetof(HirariTrackRegionRenderInput, warpRatio) == 248);
        static_assert(offsetof(HirariTrackRegionRenderInput, muted) == 300);
        snapshot->renderInputs.resize(snapshot->size());
        for (size_t index = 0; index < snapshot->size(); ++index) {
            const auto& region = (*snapshot)[index];
            const uint32_t sourceChannels = region.audio ? region.audio->getNumChannels() : 0;
            const uint64_t sourceSamples = region.audio ? region.audio->getNumSamples() : 0;
            const float* sourceLeft = sourceChannels > 0 ? region.audio->getReadPointer(0) : nullptr;
            const float* sourceRight = sourceChannels > 1 ? region.audio->getReadPointer(1) : sourceLeft;
            auto& input = snapshot->renderInputs[index];
            input.sourceLeft = sourceLeft;
            input.sourceRight = sourceRight;
            input.compRanges = reinterpret_cast<const HirariRegionCompRange*>(region.compRanges.data());
            input.compRangeCount = region.compRanges.size();
            input.rangeEdits = reinterpret_cast<const HirariRegionRangeEdit*>(region.rangeEdits.data());
            input.rangeEditCount = region.rangeEdits.size();
            input.noteRanges = region.audioNoteSegmentRanges.data();
            input.noteRangeCount = region.audioNoteSegmentRanges.size();
            input.noteCurves = region.audioNoteCurveViews.data();
            input.noteCurveCount = region.audioNoteCurveViews.size();
            input.warpMarkers = region.warpMarkers.data();
            input.warpMarkerCount = region.warpMarkers.size();
            input.resampleKernel = m_regionResampleKernel;
            input.wsolaWindow = kTrackWsolaWindow.data();
            input.stretchHandle = region.timeStretch ? region.timeStretch->rustState() : nullptr;
            input.cacheOwner = reinterpret_cast<std::uintptr_t>(this);
            input.cacheSnapshot = reinterpret_cast<std::uintptr_t>(snapshot.get());
            input.cacheNoteSegments = reinterpret_cast<std::uintptr_t>(region.audioNoteSegments.data());
            input.regionStart = region.start;
            input.regionLength = region.len;
            input.loopCount = region.loopCount;
            input.sourceSamples = sourceSamples;
            input.sourceOffset = region.sourceOffset;
            input.sourceLength = region.sourceLength;
            input.fadeIn = region.fadeInSamples;
            input.fadeOut = region.fadeOutSamples;
            input.crossfadeIn = region.crossfadeInSamples;
            input.crossfadeOut = region.crossfadeOutSamples;
            input.warpRatio = region.warpRatio;
            input.sourceSampleRate = region.sourceSampleRate;
            input.timelineSampleRate = region.timelineSampleRate;
            input.sampleRate = m_sampleRate;
            input.clipGain = region.clipGain;
            input.pitchSemitones = region.pitchSemitones;
            input.regionId = region.id;
            input.syncGroup = region.syncGroup;
            input.muted = static_cast<uint8_t>(region.muted ? 1 : 0);
            input.compManaged = static_cast<uint8_t>(region.compManaged ? 1 : 0);
            input.reverse = static_cast<uint8_t>(region.reverse ? 1 : 0);
            input.pitchPreserveWarp = static_cast<uint8_t>(region.pitchPreserveWarp ? 1 : 0);
            input.needsSpectralStretch = static_cast<uint8_t>(region.needsSpectralStretch ? 1 : 0);
        }
        const RegionSnapshot* published = snapshot.get();
        m_regionSnapshots.push_back(std::move(snapshot));
        m_regionGeneration.fetch_add(1, std::memory_order_release);
        m_regionSnapshot.store(published, std::memory_order_release);
        markProjectLayoutChanged();
    }

    static float evaluateAutomation(const AutomationList& points, double time, size_t& lastIndexHint) {
        static_assert(sizeof(ProjectSerializer::AutomationPoint) == 16);
        static_assert(offsetof(ProjectSerializer::AutomationPoint, value) == 8);
        static_assert(offsetof(ProjectSerializer::AutomationPoint, curve) == 12);
        return hirari_realtime_automation_evaluate(
            points.data(), points.size(), time, &lastIndexHint);
    }
    std::shared_ptr<ProjectLayoutRevision> m_layoutRevision;
    uint32_t m_id;
    std::string m_name;
    Type m_type;
    DSP::Mixing::ChannelStrip m_strip;
    Hirari::Core::EffectChain m_effectChain;
    MidiBuffer m_pluginMidi;
    void* m_articulationState = nullptr;
    void* m_builtInMidiState = nullptr;
    double m_sampleRate = 44100.0;
    const float* m_regionResampleKernel = nullptr;
    std::atomic<double> m_transportBpm{120.0};
    std::atomic<bool> m_transportPlaying{false};
    uint32_t m_blockSize = 0;
    bool m_isPrepared = false;
    std::atomic<float> m_volume{1.0f};
    std::atomic<float> m_pan{0.0f};
    void* m_trackAutomationCaptureState = nullptr;
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
    std::atomic<bool> m_audioPitchAnalysisRunning{false};
    std::vector<uint32_t> m_pluginTypes;
    // Updated on the control thread from sandbox-reported plugin metadata;
    // the callback reads it to choose the built-in MIDI fallback.
    std::atomic<uint32_t> m_midiInstrumentPluginCount{0};
    std::vector<std::shared_ptr<Hirari::Core::MidiRegion>> m_midiRegions;
    std::vector<std::string> m_sandboxedPluginPaths;
    std::vector<std::vector<uint8_t>> m_pluginGuiStates;
    std::vector<uint32_t> m_sandboxedPluginIndices;
    std::atomic<uint8_t> m_lastSandboxFailure{0};
    std::string m_lastSandboxFailureText = "none";
    RegionProcessor m_regionProcessor;
    AudioBuffer m_activeBuffer;
    AudioBuffer m_preFaderBuffer;
    AudioBuffer m_sendPreFaderBuffer;
    AudioBuffer m_sendPostFaderBuffer;
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
    struct PluginAutomationSnapshot {
        PluginAutomationList lanes;
        std::vector<HirariPluginAutomationLaneView> views;
    };
    std::vector<std::unique_ptr<PluginAutomationSnapshot>> m_pluginAutomationHistory;
    std::atomic<const PluginAutomationSnapshot*> m_pluginAutomation{nullptr};
    std::atomic<float> m_spatialX{0.0f}, m_spatialY{0.0f}, m_spatialZ{0.0f};
    std::atomic<uint32_t> m_spatialMode{0};
    ::Hirari::DSP::Spatial::HolographicPanner m_spatialPanner;
    static constexpr uint32_t kMaxPdcDelaySamples = 8192;
    std::atomic<uint32_t> m_pdcRequested{0};
    std::atomic<uint32_t> m_trackDelaySamples{0};
    void* m_pdcDelayState = nullptr;
};
