    /// Internal coherent publication read. Raw pointers never leave the
    /// manager through a public API; copySidechainBlock is the only consumer
    /// entry point and copies into callback-owned buffers.
    bool readSnapshot(uint32_t destTrackId, uint32_t pluginIdx,
                      LinkSnapshot& destination) const noexcept {
        if (pluginIdx >= kMaxPluginIndex) return false;
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        for (uint32_t attempt = 0; attempt < 3; ++attempt) {
            for (const auto& slot : m_slots) {
                const uint64_t version = slot.version.load(std::memory_order_acquire);
                if ((version & 1u) != 0u || slot.key.load(std::memory_order_acquire) != key) continue;
                const uint32_t published = slot.publishedBuffer.load(std::memory_order_acquire);
                LinkSnapshot candidate{
                    slot.left.load(std::memory_order_relaxed),
                    slot.right.load(std::memory_order_relaxed),
                    slot.sourceTrackId.load(std::memory_order_relaxed),
                    slot.level.load(std::memory_order_relaxed),
                    static_cast<SidechainTapPoint>(slot.tapPoint.load(std::memory_order_relaxed)),
                    0, 0, 0, 0};
                if (slot.version.load(std::memory_order_acquire) == version) {
                    // Owned blocks are immutable after publication. The
                    // published index is read as part of the seqlock so the
                    // audio reader never observes a block while it is being
                    // copied into the inactive slot.
                    if (slot.owned.load(std::memory_order_relaxed)) {
                        if (!slot.ownedLeft[published] || !slot.ownedRight[published]) continue;
                        candidate.left = slot.ownedLeft[published]->data();
                        candidate.right = slot.ownedRight[published]->data();
                        candidate.frames = slot.ownedFrames[published].load(std::memory_order_relaxed);
                        candidate.sampleRate = slot.ownedSampleRates[published].load(std::memory_order_relaxed);
                        candidate.sourceGeneration = slot.ownedSourceGenerations[published].load(std::memory_order_relaxed);
                        candidate.generation = slot.ownedGenerations[published].load(std::memory_order_relaxed);
                    } else {
                        candidate.frames = slot.sourceFrames.load(std::memory_order_relaxed);
                        candidate.sampleRate = slot.sourceSampleRate.load(std::memory_order_relaxed);
                        candidate.sourceGeneration = slot.sourceGeneration.load(std::memory_order_relaxed);
                        candidate.generation = slot.generation.load(std::memory_order_relaxed);
                    }
                    if (candidate.frames == 0 || candidate.frames > kMaxBlockSize) continue;
                    destination = candidate;
                    return candidate.left != nullptr && candidate.right != nullptr;
                }
            }
        }
        return false;
    }

public:
