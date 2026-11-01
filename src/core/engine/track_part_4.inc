    struct RegionSnapshot : std::vector<Region> {
        explicit RegionSnapshot(const std::vector<Region>& source)
            : std::vector<Region>(source) {}
        std::vector<HirariTrackRegionRenderInput> renderInputs;
    };

    // Replaces the Rust mirror with the current region list. Every region
    // mutation ends in publishRegionSnapshot(), so syncing here keeps the
    // mirror equal to m_regions without touching each mutation path.
    void syncRegionTableMirror() {
        std::vector<uint32_t> ids;
        std::vector<HirariRegionState> states;
        ids.reserve(m_regions.size());
        states.reserve(m_regions.size());
        for (const auto& region : m_regions) {
            ids.push_back(region.id);
            states.push_back(region.editableState());
        }
        (void)hirari_region_table_replace(m_regionTable, ids.data(), states.data(), ids.size());
    }

    void publishRegionSnapshot() {
        syncRegionTableMirror();
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
            // The note-edit table is Rust-owned: republish its immutable
            // projection here and let the callback read it lock-free.
            region.publishNoteEdits();
            region.publishClipEdits();
            region.needsSpectralStretch = hirari_region_needs_spectral_stretch(
                region.sourceFramesPerTimelineFrame(), region.clip()->warpMarkerCount,
                region.pitchSemitones, region.noteCurves, region.noteCurveCount);
            if (!region.pitchPreserveWarp) continue;
            region.timeStretch = std::shared_ptr<void>(
                hirari_region_stretch_create(),
                [](void* handle) { hirari_region_stretch_destroy(handle); });
            (void)hirari_region_stretch_prepare(region.timeStretch.get(), m_sampleRate,
                                                std::max<uint32_t>(m_blockSize, 512u));
        }
        auto snapshot = std::make_unique<RegionSnapshot>(m_regions);
        // The published note views address the shared Rust snapshot, so the
        // snapshot copy needs no second rebuild pass.
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
            const auto* clip = region.clip();
            input.sourceLeft = sourceLeft;
            input.sourceRight = sourceRight;
            input.compRanges = clip->compRanges;
            input.compRangeCount = clip->compRangeCount;
            input.rangeEdits = clip->rangeEdits;
            input.rangeEditCount = clip->rangeEditCount;
            input.noteRanges = region.noteRanges;
            input.noteRangeCount = region.noteRangeCount;
            input.noteCurves = region.noteCurves;
            input.noteCurveCount = region.noteCurveCount;
            input.warpMarkers = clip->warpMarkers;
            input.warpMarkerCount = clip->warpMarkerCount;
            input.resampleKernel = m_regionResampleKernel;
            input.wsolaWindow = kTrackWsolaWindow.data();
            input.stretchHandle = region.timeStretch.get();
            input.cacheOwner = reinterpret_cast<std::uintptr_t>(this);
            input.cacheSnapshot = reinterpret_cast<std::uintptr_t>(snapshot.get());
            input.cacheNoteSegments = reinterpret_cast<std::uintptr_t>(region.noteCacheKey);
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
            input.compManaged = clip->compManaged;
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

    std::shared_ptr<ProjectLayoutRevision> m_layoutRevision;
    uint32_t m_id;
    std::string m_name;
    Type m_type;
    // Channel-strip gain/pan/solo state is Rust-owned; this is its handle.
    void* m_strip = hirari_channel_strip_create();
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
    std::atomic<bool> m_muted{false};
    std::atomic<bool> m_phaseInvert{false};
    std::atomic<bool> m_solo{false};
    std::atomic<bool> m_recordArmed{false};
    std::vector<Region> m_regions;
    HirariRegionTable* m_regionTable = hirari_region_table_new();
    mutable std::mutex m_regionEditMutex;
    // Published only by the control thread. Old generations intentionally
    // remain alive until Track destruction, avoiding RT-side shared_ptr release.
    std::vector<std::unique_ptr<RegionSnapshot>> m_regionSnapshots;
    std::atomic<const RegionSnapshot*> m_regionSnapshot{nullptr};
    std::atomic<uint64_t> m_regionGeneration{0};
    std::atomic<bool> m_audioPitchAnalysisRunning{false};
    // Updated on the control thread from sandbox-reported plugin metadata;
    // the callback reads it to choose the built-in MIDI fallback.
    std::atomic<uint32_t> m_midiInstrumentPluginCount{0};
    // Rust-owned slot registry: chain index and path per sandboxed plugin.
    void* m_sandboxSlots = hirari_sandbox_slots_create();
    std::atomic<uint8_t> m_lastSandboxFailure{0};
    std::string m_lastSandboxFailureText = "none";
    AudioBuffer m_activeBuffer;
    AudioBuffer m_preFaderBuffer;
    AudioBuffer m_sendPreFaderBuffer;
    AudioBuffer m_sendPostFaderBuffer;
    AudioBuffer m_preInsertBuffer;
    // Control-thread-owned retention prevents an in-flight audio callback
    // from observing freed freeze memory; the store publishes the read-side
    // pointer through an atomic the callback loads without locking.
    static void destroyFrozenOwner(void* owner) {
        delete static_cast<std::shared_ptr<AudioBuffer>*>(owner);
    }
    static constexpr HirariTrackFrozenOps kFrozenOps = {&Track::destroyFrozenOwner};
    void* m_frozenStore = hirari_track_frozen_store_create(&kFrozenOps);
    float m_lowGain = 1.0f;
    float m_highGain = 1.0f;
    float m_lowStateL = 0.0f;
    float m_lowStateR = 0.0f;
    void* m_automationStore = hirari_track_automation_store_create();
    std::atomic<float> m_spatialX{0.0f}, m_spatialY{0.0f}, m_spatialZ{0.0f};
    // The spatial processor is Rust-owned; the track keeps only its handle.
    void* m_spatialPanner = hirari_track_holographic_panner_create();
    static constexpr uint32_t kMaxHrtfTaps = 128;
    static constexpr uint32_t kMaxPdcDelaySamples = 8192;
    std::atomic<uint32_t> m_pdcRequested{0};
    std::atomic<uint32_t> m_trackDelaySamples{0};
    void* m_pdcDelayState = nullptr;
};
