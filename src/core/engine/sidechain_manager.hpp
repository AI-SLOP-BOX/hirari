#pragma once
#include <array>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <thread>
#include <vector>

namespace Aura::Core::Engine {

enum class SidechainTapPoint { PreFX, PostFX, PostFader };

// Kept for source compatibility with the original control-plane API.
struct SidechainSource {
    uint32_t trackId = 0;
    float level = 1.0f;
};

struct SidechainLink {
    const float* sourceBufferL = nullptr;
    const float* sourceBufferR = nullptr;
    uint32_t sourceTrackId = 0;
    float level = 1.0f;
    SidechainTapPoint tapPoint = SidechainTapPoint::PostFX;
    // When non-zero, registerLink copies the source block into the manager's
    // owned fixed-address buffers. This is the safe publication path.
    uint32_t sourceFrames = 0;
    uint32_t sourceSampleRate = 0;
    uint64_t sourceGeneration = 0;
};

/**
 * @class SidechainManager
 * @brief Industrial Sidechain Orchestration Engine.
 * HONEST FIX: Implemented O(1) direct buffer access and flexible tap points.
 */
class SidechainManager {
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

    float getSidechainLevel(uint32_t destTrackId, uint32_t pluginIdx) const noexcept {
        if (pluginIdx >= kMaxPluginIndex) return 0.0f;
        const uint64_t key = makeKey(destTrackId, pluginIdx);
        for (const auto& slot : m_slots) {
            if (slot.key.load(std::memory_order_acquire) == key)
                return slot.level.load(std::memory_order_relaxed);
        }
        return 0.0f;
    }

    bool controlRefreshSourceBuffer(uint32_t sourceTrackId, const float* left,
                                    const float* right, uint32_t frames,
                                    uint32_t sampleRate,
                                    uint64_t sourceGeneration) noexcept {
        if (!left || !right || frames == 0 || frames > kMaxBlockSize ||
            sampleRate == 0 || sourceGeneration == 0) return false;
        bool refreshed = false;
        ControlMutationGuard mutation(*this);
        for (auto& slot : m_slots) {
            if (slot.key.load(std::memory_order_acquire) != kEmptyKey &&
                slot.sourceTrackId.load(std::memory_order_relaxed) == sourceTrackId) {
                if (slot.sourceGeneration.load(std::memory_order_relaxed) != sourceGeneration) continue;
                if (slot.sourceSampleRate.load(std::memory_order_relaxed) != sampleRate) continue;
                beginWrite(slot);
                const uint32_t copyFrames = frames;
                if (slot.owned.load(std::memory_order_relaxed) &&
                    copyFrames > 0 && copyFrames <= kMaxBlockSize) {
                    const uint32_t active = slot.publishedBuffer.load(std::memory_order_relaxed);
                    const uint32_t next = 1u - active;
                    std::memcpy(slot.ownedLeft[next]->data(), left,
                                copyFrames * sizeof(float));
                    std::memcpy(slot.ownedRight[next]->data(), right,
                                copyFrames * sizeof(float));
                    std::fill(slot.ownedLeft[next]->begin() + copyFrames,
                              slot.ownedLeft[next]->end(), 0.0f);
                    std::fill(slot.ownedRight[next]->begin() + copyFrames,
                              slot.ownedRight[next]->end(), 0.0f);
                    slot.ownedFrames[next].store(copyFrames, std::memory_order_relaxed);
                    slot.ownedSampleRates[next].store(sampleRate, std::memory_order_relaxed);
                    slot.ownedSourceGenerations[next].store(sourceGeneration, std::memory_order_relaxed);
                    slot.ownedGenerations[next].store(
                        slot.generation.fetch_add(1, std::memory_order_relaxed) + 1,
                        std::memory_order_relaxed);
                    slot.sourceFrames.store(copyFrames, std::memory_order_relaxed);
                    slot.publishedBuffer.store(next, std::memory_order_release);
                    refreshed = true;
                }
                endWrite(slot);
            }
        }
        return refreshed;
    }

    // Audio-thread publication path. The slot's two owned buffers are
    // allocated by registerLink/registerSilentLink on the control thread.
    // Device reconfiguration and graph mutation quiesce callbacks before
    // changing the slot topology, so this path performs only bounded copies
    // into the inactive buffer and never takes the control writer mutex.
    void publishAudioSourceBlock(uint32_t sourceTrackId, const float* left,
                                 const float* right, uint32_t frames,
                                 uint32_t sampleRate,
                                 uint64_t sourceGeneration) noexcept {
        if (!left || !right || frames == 0 || frames > kMaxBlockSize ||
            sampleRate == 0 || sourceGeneration == 0) return;
        // Control publication and device publication are separate writers.
        // The audio path must never wait for the control mutex, so it uses an
        // admission flag plus a bounded CAS guard. A control mutation closes
        // admission before it reuses either owned buffer; a callback already
        // inside this section is allowed to finish and is waited on by the
        // control-side beginWrite().
        if (m_controlMutation.load(std::memory_order_acquire)) return;
        // A reader copies from the currently published buffer while holding
        // m_activeReaders.  One publication is safe when a reader arrives
        // after this check because the writer only touches the inactive
        // buffer.  A second publication before that reader leaves, however,
        // could rotate back and overwrite the buffer being copied.  Never
        // wait in the audio callback: skip this publication and let the
        // consumer retain the last coherent block instead.
        if (m_activeReaders.load(std::memory_order_acquire) != 0) return;
        bool expected = false;
        if (!m_audioPublishing.compare_exchange_strong(
                expected, true, std::memory_order_acq_rel,
                std::memory_order_relaxed)) {
            return;
        }
        if (m_controlMutation.load(std::memory_order_acquire)) {
            m_audioPublishing.store(false, std::memory_order_release);
            return;
        }
        // Re-check after admission so a reader that entered while the
        // publisher lock was acquired cannot be followed by a second buffer
        // rotation from this callback.
        if (m_activeReaders.load(std::memory_order_acquire) != 0) {
            m_audioPublishing.store(false, std::memory_order_release);
            return;
        }
        for (auto& slot : m_slots) {
            // Do not rotate another slot after a reader has entered.  This
            // keeps the two-buffer lifetime rule valid for every slot touched
            // by this publication pass without introducing an RT wait.
            if (m_activeReaders.load(std::memory_order_acquire) != 0) break;
            if (slot.key.load(std::memory_order_acquire) == kEmptyKey ||
                slot.sourceTrackId.load(std::memory_order_relaxed) != sourceTrackId ||
                !slot.owned.load(std::memory_order_relaxed)) continue;
            auto* leftBuffers = slot.ownedLeft[0].get();
            auto* rightBuffers = slot.ownedRight[0].get();
            if (!leftBuffers || !rightBuffers || !slot.ownedLeft[1] || !slot.ownedRight[1]) continue;

            // This writer is the single device callback publisher. The
            // control plane closes callback admission before modifying slots.
            slot.version.fetch_add(1, std::memory_order_acq_rel);
            const uint32_t active = slot.publishedBuffer.load(std::memory_order_relaxed);
            const uint32_t next = 1u - active;
            std::memcpy(slot.ownedLeft[next]->data(), left, frames * sizeof(float));
            std::memcpy(slot.ownedRight[next]->data(), right, frames * sizeof(float));
            std::fill(slot.ownedLeft[next]->begin() + frames,
                      slot.ownedLeft[next]->end(), 0.0f);
            std::fill(slot.ownedRight[next]->begin() + frames,
                      slot.ownedRight[next]->end(), 0.0f);
            slot.ownedFrames[next].store(frames, std::memory_order_relaxed);
            slot.ownedSampleRates[next].store(sampleRate, std::memory_order_relaxed);
            slot.ownedSourceGenerations[next].store(sourceGeneration, std::memory_order_relaxed);
            slot.ownedGenerations[next].store(
                slot.generation.fetch_add(1, std::memory_order_relaxed) + 1,
                std::memory_order_relaxed);
            slot.sourceFrames.store(frames, std::memory_order_relaxed);
            slot.sourceSampleRate.store(sampleRate, std::memory_order_relaxed);
            slot.sourceGeneration.store(sourceGeneration, std::memory_order_relaxed);
            slot.publishedBuffer.store(next, std::memory_order_release);
            slot.left.store(slot.ownedLeft[next]->data(), std::memory_order_relaxed);
            slot.right.store(slot.ownedRight[next]->data(), std::memory_order_relaxed);
            slot.version.fetch_add(1, std::memory_order_release);
        }
        m_audioPublishing.store(false, std::memory_order_release);
    }

    [[deprecated("use controlRefreshSourceBuffer with mandatory generation metadata")]]
    bool refreshSourceBuffer(uint32_t sourceTrackId, const float* left,
                             const float* right, uint32_t frames,
                             uint32_t sampleRate, uint64_t sourceGeneration) noexcept {
        return controlRefreshSourceBuffer(sourceTrackId, left, right, frames,
                                          sampleRate, sourceGeneration);
    }

    /**
     * @brief RESOLVE: Connects sidechain destinations to source buffers with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating signal dependency tracking and tap-point resolution to the Rust 'RoutingOrchestrator'.
     */
    bool resolveSidechainLinks() noexcept {
        // This legacy entry point never carried enough information to publish
        // a safe, frame-bounded snapshot. Returning false is intentional: a
        // caller must use registerLink() instead of silently believing that a
        // sidechain was connected.
        m_legacyApiMisuse.store(true, std::memory_order_release);
        return false;
    }

    bool takeLegacyApiMisuse() noexcept {
        return m_legacyApiMisuse.exchange(false, std::memory_order_acq_rel);
    }


private:
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

} // namespace Aura::Core::Engine
