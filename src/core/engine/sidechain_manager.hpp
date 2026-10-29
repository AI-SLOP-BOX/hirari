#pragma once
#include <atomic>
#include <bit>
#include <cstdint>
#include <utility>
#include <vector>

// The gate handle, writer lock and control/reader entry points live in
// sidechain_manager.rs behind one engine handle.
extern "C" void* hirari_sidechain_engine_create();
extern "C" void hirari_sidechain_engine_destroy(void* engine);
extern "C" bool hirari_sidechain_engine_register_link(void*, uint32_t, uint32_t, uint32_t, const float*, const float*, uint32_t, uint32_t, uint64_t, float, uint8_t);
extern "C" bool hirari_sidechain_engine_register_silent_link(void*, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint64_t, float, uint8_t);
extern "C" bool hirari_sidechain_engine_remove_link(void*, uint32_t, uint32_t);
extern "C" bool hirari_sidechain_engine_has_link(const void*, uint32_t, uint32_t, uint32_t);
extern "C" void hirari_sidechain_engine_remove_track(void*, uint32_t);
extern "C" void hirari_sidechain_engine_reset(void*);
extern "C" uint32_t hirari_sidechain_engine_copy_link(const void*, uint32_t, uint32_t, float*, float*, uint32_t, uint64_t*);
extern "C" bool hirari_sidechain_engine_refresh_source(const void*, uint32_t, const float*, const float*, uint32_t, uint32_t, uint64_t);
extern "C" void hirari_sidechain_engine_publish_source(const void*, uint32_t, const float*, const float*, const float*, const float*, const float*, const float*, uint32_t, uint32_t, uint64_t);
using HirariSidechainCaptureCallback = bool (*)(void*, uint32_t, uint32_t, uint32_t, float, uint8_t, uint32_t, uint64_t, const float*, const float*, uint32_t);
extern "C" bool hirari_sidechain_engine_capture_track(const void*, uint32_t, void*, HirariSidechainCaptureCallback);

namespace Hirari::Core::Engine {

enum class SidechainTapPoint { PreFX, PostFX, PostFader };

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

    SidechainManager() : m_engine(hirari_sidechain_engine_create()) {}
    ~SidechainManager() { hirari_sidechain_engine_destroy(m_engine); }
    static SidechainManager& getInstance() { static SidechainManager i; return i; }

    static constexpr uint32_t kMaxLinks = 256;
    static constexpr uint32_t kMaxBlockSize = 4096;

    bool registerLink(uint32_t destination, uint32_t plugin,
                      const SidechainLink& link) noexcept {
        return hirari_sidechain_engine_register_link(
            m_engine, destination, plugin, link.sourceTrackId,
            link.sourceBufferL, link.sourceBufferR, link.sourceFrames,
            link.sourceSampleRate, link.sourceGeneration, link.level,
            static_cast<uint8_t>(link.tapPoint));
    }

    bool registerSilentLink(uint32_t destination, uint32_t plugin, uint32_t source,
                            uint32_t frames, uint32_t sampleRate,
                            uint64_t sourceGeneration, float level,
                            SidechainTapPoint tapPoint) noexcept {
        return hirari_sidechain_engine_register_silent_link(
            m_engine, destination, plugin, source, frames, sampleRate,
            sourceGeneration, level, static_cast<uint8_t>(tapPoint));
    }

    bool removeLink(uint32_t destination, uint32_t plugin) noexcept {
        return hirari_sidechain_engine_remove_link(m_engine, destination, plugin);
    }

    bool hasLink(uint32_t destination, uint32_t plugin, uint32_t source) const noexcept {
        return hirari_sidechain_engine_has_link(m_engine, destination, plugin, source);
    }

    void removeLinksForTrack(uint32_t track) noexcept {
        hirari_sidechain_engine_remove_track(m_engine, track);
    }

    std::vector<LinkState> captureLinksForTrack(uint32_t track) {
        std::vector<LinkState> result;
        if (track == 0) return result;
        struct CaptureContext { std::vector<LinkState>* links; } context{&result};
        const bool captured = hirari_sidechain_engine_capture_track(
            m_engine, track, &context,
            [](void* opaque, uint32_t destination, uint32_t plugin, uint32_t source,
               float level, uint8_t tap, uint32_t sampleRate, uint64_t generation,
               const float* left, const float* right, uint32_t frames) -> bool {
                auto* context = static_cast<CaptureContext*>(opaque);
                if (!context || !context->links || !left || !right || frames == 0) return false;
                try {
                    LinkState item;
                    item.destinationTrackId = destination;
                    item.pluginIndex = plugin;
                    item.sourceTrackId = source;
                    item.level = level;
                    item.tapPoint = static_cast<SidechainTapPoint>(tap);
                    item.sampleRate = sampleRate;
                    item.sourceGeneration = generation;
                    item.left.assign(left, left + frames);
                    item.right.assign(right, right + frames);
                    context->links->push_back(std::move(item));
                    return true;
                } catch (...) {
                    return false;
                }
            });
        if (!captured) result.clear();
        return result;
    }

    void restoreLinks(const std::vector<LinkState>& states) noexcept {
        for (const auto& item : states) {
            if (item.destinationTrackId == 0 || item.sourceTrackId == 0 ||
                item.left.empty() || item.left.size() != item.right.size() ||
                item.left.size() > kMaxBlockSize || item.sampleRate == 0 ||
                item.sourceGeneration == 0) continue;
            SidechainLink link;
            link.sourceBufferL = item.left.data();
            link.sourceBufferR = item.right.data();
            link.sourceTrackId = item.sourceTrackId;
            link.level = item.level;
            link.tapPoint = item.tapPoint;
            link.sourceFrames = static_cast<uint32_t>(item.left.size());
            link.sourceSampleRate = item.sampleRate;
            link.sourceGeneration = item.sourceGeneration;
            (void)registerLink(item.destinationTrackId, item.pluginIndex, link);
        }
    }

    void resetForProject() noexcept {
        m_legacyApiMisuse.store(false, std::memory_order_release);
        hirari_sidechain_engine_reset(m_engine);
    }

    bool copySidechainBlock(uint32_t destination, uint32_t plugin,
                            float* left, float* right, uint32_t capacity,
                            LinkSnapshot& metadata) const noexcept {
        if (!left || !right || capacity == 0) return false;
        uint64_t values[6]{};
        const uint32_t frames = hirari_sidechain_engine_copy_link(
            m_engine, destination, plugin, left, right, capacity, values);
        if (frames == 0) return false;
        metadata.left = left;
        metadata.right = right;
        metadata.sourceTrackId = static_cast<uint32_t>(values[0]);
        metadata.level = std::bit_cast<float>(static_cast<uint32_t>(values[1]));
        metadata.tapPoint = static_cast<SidechainTapPoint>(values[2]);
        metadata.sampleRate = static_cast<uint32_t>(values[3]);
        metadata.sourceGeneration = values[4];
        metadata.generation = values[5];
        metadata.frames = frames;
        return true;
    }

    bool controlRefreshSourceBuffer(uint32_t source, const float* left,
                                    const float* right, uint32_t frames,
                                    uint32_t sampleRate,
                                    uint64_t sourceGeneration) noexcept {
        return hirari_sidechain_engine_refresh_source(
            m_engine, source, left, right, frames, sampleRate, sourceGeneration);
    }

    void publishAudioSourceBlock(uint32_t source,
                                 const float* preLeft, const float* preRight,
                                 const float* postLeft, const float* postRight,
                                 const float* faderLeft, const float* faderRight,
                                 uint32_t frames, uint32_t sampleRate,
                                 uint64_t sourceGeneration) noexcept {
        hirari_sidechain_engine_publish_source(
            m_engine, source, preLeft, preRight, postLeft, postRight,
            faderLeft, faderRight, frames, sampleRate, sourceGeneration);
    }

    // Kept for the snapshot contract: the retired source-registration API
    // reports its misuse exactly once per reset.
    bool resolveSidechainLinks() noexcept {
        m_legacyApiMisuse.store(true, std::memory_order_release);
        return false;
    }

    bool takeLegacyApiMisuse() noexcept {
        return m_legacyApiMisuse.exchange(false, std::memory_order_acq_rel);
    }

private:
    void* m_engine = nullptr;
    std::atomic<bool> m_legacyApiMisuse{false};
};

} // namespace Hirari::Core::Engine
