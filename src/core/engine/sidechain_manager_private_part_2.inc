    static constexpr uint32_t kMaxPluginIndex = 65535;
    static constexpr uint64_t kEmptyKey = UINT64_MAX;

    struct Slot {
        std::atomic<uint64_t> key{kEmptyKey};
        std::atomic<uint64_t> version{0};
        std::atomic<const float*> left{nullptr};
        std::atomic<const float*> right{nullptr};
        std::array<std::unique_ptr<std::array<float, kMaxBlockSize>>, 2> ownedLeft{};
        std::array<std::unique_ptr<std::array<float, kMaxBlockSize>>, 2> ownedRight{};
        std::atomic<uint32_t> publishedBuffer{0};
        std::atomic<uint32_t> sourceFrames{0};
        std::array<std::atomic<uint32_t>, 2> ownedFrames{{0, 0}};
        std::array<std::atomic<uint32_t>, 2> ownedSampleRates{{0, 0}};
        std::array<std::atomic<uint64_t>, 2> ownedSourceGenerations{{0, 0}};
        std::array<std::atomic<uint64_t>, 2> ownedGenerations{{0, 0}};
        std::atomic<uint32_t> sourceSampleRate{0};
        std::atomic<uint64_t> sourceGeneration{0};
        std::atomic<uint64_t> generation{0};
        std::atomic<bool> owned{false};
        std::atomic<uint32_t> sourceTrackId{0};
        std::atomic<float> level{1.0f};
        std::atomic<uint8_t> tapPoint{static_cast<uint8_t>(SidechainTapPoint::PostFX)};
    };

    struct ReaderGuard {
        explicit ReaderGuard(const SidechainManager& owner) noexcept : m_owner(owner) {
            m_owner.m_activeReaders.fetch_add(1, std::memory_order_acq_rel);
        }
        ~ReaderGuard() {
            m_owner.m_activeReaders.fetch_sub(1, std::memory_order_release);
        }
        ReaderGuard(const ReaderGuard&) = delete;
        ReaderGuard& operator=(const ReaderGuard&) = delete;
        const SidechainManager& m_owner;
    };

    // A control mutation owns the writer mutex and closes audio publication
    // for the entire transaction, not just one slot. This is important for
    // refresh/remove operations that touch multiple links: an audio writer
    // must not interleave between two slot publications.
    struct ControlMutationGuard {
        explicit ControlMutationGuard(SidechainManager& owner) noexcept
            : m_owner(owner), m_lock(owner.m_writerMutex) {
            m_owner.m_controlMutation.store(true, std::memory_order_release);
            while (m_owner.m_audioPublishing.load(std::memory_order_acquire) ||
                   m_owner.m_activeReaders.load(std::memory_order_acquire) != 0) {
                std::this_thread::yield();
            }
        }
        ~ControlMutationGuard() {
            m_owner.m_controlMutation.store(false, std::memory_order_release);
        }
        ControlMutationGuard(const ControlMutationGuard&) = delete;
        ControlMutationGuard& operator=(const ControlMutationGuard&) = delete;
        SidechainManager& m_owner;
        std::unique_lock<std::mutex> m_lock;
    };

    static uint64_t makeKey(uint32_t destTrackId, uint32_t pluginIdx) noexcept {
        return (static_cast<uint64_t>(destTrackId) << 32) |
               static_cast<uint64_t>(pluginIdx);
    }

    static bool publish(Slot& slot, uint64_t key, const SidechainLink& link) noexcept {
        if (link.sourceFrames > 0) {
            try {
                if (!slot.ownedLeft[0] || !slot.ownedLeft[1] ||
                    !slot.ownedRight[0] || !slot.ownedRight[1]) {
                    slot.ownedLeft[0] = std::make_unique<std::array<float, kMaxBlockSize>>();
                    slot.ownedLeft[1] = std::make_unique<std::array<float, kMaxBlockSize>>();
                    slot.ownedRight[0] = std::make_unique<std::array<float, kMaxBlockSize>>();
                    slot.ownedRight[1] = std::make_unique<std::array<float, kMaxBlockSize>>();
                }
            } catch (...) {
                return false;
            }
            const uint32_t active = slot.publishedBuffer.load(std::memory_order_relaxed);
            const uint32_t next = 1u - active;
            std::memcpy(slot.ownedLeft[next]->data(), link.sourceBufferL,
                        link.sourceFrames * sizeof(float));
            std::memcpy(slot.ownedRight[next]->data(), link.sourceBufferR,
                        link.sourceFrames * sizeof(float));
            std::fill(slot.ownedLeft[next]->begin() + link.sourceFrames,
                      slot.ownedLeft[next]->end(), 0.0f);
            std::fill(slot.ownedRight[next]->begin() + link.sourceFrames,
                      slot.ownedRight[next]->end(), 0.0f);
            slot.sourceFrames.store(link.sourceFrames, std::memory_order_relaxed);
            slot.ownedFrames[next].store(link.sourceFrames, std::memory_order_relaxed);
            slot.ownedSampleRates[next].store(link.sourceSampleRate, std::memory_order_relaxed);
            slot.ownedSourceGenerations[next].store(link.sourceGeneration, std::memory_order_relaxed);
            slot.ownedGenerations[next].store(
                slot.generation.fetch_add(1, std::memory_order_relaxed) + 1,
                std::memory_order_relaxed);
            slot.owned.store(true, std::memory_order_relaxed);
            slot.publishedBuffer.store(next, std::memory_order_release);
            slot.left.store(slot.ownedLeft[next]->data(), std::memory_order_relaxed);
            slot.right.store(slot.ownedRight[next]->data(), std::memory_order_relaxed);
        }
        slot.sourceTrackId.store(link.sourceTrackId, std::memory_order_relaxed);
        slot.sourceSampleRate.store(link.sourceSampleRate, std::memory_order_relaxed);
        slot.sourceGeneration.store(link.sourceGeneration, std::memory_order_relaxed);
        slot.level.store(std::max(0.0f, link.level), std::memory_order_relaxed);
        slot.tapPoint.store(static_cast<uint8_t>(link.tapPoint), std::memory_order_relaxed);
        slot.key.store(key, std::memory_order_release);
        return true;
    }

    static bool publishSilent(Slot& slot, uint64_t key, uint32_t sourceTrackId,
                              uint32_t frames, uint32_t sampleRate,
                              uint64_t sourceGeneration, float level,
                              SidechainTapPoint tapPoint) noexcept {
        try {
            if (!slot.ownedLeft[0] || !slot.ownedLeft[1] ||
                !slot.ownedRight[0] || !slot.ownedRight[1]) {
                slot.ownedLeft[0] = std::make_unique<std::array<float, kMaxBlockSize>>();
                slot.ownedLeft[1] = std::make_unique<std::array<float, kMaxBlockSize>>();
                slot.ownedRight[0] = std::make_unique<std::array<float, kMaxBlockSize>>();
                slot.ownedRight[1] = std::make_unique<std::array<float, kMaxBlockSize>>();
            }
        } catch (...) {
            return false;
        }
        const uint32_t active = slot.publishedBuffer.load(std::memory_order_relaxed);
        const uint32_t next = 1u - active;
        slot.ownedLeft[next]->fill(0.0f);
        slot.ownedRight[next]->fill(0.0f);
        slot.ownedFrames[next].store(frames, std::memory_order_relaxed);
        slot.ownedSampleRates[next].store(sampleRate, std::memory_order_relaxed);
        slot.ownedSourceGenerations[next].store(sourceGeneration, std::memory_order_relaxed);
        slot.ownedGenerations[next].store(
            slot.generation.fetch_add(1, std::memory_order_relaxed) + 1,
            std::memory_order_relaxed);
        slot.sourceFrames.store(frames, std::memory_order_relaxed);
        slot.sourceTrackId.store(sourceTrackId, std::memory_order_relaxed);
        slot.sourceSampleRate.store(sampleRate, std::memory_order_relaxed);
        slot.sourceGeneration.store(sourceGeneration, std::memory_order_relaxed);
        slot.level.store(std::max(0.0f, level), std::memory_order_relaxed);
        slot.tapPoint.store(static_cast<uint8_t>(tapPoint), std::memory_order_relaxed);
        slot.owned.store(true, std::memory_order_relaxed);
        slot.publishedBuffer.store(next, std::memory_order_release);
        slot.left.store(slot.ownedLeft[next]->data(), std::memory_order_relaxed);
        slot.right.store(slot.ownedRight[next]->data(), std::memory_order_relaxed);
        slot.key.store(key, std::memory_order_release);
        return true;
    }

    void beginWrite(Slot& slot) noexcept {
        // Mark the slot unavailable before waiting. Readers that arrive after
        // this point will observe the odd version and never dereference the
        // published pointers. Existing readers are allowed to finish before
        // the inactive buffer is reused.
        while (m_audioPublishing.load(std::memory_order_acquire) ||
               m_activeReaders.load(std::memory_order_acquire) != 0) {
            std::this_thread::yield();
        }
        slot.version.fetch_add(1, std::memory_order_acq_rel);
    }

    void endWrite(Slot& slot) noexcept {
        slot.version.fetch_add(1, std::memory_order_release);
    }

    mutable std::atomic<uint32_t> m_activeReaders{0};
    // Protects the single audio publisher from control-side slot reuse
    // without ever making the audio callback wait on a mutex.
    std::atomic<bool> m_controlMutation{false};
    std::atomic<bool> m_audioPublishing{false};

    std::array<Slot, kMaxLinks> m_slots{};
    // The seqlock protects readers from a writer's partial publication, but
    // it is only correct with one writer. All control-plane mutations are
    // serialized here; the audio reader never takes this mutex.
    mutable std::mutex m_writerMutex;
    std::atomic<bool> m_legacyApiMisuse{false};
};
