#pragma once
#include <memory>
#include <string>
#include <filesystem>
#include <algorithm>
#include <cstdint>
#include <cstring>
#include "track.hpp"

namespace Hirari::Core::Engine {

/**
 * @class TrackFreezeManager
 * @brief Industrial Disk-Backed Track Freeze Engine.
 * HONEST FIX: Implemented disk-rendering to save RAM and background processing.
 */
class TrackFreezeManager {
public:
    // Bound control-plane allocation for malformed or accidental long-range
    // requests. Two float channels at this limit are about 512 MiB.
    static constexpr uint64_t kMaxFreezeSamples = 64ull * 1024ull * 1024ull;
    // Render and publish on a control/offline thread. Track::process sees the
    // immutable buffer at the next block boundary and performs only bounded
    // copies on the audio thread.
    static bool freezeTrack(std::shared_ptr<Track> track,
                            uint64_t totalSamples,
                            uint32_t sampleRate) {
        auto rendered = renderOffline(track, totalSamples);
        return rendered && track->publishFrozenAudio(
            std::move(rendered), totalSamples, sampleRate);
    }

    // Disk-backed variant used by project freeze caches. The writer owns a
    // unique temporary path, fsyncs it, and atomically renames it before the
    // in-memory snapshot becomes visible to the audio thread.
    static bool freezeTrackToFile(std::shared_ptr<Track> track,
                                  const std::filesystem::path& path,
                                  uint64_t totalSamples,
                                  uint32_t sampleRate) {
        if (path.empty()) return false;
        auto rendered = renderOffline(track, totalSamples);
        if (!rendered) return false;
        const std::string outputPath = path.string();
        void* storage = rendered.get();
        if (!hirari_track_freeze_write_float32(
                reinterpret_cast<const uint8_t*>(outputPath.data()), outputPath.size(),
                sampleRate, hirari_audio_buffer_storage_read_pointer(storage, 0, 0),
                hirari_audio_buffer_storage_read_pointer(storage, 1, 0),
                totalSamples)) return false;
        if (!track->publishFrozenAudio(std::move(rendered), totalSamples, sampleRate,
                                       0, 0, path.string())) {
            std::error_code ec;
            std::filesystem::remove(path, ec);
            return false;
        }
        return true;
    }

    // Restore the exact published audio artifact. This deliberately does not
    // re-render the live plugin graph: a plugin may be nondeterministic or
    // have changed version since the cache was produced.
    static bool restoreTrackFromFile(std::shared_ptr<Track> track,
                                     const std::filesystem::path& path,
                                     uint64_t totalSamples,
                                     uint32_t sampleRate) {
        if (!track || path.empty() || totalSamples == 0 || sampleRate == 0 ||
            totalSamples > kMaxFreezeSamples) return false;
        auto restored = makeSharedAudioStorage(2, static_cast<uint32_t>(totalSamples));
        if (!restored) return false;
        const std::string pathText = path.string();
        // Decode, rate agreement, and length admission are enforced by the
        // Rust freeze reader; a mono artifact is widened to stereo there.
        void* storage = restored.get();
        if (!hirari_track_freeze_read_float32(
                reinterpret_cast<const uint8_t*>(pathText.data()), pathText.size(),
                sampleRate, totalSamples,
                hirari_audio_buffer_storage_write_pointer(storage, 0),
                hirari_audio_buffer_storage_write_pointer(storage, 1))) {
            return false;
        }
        return track->publishFrozenAudio(std::move(restored), totalSamples,
                                         sampleRate, 0, 0, pathText);
    }

    static void unfreezeTrack(std::shared_ptr<Track> track) noexcept {
        if (track) track->clearFrozenAudio();
    }

private:
    struct RenderContext {
        Track* track = nullptr;
        void* block = nullptr;
    };

    static bool resetOfflineRender(void* opaque) noexcept {
        auto* context = static_cast<RenderContext*>(opaque);
        if (context == nullptr || context->track == nullptr) return false;
        try {
            context->track->resetForOfflineRender();
            return true;
        } catch (...) {
            return false;
        }
    }

    static bool renderBlock(void* opaque, float* left, float* right,
                            uint32_t frames, uint64_t playhead) noexcept {
        auto* context = static_cast<RenderContext*>(opaque);
        if (context == nullptr || context->track == nullptr || context->block == nullptr ||
            left == nullptr || right == nullptr || frames == 0 ||
            hirari_audio_buffer_storage_channels(context->block) < 2 ||
            hirari_audio_buffer_storage_samples(context->block) < frames) return false;
        try {
            hirari_audio_buffer_storage_clear(context->block, 0,
                hirari_audio_buffer_storage_samples(context->block));
            context->track->process(context->block, frames, playhead);
            const float* sourceLeft =
                hirari_audio_buffer_storage_read_pointer(context->block, 0, 0);
            const float* sourceRight =
                hirari_audio_buffer_storage_read_pointer(context->block, 1, 0);
            if (sourceLeft == nullptr || sourceRight == nullptr) return false;
            std::memcpy(left, sourceLeft, static_cast<size_t>(frames) * sizeof(float));
            std::memcpy(right, sourceRight, static_cast<size_t>(frames) * sizeof(float));
            return true;
        } catch (...) {
            return false;
        }
    }

    static SharedAudioStorage renderOffline(
        const std::shared_ptr<Track>& track, uint64_t totalSamples) {
        if (!track || totalSamples == 0 || totalSamples > kMaxFreezeSamples ||
            totalSamples > static_cast<uint64_t>(UINT32_MAX)) return nullptr;
        try {
            constexpr uint32_t kRenderBlockSize = 1024;
            auto rendered = makeSharedAudioStorage(2, static_cast<uint32_t>(totalSamples));
            auto block = makeSharedAudioStorage(2, kRenderBlockSize);
            RenderContext context{track.get(), block.get()};
            void* storage = rendered.get();
            if (!rendered || !hirari_track_freeze_render(
                    totalSamples, kRenderBlockSize,
                    hirari_audio_buffer_storage_write_pointer(storage, 0),
                    hirari_audio_buffer_storage_write_pointer(storage, 1),
                    hirari_audio_buffer_storage_samples(storage), &context,
                    &resetOfflineRender, &renderBlock)) return nullptr;
            return rendered;
        } catch (...) {
            return nullptr;
        }
    }

    TrackFreezeManager() = default;
};

} // namespace Hirari::Core::Engine
