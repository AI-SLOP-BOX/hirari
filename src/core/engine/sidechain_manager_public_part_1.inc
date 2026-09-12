public:
    struct LinkSnapshot {
        const float* left = nullptr;
        const float* right = nullptr;
        uint32_t sourceTrackId = 0;
        float level = 1.0f;
        SidechainTapPoint tapPoint = SidechainTapPoint::PostFX;
        uint32_t frames = 0;
        uint32_t sampleRate = 0;
        uint64_t sourceGeneration = 0;
        uint64_t generation = 0;
    };

    // Control-plane backup used when a track is temporarily removed by an
    // undoable project edit. Audio data is copied so restoration never relies
    // on a source track's lifetime or a stale callback buffer.
    struct LinkState {
        uint32_t destinationTrackId = 0;
        uint32_t pluginIndex = 0;
        uint32_t sourceTrackId = 0;
        float level = 1.0f;
        SidechainTapPoint tapPoint = SidechainTapPoint::PostFX;
        uint32_t sampleRate = 0;
        uint64_t sourceGeneration = 0;
        std::vector<float> left;
        std::vector<float> right;
    };

    SidechainManager() = default;
    static SidechainManager& getInstance() { static SidechainManager i; return i; }

    static constexpr uint32_t kMaxLinks = 256;
    static constexpr uint32_t kMaxBlockSize = 4096;

    // Legacy control-plane entry points. Buffer publication is performed by
    // registerLink(), because these older methods did not carry buffer data.
    bool registerSource(uint32_t trackId) noexcept {
        (void)trackId;
        m_legacyApiMisuse.store(true, std::memory_order_release);
        return false;
    }

    bool route(uint32_t sourceTrackId, uint32_t destTrackId,
               uint32_t pluginIdx) noexcept {
        (void)sourceTrackId;
        (void)destTrackId;
        (void)pluginIdx;
        m_legacyApiMisuse.store(true, std::memory_order_release);
        return false;
    }

    // Control-thread API. Registration copies the source block into fixed
    // storage; the audio thread never observes the caller's buffer address.
    bool registerLink(uint32_t destTrackId, uint32_t pluginIdx,
                      const SidechainLink& link) noexcept {
        ControlMutationGuard mutation(*this);
        if (pluginIdx >= kMaxPluginIndex ||
            link.sourceBufferL == nullptr || link.sourceBufferR == nullptr ||
            link.sourceFrames == 0 || link.sourceFrames > kMaxBlockSize ||
            link.sourceSampleRate == 0 || link.sourceGeneration == 0 ||
            !std::isfinite(link.level)) {
            return false;
        }

        if (link.sourceFrames > kMaxBlockSize) return false;
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        Slot* freeSlot = nullptr;
        for (auto& slot : m_slots) {
            const uint64_t current = slot.key.load(std::memory_order_acquire);
            if (current == key) {
                beginWrite(slot);
                const bool published = publish(slot, key, link);
                endWrite(slot);
                return published;
            }
            if (current == kEmptyKey && freeSlot == nullptr) freeSlot = &slot;
        }
        if (freeSlot == nullptr) return false;
        beginWrite(*freeSlot);
        const bool published = publish(*freeSlot, key, link);
        endWrite(*freeSlot);
        return published;
    }

    // Control-plane link publication for a graph that has not been prepared
    // by the audio device yet.  The logical route must still be serializable
    // and queryable before the first callback; publish an owned silent block
    // until controlRefreshSourceBuffer() receives the real source block.
    bool registerSilentLink(uint32_t destTrackId, uint32_t pluginIdx,
                            uint32_t sourceTrackId, uint32_t frames,
                            uint32_t sampleRate, uint64_t sourceGeneration,
                            float level, SidechainTapPoint tapPoint) noexcept {
        ControlMutationGuard mutation(*this);
        if (pluginIdx >= kMaxPluginIndex || frames == 0 || frames > kMaxBlockSize ||
            sampleRate == 0 || sourceGeneration == 0 || !std::isfinite(level)) {
            return false;
        }
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        Slot* target = nullptr;
        for (auto& slot : m_slots) {
            const uint64_t current = slot.key.load(std::memory_order_acquire);
            if (current == key) { target = &slot; break; }
            if (current == kEmptyKey && target == nullptr) target = &slot;
        }
        if (target == nullptr) return false;
        beginWrite(*target);
        const bool published = publishSilent(*target, key, sourceTrackId, frames,
                                             sampleRate, sourceGeneration, level, tapPoint);
        endWrite(*target);
        return published;
    }

    bool removeLink(uint32_t destTrackId, uint32_t pluginIdx) noexcept {
        ControlMutationGuard mutation(*this);
        if (pluginIdx >= kMaxPluginIndex) return false;
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        for (auto& slot : m_slots) {
            if (slot.key.load(std::memory_order_acquire) == key) {
                beginWrite(slot);
                slot.key.store(kEmptyKey, std::memory_order_release);
                slot.left.store(nullptr, std::memory_order_relaxed);
                slot.right.store(nullptr, std::memory_order_relaxed);
                slot.owned.store(false, std::memory_order_relaxed);
                slot.sourceFrames.store(0, std::memory_order_relaxed);
                slot.sourceSampleRate.store(0, std::memory_order_relaxed);
                slot.sourceGeneration.store(0, std::memory_order_relaxed);
                slot.generation.store(0, std::memory_order_relaxed);
                for (uint32_t i = 0; i < 2; ++i) {
                    slot.ownedFrames[i].store(0, std::memory_order_relaxed);
                    slot.ownedSampleRates[i].store(0, std::memory_order_relaxed);
                    slot.ownedSourceGenerations[i].store(0, std::memory_order_relaxed);
                    slot.ownedGenerations[i].store(0, std::memory_order_relaxed);
                }
                endWrite(slot);
                return true;
            }
        }
        return false;
    }

    bool hasLink(uint32_t destTrackId, uint32_t pluginIdx,
                uint32_t sourceTrackId) const noexcept {
        if (pluginIdx >= kMaxPluginIndex) return false;
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        for (const auto& slot : m_slots) {
            if (slot.key.load(std::memory_order_acquire) == key) {
                return slot.sourceTrackId.load(std::memory_order_relaxed) == sourceTrackId;
            }
        }
        return false;
    }

    void removeLinksForTrack(uint32_t trackId) noexcept {
        ControlMutationGuard mutation(*this);
        for (auto& slot : m_slots) {
            const uint64_t key = slot.key.load(std::memory_order_acquire);
            const uint32_t destination = static_cast<uint32_t>(key >> 32);
            if (key != kEmptyKey && (destination == trackId ||
                                     slot.sourceTrackId.load(std::memory_order_relaxed) == trackId)) {
                beginWrite(slot);
                slot.key.store(kEmptyKey, std::memory_order_release);
                slot.left.store(nullptr, std::memory_order_relaxed);
                slot.right.store(nullptr, std::memory_order_relaxed);
                slot.sourceTrackId.store(0, std::memory_order_relaxed);
                slot.owned.store(false, std::memory_order_relaxed);
                slot.sourceFrames.store(0, std::memory_order_relaxed);
                slot.sourceSampleRate.store(0, std::memory_order_relaxed);
                slot.sourceGeneration.store(0, std::memory_order_relaxed);
                slot.generation.store(0, std::memory_order_relaxed);
                endWrite(slot);
            }
        }
    }

    std::vector<LinkState> captureLinksForTrack(uint32_t trackId) {
        std::vector<LinkState> result;
        if (trackId == 0) return result;
        // Use the same admission/reader protocol as other control mutations.
        // A writer mutex by itself would still allow an in-flight audio reader
        // or publisher to flip the active double-buffer while it is copied.
        ControlMutationGuard mutation(*this);
        for (const auto& slot : m_slots) {
            const uint64_t key = slot.key.load(std::memory_order_acquire);
            const uint32_t destination = static_cast<uint32_t>(key >> 32);
            const uint32_t source = slot.sourceTrackId.load(std::memory_order_relaxed);
            if (key == kEmptyKey || (destination != trackId && source != trackId) ||
                !slot.owned.load(std::memory_order_relaxed)) {
                continue;
            }
            const uint32_t buffer = slot.publishedBuffer.load(std::memory_order_relaxed);
            if (buffer > 1 || !slot.ownedLeft[buffer] || !slot.ownedRight[buffer]) continue;
            const uint32_t frames = slot.ownedFrames[buffer].load(std::memory_order_relaxed);
            if (frames == 0 || frames > kMaxBlockSize) continue;
            LinkState state;
            state.destinationTrackId = destination;
            state.pluginIndex = static_cast<uint32_t>(key & 0xFFFFFFFFu);
            state.sourceTrackId = source;
            state.level = slot.level.load(std::memory_order_relaxed);
            state.tapPoint = static_cast<SidechainTapPoint>(slot.tapPoint.load(std::memory_order_relaxed));
            state.sampleRate = slot.ownedSampleRates[buffer].load(std::memory_order_relaxed);
            state.sourceGeneration = slot.ownedSourceGenerations[buffer].load(std::memory_order_relaxed);
            state.left.assign(slot.ownedLeft[buffer]->begin(), slot.ownedLeft[buffer]->begin() + frames);
            state.right.assign(slot.ownedRight[buffer]->begin(), slot.ownedRight[buffer]->begin() + frames);
            result.push_back(std::move(state));
        }
        return result;
    }

    void restoreLinks(const std::vector<LinkState>& states) noexcept {
        for (const auto& state : states) {
            if (state.destinationTrackId == 0 || state.sourceTrackId == 0 ||
                state.left.empty() || state.left.size() != state.right.size() ||
                state.left.size() > kMaxBlockSize || state.sampleRate == 0 ||
                state.sourceGeneration == 0) {
                continue;
            }
            SidechainLink link;
            link.sourceBufferL = state.left.data();
            link.sourceBufferR = state.right.data();
            link.sourceTrackId = state.sourceTrackId;
            link.level = state.level;
            link.tapPoint = state.tapPoint;
            link.sourceFrames = static_cast<uint32_t>(state.left.size());
            link.sourceSampleRate = state.sampleRate;
            link.sourceGeneration = state.sourceGeneration;
            (void)registerLink(state.destinationTrackId, state.pluginIndex, link);
        }
    }

    void resetForProject() noexcept {
        ControlMutationGuard mutation(*this);
        m_legacyApiMisuse.store(false, std::memory_order_release);
        for (auto& slot : m_slots) {
            beginWrite(slot);
            slot.key.store(kEmptyKey, std::memory_order_release);
            slot.left.store(nullptr, std::memory_order_relaxed);
            slot.right.store(nullptr, std::memory_order_relaxed);
            slot.owned.store(false, std::memory_order_relaxed);
            slot.sourceFrames.store(0, std::memory_order_relaxed);
            slot.sourceSampleRate.store(0, std::memory_order_relaxed);
            slot.sourceGeneration.store(0, std::memory_order_relaxed);
            slot.generation.store(0, std::memory_order_relaxed);
            slot.sourceTrackId.store(0, std::memory_order_relaxed);
            slot.level.store(1.0f, std::memory_order_relaxed);
            slot.tapPoint.store(static_cast<uint8_t>(SidechainTapPoint::PostFX),
                                std::memory_order_relaxed);
            slot.publishedBuffer.store(0, std::memory_order_relaxed);
            for (uint32_t i = 0; i < 2; ++i) {
                slot.ownedFrames[i].store(0, std::memory_order_relaxed);
                slot.ownedSampleRates[i].store(0, std::memory_order_relaxed);
                slot.ownedSourceGenerations[i].store(0, std::memory_order_relaxed);
                slot.ownedGenerations[i].store(0, std::memory_order_relaxed);
            }
            endWrite(slot);
        }
    }


    /**
     * @brief GET: Lock-free, allocation-free bounded access to a sidechain signal.
     */
    // Deliberately no pointer-returning API: a published double-buffer can be
    // reused by a later control publication. Consumers must copy one coherent
    // block into callback-owned storage and use it for that callback only.
    bool copySidechainBlock(uint32_t destTrackId, uint32_t pluginIdx,
                            float* left, float* right, uint32_t capacity,
                            LinkSnapshot& metadata) const noexcept {
        if (left == nullptr || right == nullptr || capacity == 0) return false;
        // The snapshot contains pointers into one of the manager-owned
        // buffers. Keep the reader registered until the memcpy is complete;
        // otherwise a control writer could publish twice and reuse the buffer
        // while this callback is still copying it.
        ReaderGuard reader(*this);
        LinkSnapshot snapshot{};
        if (!readSnapshot(destTrackId, pluginIdx, snapshot) ||
            snapshot.frames == 0) return false;
        const uint32_t copyFrames = std::min(snapshot.frames, capacity);
        std::memcpy(left, snapshot.left, copyFrames * sizeof(float));
        std::memcpy(right, snapshot.right, copyFrames * sizeof(float));
        metadata = snapshot;
        metadata.left = left;
        metadata.right = right;
        metadata.frames = copyFrames;
        return true;
    }

private:
