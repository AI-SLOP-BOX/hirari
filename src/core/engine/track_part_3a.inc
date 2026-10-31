    void setEq(float lowBoostDb, float lowCutDb, float highBoostDb, float highCutDb) {
        m_lowGain = hirari_track_eq_gain(lowBoostDb, lowCutDb);
        m_highGain = hirari_track_eq_gain(highBoostDb, highCutDb);
    }
    virtual void prepareToPlay(double sr, uint32_t blockSize) {
        if (!std::isfinite(sr) || sr <= 0.0 || blockSize == 0) return;
        m_sampleRate = sr;
        m_blockSize = blockSize;
        m_isPrepared = true;
        {
            std::lock_guard<std::mutex> regionLock(m_regionEditMutex);
            for (auto& region : m_regions) {
                if (region.pitchPreserveWarp && region.timeStretch)
                    (void)region.timeStretch->prepare(sr, blockSize);
            }
        }
        m_strip.setSampleRate(sr);
        m_spatialPanner.setSampleRate(sr);
        // Preparing a track is also the boundary used by offline renders and
        // device reinitialisation. Snap smoothed channel-strip state to its
        // current targets so the first block is deterministic after reload.
        m_strip.reset();
        m_activeBuffer.resize(2, blockSize);
        m_preFaderBuffer.resize(2, blockSize);
        m_sendPreFaderBuffer.resize(2, blockSize);
        m_sendPostFaderBuffer.resize(2, blockSize);
        m_preInsertBuffer.resize(2, blockSize);
        m_effectChain.setSampleRate(sr, blockSize);
        // A published region snapshot bakes this track's sample rate into
        // HirariTrackRegionRenderInput::sampleRate. prepareToPlay is the only
        // place the rate changes, and it is also the offline-render and
        // project-reload boundary, so a snapshot published earlier (while the
        // track still held its default rate) would otherwise keep rendering at
        // the stale rate. Republish so the rate is captured with the new
        // configuration. This is a per-prepare rebuild, not a per-block reset.
        publishRegionSnapshot();
    }
    void resetForOfflineRender() noexcept {
        m_strip.reset();
        m_effectChain.reset();
        m_volumeAutomationHint = 0;
        m_panAutomationHint = 0;
        m_trackDelayAutomationHint = 0;
        const uint32_t pdc = m_pdcRequested.load(std::memory_order_acquire);
        const uint32_t manual = m_trackDelaySamples.load(std::memory_order_acquire);
        const uint32_t requested = std::min(kMaxPdcDelaySamples,
            pdc > kMaxPdcDelaySamples - manual ? kMaxPdcDelaySamples : pdc + manual);
        hirari_track_pdc_delay_reset(m_pdcDelayState, requested);
    }

    // Publishes a fully rendered, immutable track snapshot.  Loading and
    // decoding are control-thread responsibilities; the audio thread only
    // observes the raw pointer and copies bounded samples from it.  Published
    // buffers are retained for the lifetime of the Track so replacing a
    // freeze cannot invalidate a callback already in flight.
    bool publishFrozenAudio(std::shared_ptr<AudioBuffer> rendered,
                            uint64_t totalSamples,
                            uint32_t sampleRate,
                            uint64_t projectGeneration = 0,
                            uint64_t audioGeneration = 0,
                            std::string_view cachePath = {}) {
        if (!rendered || rendered->getNumChannels() == 0 ||
            rendered->getNumSamples() == 0 || sampleRate == 0 ||
            totalSamples == 0 || totalSamples > rendered->getNumSamples()) {
            return false;
        }
        std::lock_guard<std::mutex> freezeLock(m_freezeEditMutex);
        m_frozenBuffers.push_back(rendered);
        m_frozenTotalSamples.store(totalSamples, std::memory_order_relaxed);
        m_frozenSampleRate.store(sampleRate, std::memory_order_relaxed);
        m_frozenProjectGeneration.store(projectGeneration, std::memory_order_relaxed);
        m_frozenAudioGeneration.store(audioGeneration, std::memory_order_relaxed);
        m_frozenPath = std::string(cachePath);
        m_frozenAudio.store(rendered.get(), std::memory_order_release);
        m_frozen.store(true, std::memory_order_release);
        markProjectLayoutChanged();
        return true;
    }

    void clearFrozenAudio() noexcept {
        std::lock_guard<std::mutex> freezeLock(m_freezeEditMutex);
        m_frozen.store(false, std::memory_order_release);
        m_frozenAudio.store(nullptr, std::memory_order_release);
        m_frozenTotalSamples.store(0, std::memory_order_relaxed);
        m_frozenSampleRate.store(0, std::memory_order_relaxed);
        m_frozenProjectGeneration.store(0, std::memory_order_relaxed);
        m_frozenAudioGeneration.store(0, std::memory_order_relaxed);
        m_frozenPath.clear();
        markProjectLayoutChanged();
    }

    // Must be called by the control side only after the audio callback has
    // been quiesced. This is the reclamation point for replaced freeze
    // buffers; keeping it explicit avoids both RT-side refcount work and an
    // unbounded control-side retention leak during repeated freeze cycles.
    void collectRetiredFrozenAudio() {
        std::lock_guard<std::mutex> freezeLock(m_freezeEditMutex);
        const AudioBuffer* current = m_frozenAudio.load(std::memory_order_acquire);
        m_frozenBuffers.erase(
            std::remove_if(m_frozenBuffers.begin(), m_frozenBuffers.end(),
                [current](const auto& buffer) { return buffer.get() != current; }),
            m_frozenBuffers.end());
    }

    bool isFrozen() const noexcept { return m_frozen.load(std::memory_order_acquire); }
    // Live edits invalidate the rendered snapshot so stale audio cannot
    // survive a parameter, region, or automation mutation.
    void invalidateFrozenForEdit() noexcept {
        clearFrozenAudio();
    }
    uint64_t frozenTotalSamples() const noexcept {
        return m_frozenTotalSamples.load(std::memory_order_relaxed);
    }
    uint32_t frozenSampleRate() const noexcept {
        return m_frozenSampleRate.load(std::memory_order_relaxed);
    }
    uint64_t frozenProjectGeneration() const noexcept {
        return m_frozenProjectGeneration.load(std::memory_order_relaxed);
    }
    uint64_t frozenAudioGeneration() const noexcept {
        return m_frozenAudioGeneration.load(std::memory_order_relaxed);
    }
    std::string frozenPath() const {
        std::lock_guard<std::mutex> freezeLock(m_freezeEditMutex);
        return m_frozenPath;
    }
    bool setFrozenCachePath(std::string_view path) {
        if (path.empty() || !isFrozen()) return false;
        std::lock_guard<std::mutex> freezeLock(m_freezeEditMutex);
        m_frozenPath = std::string(path);
        markProjectLayoutChanged();
        return true;
    }

    // Offline render taps are prepared on the control thread and only copied
    // on the audio side. They let the render graph select a real pre-fader or
    // pre-insert signal without changing the interactive mix state.
    const AudioBuffer& offlinePreFaderBuffer() const noexcept { return m_preFaderBuffer; }
    const AudioBuffer& sendPreFaderBuffer() const noexcept { return m_sendPreFaderBuffer; }
    const AudioBuffer& sendPostFaderBuffer() const noexcept { return m_sendPostFaderBuffer; }
    const AudioBuffer& offlinePreInsertBuffer() const noexcept { return m_preInsertBuffer; }
    const AudioBuffer& preFxSidechainBuffer() const noexcept { return m_preInsertBuffer; }
    const AudioBuffer& postFxSidechainBuffer() const noexcept { return m_preFaderBuffer; }
    // Complete the channel signal after all insert chains. Rust executes this
    // realtime stage, including the pre-fader taps and linked output DSP.
    void finalizeTrackOutput(uint32_t frames, uint64_t playhead) noexcept {
        auto& buffer = getWorkBuffer(frames);
        const auto* volumeAutomation = m_volumeAutomation.load(std::memory_order_acquire);
        const auto* panAutomation = m_panAutomation.load(std::memory_order_acquire);
        const bool validPreInsert = m_preInsertBuffer.getNumChannels() >= buffer.getNumChannels() &&
            m_preInsertBuffer.getNumSamples() >= frames;
        const bool validPreFader = m_preFaderBuffer.getNumChannels() >= 2 &&
            m_preFaderBuffer.getNumSamples() >= frames;
        const bool validSendPre = m_sendPreFaderBuffer.getNumChannels() >= 2 &&
            m_sendPreFaderBuffer.getNumSamples() >= frames;
        const bool validSendPost = m_sendPostFaderBuffer.getNumChannels() >= 2 &&
            m_sendPostFaderBuffer.getNumSamples() >= frames;
        hirari_track_finalize_output(
            m_id, frames, static_cast<double>(playhead),
            volumeAutomation && !volumeAutomation->empty() ? volumeAutomation->data() : nullptr,
            volumeAutomation ? volumeAutomation->size() : 0, &m_volumeAutomationHint,
            panAutomation && !panAutomation->empty() ? panAutomation->data() : nullptr,
            panAutomation ? panAutomation->size() : 0, &m_panAutomationHint,
            m_strip.nativeState(), buffer.getArrayOfWritePointers(), buffer.getNumChannels(),
            validPreInsert ? m_preInsertBuffer.getArrayOfWritePointers() : nullptr,
            validPreInsert ? m_preInsertBuffer.getNumChannels() : 0,
            validSendPre ? m_sendPreFaderBuffer.getArrayOfWritePointers() : nullptr,
            validSendPre ? m_sendPreFaderBuffer.getNumChannels() : 0,
            validSendPost ? m_sendPostFaderBuffer.getArrayOfWritePointers() : nullptr,
            validSendPost ? m_sendPostFaderBuffer.getNumChannels() : 0,
            m_spatialPanner.nativeState(), m_spatialMode.load(std::memory_order_relaxed),
            m_spatialX.load(std::memory_order_relaxed),
            m_spatialY.load(std::memory_order_relaxed),
            m_spatialZ.load(std::memory_order_relaxed), m_lowGain, m_highGain,
            &m_lowStateL, &m_lowStateR, m_pdcDelayState,
            m_pdcRequested.load(std::memory_order_acquire),
            m_trackDelaySamples.load(std::memory_order_acquire),
            validPreFader ? m_preFaderBuffer.getArrayOfWritePointers() : nullptr,
            validPreFader ? m_preFaderBuffer.getNumChannels() : 0);
    }
    void setSpatialPosition(float x, float y, float z) {
        // Spatial coordinates are control-thread input. Never publish a
        // non-finite value to the real-time panner.
        const auto sanitize = [](float value) {
            return std::isfinite(value) ? std::clamp(value, -100.0f, 100.0f) : 0.0f;
        };
        invalidateFrozenForEdit();
        m_spatialX.store(sanitize(x), std::memory_order_relaxed);
        m_spatialY.store(sanitize(y), std::memory_order_relaxed);
        m_spatialZ.store(sanitize(z), std::memory_order_relaxed);
        markProjectLayoutChanged();
    }
    float getSpatialX() const { return m_spatialX.load(std::memory_order_relaxed); }
    float getSpatialY() const { return m_spatialY.load(std::memory_order_relaxed); }
    float getSpatialZ() const { return m_spatialZ.load(std::memory_order_relaxed); }
    bool setHrtfKernel(const float* left, const float* right, uint32_t taps) {
        if (!left || !right || taps == 0 || taps > ::Hirari::DSP::Spatial::HolographicPanner::kMaxHrtfTaps) {
            return false;
        }
        invalidateFrozenForEdit();
        const bool changed = m_spatialPanner.setHrtfKernel(left, right, taps);
        if (changed) markProjectLayoutChanged();
        return changed;
    }
    void clearHrtfKernel() {
        invalidateFrozenForEdit();
        m_spatialPanner.clearHrtfKernel();
        markProjectLayoutChanged();
    }
    bool isMuted() const { return m_muted.load(std::memory_order_relaxed); }
    void setMuted(bool muted) {
        invalidateFrozenForEdit();
        m_muted.store(muted, std::memory_order_relaxed);
        markProjectLayoutChanged();
    }
    bool isPhaseInverted() const { return m_phaseInvert.load(std::memory_order_relaxed); }
    void setPhaseInverted(bool inverted) {
        invalidateFrozenForEdit();
        m_phaseInvert.store(inverted, std::memory_order_relaxed);
        markProjectLayoutChanged();
    }
    void setSolo(bool solo) {
        invalidateFrozenForEdit();
        m_solo.store(solo, std::memory_order_relaxed);
        markProjectLayoutChanged();
    }
    bool isSolo() const { return m_solo.load(std::memory_order_relaxed); }
    void setEcoMode(bool eco) { m_isEcoMode.store(eco, std::memory_order_relaxed); }
    const std::vector<Region>& getRegions() const noexcept {
        const auto* snapshot = getRegionSnapshot();
        if (snapshot != nullptr) return *snapshot;
        static const std::vector<Region> empty;
        return empty;
    }
    // Immutable generations are retained for the lifetime of the Track. UI
    // and analysis code must read this snapshot instead of m_regions, which
    // is owned by the control side and may be edited between frames.
    const std::vector<Region>* getRegionSnapshot() const noexcept {
        return m_regionSnapshot.load(std::memory_order_acquire);
    }
    // Rust mirror of the region table as of the last snapshot publish. Every
    // mutation of m_regions ends in publishRegionSnapshot(), which refreshes it.
    const HirariRegionTable* regionTableMirror() const noexcept { return m_regionTable; }
    // Clones a region's Rust note table for an analysis worker. The worker
    // owns the returned store and releases it through the note-store ABI.
    HirariRegionVariAudioStore* cloneRegionNoteStore(uint32_t regionId) const {
        const auto* snapshot = getRegionSnapshot();
        if (!snapshot) return nullptr;
        const auto it = std::find_if(snapshot->begin(), snapshot->end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == snapshot->end()) return nullptr;
        return it->noteEdits.cloneHandle();
    }

    // Runs a Rust note-table mutation under the region lock and republishes.
    // The table handle is borrowed for the duration of the callback only.
    bool editRegionNoteStore(uint32_t regionId, void* context,
                             HirariRegionNoteMutateFn mutate) {
        if (!mutate || audioPitchAnalysisRunning()) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != regionId) continue;
            if (!mutate(context, region.noteEdits.storeHandle())) return false;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    bool beginAudioPitchAnalysis() noexcept {
        bool expected = false;
        return m_audioPitchAnalysisRunning.compare_exchange_strong(
            expected, true, std::memory_order_acq_rel);
    }
    void finishAudioPitchAnalysis() noexcept {
        m_audioPitchAnalysisRunning.store(false, std::memory_order_release);
    }
    bool audioPitchAnalysisRunning() const noexcept {
        return m_audioPitchAnalysisRunning.load(std::memory_order_acquire);
    }

    // Offline VariAudio-style F0 detection. This is deliberately a control/
    // analysis-thread operation; the audio callback only consumes the
    // immutable snapshot published below.
    bool analyzeAudioNoteSegments(uint32_t regionId,
                                  double sampleRate,
                                  ::hirari::editing::AudioPitchAnalyzer::Config config = {}) {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
        invalidateFrozenForEdit();
        Region analysisRegion{};
        uint64_t generation = 0;
        {
            std::lock_guard<std::mutex> editLock(m_regionEditMutex);
            auto it = std::find_if(m_regions.begin(), m_regions.end(),
                [regionId](const Region& region) { return region.id == regionId; });
            if (it == m_regions.end() || !it->audio || it->audio->getNumSamples() == 0 ||
                it->audio->getNumSamples() > 16'000'000 || it->audio->getNumChannels() == 0) {
                return false;
            }
            analysisRegion = *it;
            generation = regionGeneration();
        }
        const auto& audio = analysisRegion.audio;
        const double sourceSampleRate = std::isfinite(analysisRegion.sourceSampleRate) &&
            analysisRegion.sourceSampleRate >= 8000.0
            ? analysisRegion.sourceSampleRate : sampleRate;
        const double timelineSampleRate = std::isfinite(analysisRegion.timelineSampleRate) &&
            analysisRegion.timelineSampleRate >= 8000.0
            ? analysisRegion.timelineSampleRate : sampleRate;
        const uint64_t sourceSpan = analysisRegion.sourceLength > 0
            ? analysisRegion.sourceLength : analysisRegion.len;
        if (!audio || sourceSpan == 0 || sourceSpan > 16'000'000 ||
            analysisRegion.sourceOffset > audio->getNumSamples() ||
            sourceSpan > audio->getNumSamples() - analysisRegion.sourceOffset ||
            analysisRegion.len == 0) return false;

        const uint32_t channels = audio->getNumChannels();
        std::vector<const float*> channelData(channels, nullptr);
        for (uint32_t channel = 0; channel < channels; ++channel) {
            channelData[channel] = audio->getReadPointer(channel);
        }
        std::vector<::HirariWarpMarker> pitchWarpMarkers;
        pitchWarpMarkers.reserve(analysisRegion.warpMarkers.size());
        for (const auto& marker : analysisRegion.warpMarkers) {
            pitchWarpMarkers.push_back({marker.sourceSample, marker.timelineSample, marker.transient});
        }
        const double sourceFramesPerTimelineFrame = std::isfinite(analysisRegion.warpRatio) &&
            analysisRegion.sourceFramesPerTimelineFrame() > 0.0
            ? analysisRegion.sourceFramesPerTimelineFrame() : 1.0;
        void* rawAnalysis = hirari_audio_pitch_analyze_region(
            channelData.data(), channelData.size(),
            static_cast<size_t>(analysisRegion.sourceOffset), static_cast<size_t>(sourceSpan),
            analysisRegion.reverse ? 1 : 0, sourceSampleRate, timelineSampleRate,
            analysisRegion.len, sourceFramesPerTimelineFrame,
            pitchWarpMarkers.data(), pitchWarpMarkers.size(), config.minHz, config.maxHz,
            config.threshold, config.noteChangeThresholdCents,
            config.noteChangeConfirmationFrames);
        if (!rawAnalysis) return false;
        std::unique_ptr<void, decltype(&hirari_audio_pitch_result_destroy)>
            analysis(rawAnalysis, &hirari_audio_pitch_result_destroy);

        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end() || it->audio != audio || regionGeneration() != generation ||
            it->sourceOffset != analysisRegion.sourceOffset || it->sourceLength != analysisRegion.sourceLength ||
            it->len != analysisRegion.len || it->reverse != analysisRegion.reverse ||
            it->warpRatio != analysisRegion.warpRatio ||
            it->sourceSampleRate != analysisRegion.sourceSampleRate ||
            it->timelineSampleRate != analysisRegion.timelineSampleRate ||
            it->warpMarkers.size() != analysisRegion.warpMarkers.size())
            return false;
        for (size_t index = 0; index < it->warpMarkers.size(); ++index) {
            if (it->warpMarkers[index].sourceSample != analysisRegion.warpMarkers[index].sourceSample ||
                it->warpMarkers[index].timelineSample != analysisRegion.warpMarkers[index].timelineSample) return false;
        }
        if (!hirari_audio_pitch_commit_region_result(
                analysis.get(), it->noteEdits.storeHandle())) return false;
        publishRegionSnapshot();
        return true;
    }

    bool appendProcessingStep(uint32_t regionId, ::hirari::editing::EventProcessingStep step) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) if (region.id == regionId) {
            // The store owns the policy: zero id, empty/oversized operation,
            // parameter finiteness and the step cap.
            if (!region.processingHistory.append(step)) return false;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }
    bool clearProcessingHistory(uint32_t regionId) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) if (region.id == regionId) {
            if (region.processingHistory.empty()) return false;
            region.processingHistory.clear();
            publishRegionSnapshot();
            return true;
        }
        return false;
    }
    uint64_t regionGeneration() const {
        return m_regionGeneration.load(std::memory_order_acquire);
    }

    // May only be called while the audio callback is quiescent. This bounds
    // the retired-generation list after a long editing session without ever
    // reclaiming memory that an RT callback could still be reading.
    void collectRetiredRegionSnapshots() {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        const RegionSnapshot* current =
            m_regionSnapshot.load(std::memory_order_acquire);
        m_regionSnapshots.erase(
            std::remove_if(m_regionSnapshots.begin(), m_regionSnapshots.end(),
                [current](const auto& snapshot) { return snapshot.get() != current; }),
            m_regionSnapshots.end());
    }

    // Host entry point for the scalar fields the Rust policy owns. Rust has
    // already decided and resolved every value, so this only stores them and
    // republishes; no second validation can disagree with it.
    bool applyRegionScalarFields(uint32_t rid, const HirariRegionState& state) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid) continue;
            region.clipGain = state.clipGain;
            region.muted = state.muted;
            region.reverse = state.reverse;
            region.pitchSemitones = state.pitchSemitones;
            region.loopCount = state.loopCount;
            region.syncGroup = state.syncGroup;
            region.fadeInSamples = state.fadeInSamples;
            region.fadeOutSamples = state.fadeOutSamples;
            region.locked = state.locked;
            if (state.pitchPreserveWarp != region.pitchPreserveWarp) {
                // The renderer needs the untrimmed length to build the warp
                // map, so backfill it the way the old setter did.
                if (region.sourceLength == 0) region.sourceLength = region.len;
                region.pitchPreserveWarp = state.pitchPreserveWarp;
            }
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    // Host entry point for trim and warp-ratio edits. Rust computed the
    // geometry and the undo pair; this stores exactly those fields and swaps
    // the marker list in a single publish.
    bool applyRegionGeometry(uint32_t rid, const HirariRegionState& state,
                             const HirariWarpMarker* markers, size_t markerCount) {
        if (markerCount != 0 && markers == nullptr) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked) continue;
            region.start = state.start;
            region.len = state.len;
            region.sourceOffset = state.sourceOffset;
            region.sourceLength = state.sourceLength;
            region.fadeInSamples = state.fadeInSamples;
            region.fadeOutSamples = state.fadeOutSamples;
            region.warpRatio = state.warpRatio;
            if (markerCount == 0) {
                region.warpMarkers.clear();
            } else {
                region.warpMarkers.assign(markers, markers + markerCount);
            }
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    // Host entry point for edits that move or re-trim a region. Re-adding the
    // replacement runs the same normalisation (`addRegion`) the previous
    // remove/add path used, including the sorted insert into `m_regions`.
    bool applyRegionState(uint32_t rid, const HirariRegionState& state) {
        invalidateFrozenForEdit();
        Region replacement;
        {
            std::lock_guard<std::mutex> editLock(m_regionEditMutex);
            bool found = false;
            for (const auto& region : m_regions) {
                if (region.id != rid) continue;
                replacement = region;
                found = true;
                break;
            }
            if (!found) return false;
        }
        replacement.applyEditableState(state);
        removeRegion(rid);
        addRegion(replacement);
        return true;
    }

    // Adopts the comp-range batch Rust admitted. The rules (count, ordering,
    // overlap, fade width, region bounds, lock/managed interaction) live in
    // `hirari_region_comp_ranges_validate`; storing the accepted batch is all
    // that is left here.
    bool replaceRegionCompRanges(uint32_t rid, std::vector<Region::CompRange> ranges,
                                 bool managed) {
        static_assert(sizeof(Region::CompRange) == sizeof(HirariRegionCompRange));
        static_assert(offsetof(Region::CompRange, start) == offsetof(HirariRegionCompRange, start));
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid) continue;
            region.compManaged = managed;
            region.compRanges = std::move(ranges);
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    // Adopts the result of a Rust range-edit transform.
    //
    // Rust returns a buffer it owns and the caller releases; nothing is ever
    // written through a pointer into this vector, so a returned count can no
    // longer be turned into a resize that value-initializes over data a
    // foreign language had already written.
    static bool adoptRangeEdits(std::vector<Region::RangeEdit>& destination,
                                const std::vector<Region::RangeEdit>& current,
                                uint32_t op, uint64_t regionLength,
                                uint64_t start = 0, uint64_t end = 0,
                                float gain = 0.0f, uint64_t fadeIn = 0,
                                uint64_t fadeOut = 0) {
        static_assert(sizeof(Region::RangeEdit) == sizeof(HirariRegionRangeEdit));
        static_assert(offsetof(Region::RangeEdit, start) == offsetof(HirariRegionRangeEdit, start));
        size_t produced = 0;
        HirariRegionRangeEdit* result = hirari_region_range_edits_apply(
            reinterpret_cast<const HirariRegionRangeEdit*>(current.data()),
            current.size(), regionLength, op, start, end, gain, fadeIn, fadeOut,
            &produced);
        if (!result) return false;
        std::vector<Region::RangeEdit> adopted;
        adopted.reserve(produced);
        for (size_t index = 0; index < produced; ++index) {
            const auto& edit = result[index];
            adopted.push_back({edit.start, edit.end, edit.gain, edit.fade_in, edit.fade_out});
        }
        hirari_free_range_edits(result, produced);
        destination = std::move(adopted);
        return true;
    }

    std::vector<Region::RangeEdit> regionRangeEdits(uint32_t rid) const {
        for (const auto& region : m_regions) if (region.id == rid) return region.rangeEdits;
        return {};
    }

    bool replaceRegionRangeEdits(uint32_t rid, std::vector<Region::RangeEdit> edits,
                                 bool validate = true) {
        if (edits.size() > 4096) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked) continue;
            if (validate) {
                if (!adoptRangeEdits(region.rangeEdits, edits, 3u, region.len)) return false;
            } else {
                // Rust already produced this sorted, bounded list; storing it
                // verbatim is the old direct-setter path, not a re-admission.
                region.rangeEdits = std::move(edits);
            }
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    // Validates and orders warp markers through the Rust pure transform.
    //
    // Rust returns a buffer it owns and never writes into caller storage, so
    // the former in-place sort/compact cannot corrupt a C++ vector. Returns
    // false when normalization would drop any marker, matching the previous
    // contract that rejected anything which did not survive 1:1. On success
    // `ordered` holds the normalized markers in their stored order.
    static bool normalizeWarpMarkers(const std::vector<Region::WarpMarker>& input,
                                     uint64_t sourceSpan, uint64_t timelineLength,
                                     std::vector<Region::WarpMarker>& ordered) {
        // An empty marker list is valid: it means "no warp markers", which is
        // the state of nearly every region. The normalizer below reports
        // emptiness with a null pointer, which previously made clearing (or
        // restoring) an unmarked region fail and broke project loading.
        if (input.empty()) {
            ordered.clear();
            return true;
        }
        static_assert(sizeof(Region::WarpMarker) == sizeof(HirariWarpMarker));
        size_t produced = 0;
        HirariWarpMarker* result = hirari_region_normalize_warp_markers(
            reinterpret_cast<const HirariWarpMarker*>(input.data()), input.size(),
            sourceSpan, timelineLength, &produced);
        if (!result) return false;
        std::vector<Region::WarpMarker> adopted;
        adopted.reserve(produced);
        for (size_t index = 0; index < produced; ++index) {
            adopted.push_back(result[index]);
        }
        hirari_free_warp_markers(result, produced);
        if (produced != input.size()) return false;
        ordered = std::move(adopted);
        return true;
    }

    bool setRegionWarpMarkers(uint32_t rid, std::vector<Region::WarpMarker> markers) {
        if (markers.size() > 100'000) return false;
        if (!normalizeWarpMarkers(markers, UINT64_MAX, UINT64_MAX, markers)) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r) {
            return r.id == rid;
        });
        if (it == m_regions.end() || it->locked) return false;
        const uint64_t sourceSpan = it->sourceLength > 0 ? it->sourceLength : it->len;
        if (!normalizeWarpMarkers(markers, sourceSpan, it->len, markers)) return false;
        if (it->warpMarkers.size() == markers.size() &&
            std::equal(it->warpMarkers.begin(), it->warpMarkers.end(), markers.begin(),
                       [](const auto& a, const auto& b) {
                           return a.sourceSample == b.sourceSample &&
                               a.timelineSample == b.timelineSample &&
                               a.transient == b.transient;
                       })) return true;
        it->warpMarkers = std::move(markers);
        publishRegionSnapshot();
        return true;
    }

    // Audio-thread producer for write automation. The queue is deliberately
    // bounded and SPSC: recording a control surface gesture must never grow a
    // vector, take an edit lock, or allocate inside processBlock().
