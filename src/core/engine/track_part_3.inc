    void setEq(float lowBoostDb, float lowCutDb, float highBoostDb, float highCutDb) {
        const auto gain = [](float boost, float cut) {
            if (!std::isfinite(boost) || !std::isfinite(cut)) return 1.0f;
            const float db = std::clamp(boost - cut, -120.0f, 24.0f);
            const float value = std::pow(10.0f, db / 20.0f);
            return std::isfinite(value) ? value : 1.0f;
        };
        m_lowGain = gain(lowBoostDb, lowCutDb);
        m_highGain = gain(highBoostDb, highCutDb);
    }
    virtual void prepareToPlay(double sr, uint32_t blockSize) {
        if (!std::isfinite(sr) || sr <= 0.0 || blockSize == 0) return;
        m_sampleRate = sr;
        m_blockSize = blockSize;
        m_isPrepared = true;
        m_strip.setSampleRate(sr);
        m_spatialPanner.setSampleRate(sr);
        // Preparing a track is also the boundary used by offline renders and
        // device reinitialisation. Snap smoothed channel-strip state to its
        // current targets so the first block is deterministic after reload.
        m_strip.reset();
        m_activeBuffer.resize(2, blockSize);
        m_preFaderBuffer.resize(2, blockSize);
        m_preInsertBuffer.resize(2, blockSize);
        m_effectChain.setSampleRate(sr, blockSize);
    }
    void resetForOfflineRender() noexcept {
        m_strip.reset();
        m_effectChain.reset();
        m_volumeAutomationHint = 0;
        m_panAutomationHint = 0;
        m_trackDelayAutomationHint = 0;
        std::fill(m_pdcDelayL.begin(), m_pdcDelayL.end(), 0.0f);
        std::fill(m_pdcDelayR.begin(), m_pdcDelayR.end(), 0.0f);
        m_pdcWrite = 0;
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
    void invalidateFrozenForEdit() noexcept { clearFrozenAudio(); }
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
        return true;
    }

    // Offline render taps are prepared on the control thread and only copied
    // on the audio side. They let the render graph select a real pre-fader or
    // pre-insert signal without changing the interactive mix state.
    const AudioBuffer& offlinePreFaderBuffer() const noexcept { return m_preFaderBuffer; }
    const AudioBuffer& offlinePreInsertBuffer() const noexcept { return m_preInsertBuffer; }
    void setSpatialMode(uint32_t mode) {
        invalidateFrozenForEdit();
        m_spatialMode.store(mode <= 2 ? mode : 0, std::memory_order_relaxed);
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
    }
    float getSpatialX() const { return m_spatialX.load(std::memory_order_relaxed); }
    float getSpatialY() const { return m_spatialY.load(std::memory_order_relaxed); }
    float getSpatialZ() const { return m_spatialZ.load(std::memory_order_relaxed); }
    bool isMuted() const { return m_muted.load(std::memory_order_relaxed); }
    void setMuted(bool muted) { invalidateFrozenForEdit(); m_muted.store(muted, std::memory_order_relaxed); }
    bool isPhaseInverted() const { return m_phaseInvert.load(std::memory_order_relaxed); }
    void setPhaseInverted(bool inverted) {
        invalidateFrozenForEdit();
        m_phaseInvert.store(inverted, std::memory_order_relaxed);
    }
    void setSolo(bool solo) { invalidateFrozenForEdit(); m_solo.store(solo, std::memory_order_relaxed); }
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
    std::vector<Region>& getRegionsMutable() { return m_regions; }
    void clearRegions() {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        m_regions.clear();
        publishRegionSnapshot();
    }

    bool upsertAudioNoteSegment(uint32_t regionId, ::aura::editing::AudioNoteSegment segment) {
        if (!segment.valid()) return false;
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end()) return false;
        auto segmentIt = std::find_if(it->audioNoteSegments.begin(), it->audioNoteSegments.end(),
            [&](const auto& current) { return std::abs(current.startSeconds - segment.startSeconds) < 1e-9; });
        if (segmentIt == it->audioNoteSegments.end()) it->audioNoteSegments.push_back(std::move(segment));
        else *segmentIt = std::move(segment);
        std::sort(it->audioNoteSegments.begin(), it->audioNoteSegments.end(),
            [](const auto& a, const auto& b) { return a.startSeconds < b.startSeconds; });
        publishRegionSnapshot();
        return true;
    }

    bool upsertAudioNoteAnchor(uint32_t regionId, double segmentStartSeconds,
                               ::aura::editing::AudioNoteAnchor anchor) {
        if (!std::isfinite(segmentStartSeconds) ||
            !std::isfinite(anchor.positionSeconds) ||
            !std::isfinite(anchor.pitchCents) ||
            !std::isfinite(anchor.formantCents)) return false;
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto regionIt = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (regionIt == m_regions.end()) return false;
        auto segmentIt = std::find_if(regionIt->audioNoteSegments.begin(),
                                      regionIt->audioNoteSegments.end(),
            [segmentStartSeconds](const auto& segment) {
                return std::abs(segment.startSeconds - segmentStartSeconds) < 1e-9;
            });
        if (segmentIt == regionIt->audioNoteSegments.end()) return false;
        if (segmentIt->anchors.size() >= 4096 &&
            std::none_of(segmentIt->anchors.begin(), segmentIt->anchors.end(),
                         [position = anchor.positionSeconds](const auto& current) {
                             return std::abs(current.positionSeconds - position) < 1e-9;
                         })) return false;
        segmentIt->upsertAnchor(anchor);
        publishRegionSnapshot();
        return true;
    }

    bool clearAudioNoteSegments(uint32_t regionId) {
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end() || it->audioNoteSegments.empty()) return false;
        it->audioNoteSegments.clear();
        publishRegionSnapshot();
        return true;
    }

    // Offline VariAudio-style F0 detection. This is deliberately a control/
    // analysis-thread operation; the audio callback only consumes the
    // immutable snapshot published below.
    bool analyzeAudioNoteSegments(uint32_t regionId,
                                  double sampleRate,
                                  ::aura::editing::AudioPitchAnalyzer::Config config = {}) {
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
        std::shared_ptr<AudioBuffer> audio;
        {
            std::lock_guard<std::mutex> editLock(m_regionEditMutex);
            auto it = std::find_if(m_regions.begin(), m_regions.end(),
                [regionId](const Region& region) { return region.id == regionId; });
            if (it == m_regions.end() || !it->audio || it->audio->getNumSamples() == 0 ||
                it->audio->getNumSamples() > 16'000'000 || it->audio->getNumChannels() == 0) {
                return false;
            }
            audio = it->audio;
        }
        const float* mono = audio->getReadPointer(0);
        if (!mono) return false;
        auto detected = ::aura::editing::AudioPitchAnalyzer::analyze(
            mono, audio->getNumSamples(), sampleRate, config);
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(),
            [regionId](const Region& region) { return region.id == regionId; });
        if (it == m_regions.end()) return false;
        it->audioNoteSegments = std::move(detected);
        publishRegionSnapshot();
        return true;
    }

    bool appendProcessingStep(uint32_t regionId, ::aura::editing::EventProcessingStep step) {
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
                region.warpRatio = ratio;
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
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
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::find_if(m_regions.begin(), m_regions.end(), [rid](const Region& r){ return r.id == rid; });
        if (it == m_regions.end()) return false; it->syncGroup = group; publishRegionSnapshot(); return true;
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
        if (!std::isfinite(startNorm) || !std::isfinite(endNorm)) return false;
        startNorm = std::clamp(startNorm, 0.0f, 1.0f);
        endNorm = std::clamp(endNorm, 0.0f, 1.0f);
        if (endNorm <= startNorm) return false;
        for (auto& region : m_regions) {
            if (region.id != rid || region.locked || region.baseLength == 0) continue;
            const uint64_t offset = static_cast<uint64_t>(region.baseLength * startNorm);
            const uint64_t end = static_cast<uint64_t>(region.baseLength * endNorm);
            if (end <= offset || region.baseStart > UINT64_MAX - offset ||
                region.baseSourceOffset > UINT64_MAX - offset) return false;
            region.start = region.baseStart + offset;
            region.sourceOffset = region.baseSourceOffset + offset;
            region.len = end - offset;
            region.fadeInSamples = std::min(region.fadeInSamples, region.len);
            region.fadeOutSamples = std::min(region.fadeOutSamples, region.len);
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
