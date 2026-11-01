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
                    (void)hirari_region_stretch_prepare(region.timeStretch.get(), sr, blockSize);
            }
        }
        hirari_channel_strip_set_sample_rate(m_strip, sr);
        hirari_track_holographic_panner_set_sample_rate(m_spatialPanner, sr);
        // Preparing a track is also the boundary used by offline renders and
        // device reinitialisation. Snap smoothed channel-strip state to its
        // current targets so the first block is deterministic after reload.
        hirari_channel_strip_reset(m_strip);
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
        hirari_channel_strip_reset(m_strip);
        m_effectChain.reset();
        hirari_track_automation_store_reset_hints(m_automationStore);
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
        size_t volumeCount = 0;
        size_t panCount = 0;
        const auto* volumeAutomation = static_cast<const Hirari::Core::ProjectAutomationPoint*>(
            hirari_track_automation_store_points(m_automationStore, 0, &volumeCount));
        const auto* panAutomation = static_cast<const Hirari::Core::ProjectAutomationPoint*>(
            hirari_track_automation_store_points(m_automationStore, 1, &panCount));
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
            volumeAutomation && volumeCount != 0 ? volumeAutomation : nullptr,
            volumeCount, hirari_track_automation_store_hint(m_automationStore, 0),
            panAutomation && panCount != 0 ? panAutomation : nullptr,
            panCount, hirari_track_automation_store_hint(m_automationStore, 1),
            m_strip, buffer.getArrayOfWritePointers(), buffer.getNumChannels(),
            validPreInsert ? m_preInsertBuffer.getArrayOfWritePointers() : nullptr,
            validPreInsert ? m_preInsertBuffer.getNumChannels() : 0,
            validSendPre ? m_sendPreFaderBuffer.getArrayOfWritePointers() : nullptr,
            validSendPre ? m_sendPreFaderBuffer.getNumChannels() : 0,
            validSendPost ? m_sendPostFaderBuffer.getArrayOfWritePointers() : nullptr,
            validSendPost ? m_sendPostFaderBuffer.getNumChannels() : 0,
            m_spatialPanner, 0,
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
        if (!left || !right || taps == 0 || taps > kMaxHrtfTaps) return false;
        invalidateFrozenForEdit();
        const bool changed =
            hirari_track_holographic_panner_set_kernel(m_spatialPanner, left, right, taps);
        if (changed) markProjectLayoutChanged();
        return changed;
    }
    void clearHrtfKernel() {
        invalidateFrozenForEdit();
        hirari_track_holographic_panner_clear_kernel(m_spatialPanner);
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
        return it->noteStore ? hirari_region_vari_audio_store_clone(it->noteStore.get()) : nullptr;
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
            if (!mutate(context, region.noteStore.get())) return false;
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
    // Commits a Rust F0 analysis result into the region's note table while
    // the analysis job holds the gate; the gate only keeps other editors out.
    bool commitAudioPitchAnalysis(uint32_t regionId, uint64_t expectedGeneration,
                                  void* analysis) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (regionGeneration() != expectedGeneration || !analysis) return false;
        for (auto& region : m_regions) {
            if (region.id != regionId) continue;
            if (!hirari_audio_pitch_commit_region_result(analysis, region.noteStore.get()))
                return false;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    bool appendProcessingStep(uint32_t regionId, uint32_t stepId, const std::string& operation,
                              float parameter, bool enabled) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) if (region.id == regionId) {
            if (!region.appendProcessingStep(stepId, operation, parameter, enabled)) return false;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }
    bool clearProcessingHistory(uint32_t regionId) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) if (region.id == regionId) {
            if (!region.processingStore ||
                hirari_region_processing_history_count(region.processingStore.get()) == 0) {
                return false;
            }
            hirari_region_processing_history_reset(region.processingStore.get());
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
            (void)hirari_region_clip_edits_set_warp_markers(
                region.clipEdits.get(), markers, markerCount, 0, 0, false);
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
    bool replaceRegionCompRanges(uint32_t rid, const HirariRegionCompRange* ranges,
                                 size_t count, bool managed) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid) continue;
            if (!hirari_region_clip_edits_replace_comp_ranges(
                    region.clipEdits.get(), ranges, count, managed)) return false;
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    std::vector<Region::RangeEdit> regionRangeEdits(uint32_t rid) const {
        for (const auto& region : m_regions) if (region.id == rid) {
            const auto* view = region.clip();
            if (view->rangeEditCount == 0) return {};
            return {view->rangeEdits, view->rangeEdits + view->rangeEditCount};
        }
        return {};
    }

    // `validate` re-admits the replacement batch; without it Rust already
    // produced this sorted, bounded list.
    bool replaceRegionRangeEdits(uint32_t rid, const Region::RangeEdit* edits,
                                 size_t count, bool validate = true) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked) continue;
            if (!hirari_region_clip_edits_replace_range_edits(
                    region.clipEdits.get(), edits, count, region.len, validate)) {
                return false;
            }
            publishRegionSnapshot();
            return true;
        }
        return false;
    }

    // Rejects the edit when normalization would drop any marker, matching the
    // previous contract. An unchanged list is accepted without republishing.
    bool setRegionWarpMarkers(uint32_t rid, const HirariWarpMarker* markers, size_t count) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r) {
            return r.id == rid;
        });
        if (it == m_regions.end() || it->locked) return false;
        const uint64_t sourceSpan = it->sourceLength > 0 ? it->sourceLength : it->len;
        const int32_t applied = hirari_region_clip_edits_set_warp_markers(
            it->clipEdits.get(), markers, count, sourceSpan, it->len, true);
        if (applied < 0) return false;
        if (applied > 0) publishRegionSnapshot();
        return true;
    }

    // Audio-thread producer for write automation. The queue is deliberately
    // bounded and SPSC: recording a control surface gesture must never grow a
    // vector, take an edit lock, or allocate inside processBlock().
