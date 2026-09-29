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
    const AudioBuffer& preFaderBuffer() const noexcept { return m_preFaderBuffer; }
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
    void setSpatialMode(uint32_t mode) {
        invalidateFrozenForEdit();
        m_spatialMode.store(mode <= 2 ? mode : 0, std::memory_order_relaxed);
        markProjectLayoutChanged();
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
    [[deprecated("Use typed region mutators or clearRegions()")]]
    std::vector<Region>& getRegionsMutable() {
        // Legacy callers mutate this vector directly and cannot publish an
        // immutable region snapshot. Conservatively invalidate before handing
        // out the reference so cheap observers never miss the edit.
        markProjectLayoutChanged();
        return m_regions;
    }
    void clearRegions() {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        m_regions.clear();
        publishRegionSnapshot();
    }

    bool upsertAudioNoteSegment(uint32_t regionId, ::hirari::editing::AudioNoteSegment segment) {
        if (!segment.valid() || audioPitchAnalysisRunning()) return false;
        segment.rebuildPitchRatioIntegral();
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end()) return false;
        const auto ranges = ::hirari::editing::audioNoteSegmentRanges(it->audioNoteSegments);
        const int64_t slot = hirari_audio_note_upsert_segment_slot(
            ranges.data(), ranges.size(), segment.startSeconds, segment.endSeconds);
        if (slot == -2) return false;
        if (slot < 0) it->audioNoteSegments.push_back(std::move(segment));
        else it->audioNoteSegments[static_cast<size_t>(slot)] = std::move(segment);
        ::hirari::editing::rebuildAudioNotePhasePrefixes(it->audioNoteSegments);
        publishRegionSnapshot();
        return true;
    }

    bool copyAudioNoteSegments(uint32_t regionId,
                               std::vector<::hirari::editing::AudioNoteSegment>& output) const {
        const auto* snapshot = getRegionSnapshot();
        if (!snapshot) return false;
        const auto it = std::find_if(snapshot->begin(), snapshot->end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == snapshot->end()) return false;
        output = it->audioNoteSegments;
        return true;
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

    bool setAudioNoteSegments(uint32_t regionId,
                              std::vector<::hirari::editing::AudioNoteSegment> segments) {
        if (segments.size() > 4096 || audioPitchAnalysisRunning()) return false;
        std::vector<::HirariAudioNoteSegmentValidation> validation;
        validation.reserve(segments.size());
        for (const auto& segment : segments) {
            validation.push_back({segment.startSeconds, segment.endSeconds,
                segment.detectedPitchCents, segment.pitchOffsetCents,
                segment.formantOffsetCents, segment.anchors.data(), segment.anchors.size()});
        }
        std::vector<uint32_t> orderedIndices(segments.size());
        if (!hirari_audio_note_validate_segments(validation.data(), validation.size(),
                                                  orderedIndices.data())) return false;
        std::vector<::hirari::editing::AudioNoteSegment> ordered;
        ordered.reserve(segments.size());
        for (uint32_t index : orderedIndices) {
            ordered.push_back(std::move(segments[index]));
        }
        segments = std::move(ordered);
        for (auto& segment : segments) segment.rebuildPitchRatioIntegral();
        ::hirari::editing::rebuildAudioNotePhasePrefixes(segments);
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end()) return false;
        it->audioNoteSegments = std::move(segments);
        publishRegionSnapshot();
        return true;
    }

    bool upsertAudioNoteAnchor(uint32_t regionId, double segmentStartSeconds,
                               ::hirari::editing::AudioNoteAnchor anchor) {
        if (audioPitchAnalysisRunning() || !std::isfinite(segmentStartSeconds) ||
            !std::isfinite(anchor.positionSeconds) ||
            !std::isfinite(anchor.pitchCents) ||
            !std::isfinite(anchor.formantCents)) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto regionIt = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (regionIt == m_regions.end()) return false;
        const auto ranges = ::hirari::editing::audioNoteSegmentRanges(regionIt->audioNoteSegments);
        const int64_t segmentIndex = hirari_audio_note_find_segment_for_edit(
            ranges.data(), ranges.size(), segmentStartSeconds);
        if (segmentIndex < 0) return false;
        auto segmentIt = regionIt->audioNoteSegments.begin() + segmentIndex;
        if (segmentIt->anchors.size() >= 4096 &&
            std::none_of(segmentIt->anchors.begin(), segmentIt->anchors.end(),
                         [position = anchor.positionSeconds](const auto& current) {
                             return std::abs(current.positionSeconds - position) < 1e-9;
                         })) return false;
        segmentIt->upsertAnchor(anchor);
        ::hirari::editing::rebuildAudioNotePhasePrefixes(regionIt->audioNoteSegments);
        publishRegionSnapshot();
        return true;
    }

    bool clearAudioNoteSegments(uint32_t regionId) {
        if (audioPitchAnalysisRunning()) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end() || it->audioNoteSegments.empty()) return false;
        it->audioNoteSegments.clear();
        publishRegionSnapshot();
        return true;
    }

    bool warpAudioNoteSegment(uint32_t regionId, double segmentStartSeconds,
                              double newStartSeconds, double newEndSeconds) {
        if (audioPitchAnalysisRunning() || !std::isfinite(segmentStartSeconds) || !std::isfinite(newStartSeconds) ||
            !std::isfinite(newEndSeconds) || newEndSeconds <= newStartSeconds) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto regionIt = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (regionIt == m_regions.end()) return false;
        const auto ranges = ::hirari::editing::audioNoteSegmentRanges(regionIt->audioNoteSegments);
        const int64_t segmentIndex = hirari_audio_note_find_segment_for_edit(
            ranges.data(), ranges.size(), segmentStartSeconds);
        if (segmentIndex < 0) return false;
        auto segmentIt = regionIt->audioNoteSegments.begin() + segmentIndex;
        if (!segmentIt->valid() || !hirari_audio_note_can_warp_segment(
                ranges.data(), ranges.size(), static_cast<size_t>(segmentIndex),
                newStartSeconds, newEndSeconds)) return false;
        if (!segmentIt->warpTiming(newStartSeconds, newEndSeconds)) return false;
        ::hirari::editing::rebuildAudioNotePhasePrefixes(regionIt->audioNoteSegments);
        publishRegionSnapshot();
        return true;
    }

    bool removeAudioNoteSegment(uint32_t regionId, double segmentStartSeconds) {
        if (audioPitchAnalysisRunning() || !std::isfinite(segmentStartSeconds)) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto regionIt = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (regionIt == m_regions.end()) return false;
        const auto before = regionIt->audioNoteSegments.size();
        regionIt->audioNoteSegments.erase(std::remove_if(regionIt->audioNoteSegments.begin(),
            regionIt->audioNoteSegments.end(), [segmentStartSeconds](const auto& segment) {
                return std::abs(segment.startSeconds - segmentStartSeconds) < 1e-9;
            }), regionIt->audioNoteSegments.end());
        if (regionIt->audioNoteSegments.size() == before) return false;
        ::hirari::editing::rebuildAudioNotePhasePrefixes(regionIt->audioNoteSegments);
        publishRegionSnapshot();
        return true;
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
        const size_t stride = std::max<size_t>(
            1, static_cast<size_t>(std::ceil(sourceSampleRate / 12000.0)));
        const size_t decimatedCount =
            (static_cast<size_t>(sourceSpan) - 1) / stride + 1;
        std::vector<float> decimated(decimatedCount);
        if (hirari_audio_pitch_select_and_decimate(
                channelData.data(), channelData.size(),
                static_cast<size_t>(analysisRegion.sourceOffset),
                static_cast<size_t>(sourceSpan), stride,
                analysisRegion.reverse ? 1 : 0,
                decimated.data(), decimated.size()) != decimated.size()) return false;
        // A 64–1200 Hz vocal range fits comfortably below the decimated
        // Nyquist rate. This keeps a multi-minute region practical while the
        // 85 ms window and 43 ms hop retain useful note-boundary resolution.
        config.window = 1024;
        config.hop = 512;
        auto detected = ::hirari::editing::AudioPitchAnalyzer::analyze(
            decimated.data(), decimated.size(),
            sourceSampleRate / static_cast<double>(stride), config);

        // Pitch analysis works in source playback order. Convert its note and
        // pitch-anchor positions back to the region timeline so trimmed,
        // reversed, and warped clips use the same coordinates as the renderer.
        std::vector<::HirariWarpMarker> pitchWarpMarkers;
        pitchWarpMarkers.reserve(analysisRegion.warpMarkers.size());
        for (const auto& marker : analysisRegion.warpMarkers) {
            pitchWarpMarkers.push_back({marker.sourceSample, marker.timelineSample, marker.transient});
        }
        const double sourceFramesPerTimelineFrame = std::isfinite(analysisRegion.warpRatio) &&
            analysisRegion.sourceFramesPerTimelineFrame() > 0.0
            ? analysisRegion.sourceFramesPerTimelineFrame() : 1.0;
        for (auto& segment : detected) {
            size_t anchorCount = segment.anchors.size();
            if (!hirari_audio_pitch_map_segment_to_timeline(
                    &segment.startSeconds, &segment.endSeconds,
                    segment.anchors.data(), &anchorCount,
                    sourceSampleRate, timelineSampleRate, sourceSpan, analysisRegion.len,
                    sourceFramesPerTimelineFrame, pitchWarpMarkers.data(), pitchWarpMarkers.size())) {
                return false;
            }
            segment.anchors.resize(anchorCount);
            segment.rebuildPitchRatioIntegral();
        }
        detected.erase(std::remove_if(detected.begin(), detected.end(),
            [](const auto& segment) { return !segment.valid(); }), detected.end());
        for (auto& segment : detected) segment.rebuildPitchRatioIntegral();
        ::hirari::editing::rebuildAudioNotePhasePrefixes(detected);
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
        it->audioNoteSegments = std::move(detected);
        publishRegionSnapshot();
        return true;
    }

    bool appendProcessingStep(uint32_t regionId, ::hirari::editing::EventProcessingStep step) {
        if (step.operation.empty() || step.operation.size() > 256 || step.id == 0) return false;
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) if (region.id == regionId) { if (region.processingHistory.size() >= 4096) return false; region.processingHistory.push_back(std::move(step)); publishRegionSnapshot(); return true; }
        return false;
    }
    bool clearProcessingHistory(uint32_t regionId) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) if (region.id == regionId) { region.processingHistory.clear(); publishRegionSnapshot(); return true; }
        return false;
    }
    // Legacy escape hatch: callers that mutate the returned vector must call
    // commitRegionEdits() before the next audio block. Engine code uses the
    // typed setters below so normal edits publish automatically.
    void commitRegionEdits() {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        publishRegionSnapshot();
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

    bool setRegionGain(uint32_t rid, float gain) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (!std::isfinite(gain)) return false;
        for (auto& region : m_regions) {
            if (region.id == rid) {
                if (region.locked) return false;
                region.clipGain = std::clamp(gain, 0.0f, 2.0f);
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool setRegionMuted(uint32_t rid, bool muted) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid) continue;
            if (region.locked) return false;
            region.muted = muted;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    bool setRegionCompRanges(
        uint32_t rid,
        const std::vector<Region::CompRange>& ranges,
        bool managed) {
        if (ranges.size() > 4096) return false;
        uint64_t previousEnd = 0;
        for (const auto& range : ranges) {
            if (range.start >= range.end || range.fadeIn > range.end - range.start ||
                range.fadeOut > range.end - range.start || range.start < previousEnd) {
                return false;
            }
            previousEnd = range.end;
        }
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid) continue;
            if (region.locked && managed) return false;
            if (std::any_of(ranges.begin(), ranges.end(), [&region](const auto& range) {
                    return range.end > region.len;
                })) {
                return false;
            }
            region.compManaged = managed;
            region.compRanges = ranges;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    bool setRegionFades(uint32_t rid, float fadeIn, float fadeOut) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (!std::isfinite(fadeIn) || !std::isfinite(fadeOut)) return false;
        for (auto& region : m_regions) {
            if (region.id == rid) {
                if (region.locked) return false;
                const float inNorm = std::clamp(fadeIn, 0.0f, 1.0f);
                const float outNorm = std::clamp(fadeOut, 0.0f, 1.0f);
                region.fadeInSamples = static_cast<uint64_t>(inNorm * region.len);
                region.fadeOutSamples = static_cast<uint64_t>(outNorm * region.len);
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool setRegionRangeEdit(uint32_t rid, uint64_t start, uint64_t end,
                            float gain, uint64_t fadeIn, uint64_t fadeOut) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked) continue;
            auto nativeEdits = copyRangeEditsToRust(region.rangeEdits);
            nativeEdits.reserve(std::max<size_t>(1, nativeEdits.size() + 1));
            const size_t count = hirari_region_range_edit_upsert(
                nativeEdits.data(), nativeEdits.size(), nativeEdits.capacity(), region.len,
                start, end, gain, fadeIn, fadeOut);
            if (count == static_cast<size_t>(-1)) return false;
            nativeEdits.resize(count);
            region.rangeEdits = copyRangeEditsFromRust(nativeEdits);
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    bool clearRegionRangeEdit(uint32_t rid, uint64_t start, uint64_t end) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked) continue;
            auto nativeEdits = copyRangeEditsToRust(region.rangeEdits);
            const size_t count = hirari_region_range_edit_remove(
                nativeEdits.data(), nativeEdits.size(), start, end);
            if (count == static_cast<size_t>(-1)) return false;
            nativeEdits.resize(count);
            region.rangeEdits = copyRangeEditsFromRust(nativeEdits);
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    std::vector<Region::RangeEdit> regionRangeEdits(uint32_t rid) const {
        for (const auto& region : m_regions) if (region.id == rid) return region.rangeEdits;
        return {};
    }

    bool replaceRegionRangeEdits(uint32_t rid, std::vector<Region::RangeEdit> edits) {
        if (edits.size() > 4096) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked) continue;
            auto nativeEdits = copyRangeEditsToRust(edits);
            if (!hirari_region_range_edits_replace(
                    nativeEdits.data(), nativeEdits.size(), region.len)) return false;
            region.rangeEdits = copyRangeEditsFromRust(nativeEdits);
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    bool clearRegionRangeEdits(uint32_t rid) {
        return replaceRegionRangeEdits(rid, {});
    }

    bool setRegionReverse(uint32_t rid, bool reverse) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id == rid) {
                if (region.locked) return false;
                region.reverse = reverse;
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool setRegionWarpRatio(uint32_t rid, double ratio) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (!std::isfinite(ratio) || ratio < 0.5 || ratio > 2.0) return false;
        for (auto& region : m_regions) {
            if (region.id == rid) {
                if (region.locked) return false;
                const auto input = regionGeometryForRust(region);
                HirariNativeRegionGeometry output{};
                if (!hirari_region_set_warp_ratio_geometry(&input, ratio, &output)) return false;
                region.warpRatio = output.warp_ratio;
                region.warpMarkers.clear();
                region.start = output.start;
                region.len = output.length;
                region.fadeInSamples = output.fade_in;
                region.fadeOutSamples = output.fade_out;
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool setRegionPitchPreserveWarp(uint32_t rid, bool enabled) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id == rid) {
                if (region.locked) return false;
                if (region.pitchPreserveWarp != enabled) {
                    if (region.sourceLength == 0) region.sourceLength = region.len;
                    region.pitchPreserveWarp = enabled;
                    publishRegionSnapshot();
                }
                return true;
            }
        }
        return false;
    }

    std::optional<bool> getRegionPitchPreserveWarp(uint32_t rid) const {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (const auto& region : m_regions) {
            if (region.id == rid) return region.pitchPreserveWarp;
        }
        return std::nullopt;
    }

    std::optional<double> getRegionWarpRatio(uint32_t rid) const {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (const auto& region : m_regions) {
            if (region.id == rid) return region.warpRatio;
        }
        return std::nullopt;
    }

    bool setRegionPitchSemitones(uint32_t rid, float semitones) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (!std::isfinite(semitones) || semitones < -24.0f || semitones > 24.0f) return false;
        for (auto& region : m_regions) {
            if (region.id == rid) {
                if (region.locked) return false;
                region.pitchSemitones = semitones;
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    std::optional<float> getRegionPitchSemitones(uint32_t rid) const {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (const auto& region : m_regions) {
            if (region.id == rid) return region.pitchSemitones;
        }
        return std::nullopt;
    }

    bool setRegionLoopCount(uint32_t rid, uint32_t count) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (count == 0 || count > 1024) return false;
        for (auto& region : m_regions) {
            if (region.id == rid && !region.locked && region.len <= UINT64_MAX / count &&
                region.start <= UINT64_MAX - region.len * count) {
                region.loopCount = count;
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool setRegionLocked(uint32_t rid, bool locked) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r){ return r.id == rid; });
        if (it == m_regions.end()) return false; it->locked = locked; publishRegionSnapshot(); return true;
    }
    bool setRegionSyncGroup(uint32_t rid, uint32_t group) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r){ return r.id == rid; });
        if (it == m_regions.end() || it->locked) return false;
        if (it->syncGroup != group) {
            it->syncGroup = group;
            publishRegionSnapshot();
        }
        return true;
    }

    bool setRegionWarpMarkers(uint32_t rid, std::vector<Region::WarpMarker> markers) {
        if (markers.size() > 100'000) return false;
        const size_t orderedCount = hirari_region_normalize_warp_markers(
            markers.data(), markers.size(), UINT64_MAX, UINT64_MAX);
        if (orderedCount != markers.size()) return false;
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r) {
            return r.id == rid;
        });
        if (it == m_regions.end() || it->locked) return false;
        const uint64_t sourceSpan = it->sourceLength > 0 ? it->sourceLength : it->len;
        const size_t boundedCount = hirari_region_normalize_warp_markers(
            markers.data(), markers.size(), sourceSpan, it->len);
        if (boundedCount != markers.size()) return false;
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

    std::vector<Region::WarpMarker> getRegionWarpMarkers(uint32_t rid) const {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        const auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r) {
            return r.id == rid;
        });
        return it == m_regions.end() ? std::vector<Region::WarpMarker>{} : it->warpMarkers;
    }

    std::optional<uint32_t> getRegionSyncGroup(uint32_t rid) const {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (const auto& region : m_regions) {
            if (region.id == rid) return region.syncGroup;
        }
        return std::nullopt;
    }

    std::optional<uint32_t> getRegionLoopCount(uint32_t rid) const {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (const auto& region : m_regions) {
            if (region.id == rid) return region.loopCount;
        }
        return std::nullopt;
    }

    bool setRegionTrim(uint32_t rid, float startNorm, float endNorm) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked || region.baseLength == 0) continue;
            const auto input = regionGeometryForRust(region);
            HirariNativeRegionGeometry output{};
            if (!hirari_region_set_trim_geometry(&input, startNorm, endNorm, &output)) return false;
            region.start = output.start;
            region.sourceOffset = output.source_offset;
            region.sourceLength = output.source_length;
            region.len = output.length;
            region.warpMarkers.clear();
            region.fadeInSamples = output.fade_in;
            region.fadeOutSamples = output.fade_out;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    void recordVolumeAutomation(float value, double time) {
        invalidateFrozenForEdit();
        m_volume.store(std::isfinite(value) ? std::clamp(value, 0.0f, 2.0f) : 1.0f,
                       std::memory_order_release);
        m_lastAutomationTime = time;
    }

    // Audio-thread producer for write automation. The queue is deliberately
    // bounded and SPSC: recording a control surface gesture must never grow a
    // vector, take an edit lock, or allocate inside processBlock().
