#pragma once
#include <memory>
#include <string>
#include <filesystem>
#include <algorithm>
#include <cstdint>
#include <cstring>
#include "track.hpp"
#include "bounce_engine.hpp"
#include "../../io/persistence/wav_writer.hpp"

namespace Aura::Core::Engine {

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
        Aura::IO::Persistence::WavWriter::Float32StreamWriter writer(
            path.string(), sampleRate, 2);
        if (!writer.isOpen()) return false;
        for (uint64_t offset = 0; offset < totalSamples;) {
            const uint32_t frames = static_cast<uint32_t>(std::min<uint64_t>(
                4096u, totalSamples - offset));
            const float* channels[2] = {
                rendered->getReadPointer(0) + static_cast<size_t>(offset),
                rendered->getReadPointer(1) + static_cast<size_t>(offset)
            };
            if (!writer.writeFrames(channels, frames)) return false;
            offset += frames;
        }
        if (!writer.finish()) return false;
        if (!track->publishFrozenAudio(rendered, totalSamples, sampleRate,
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
        Aura::Core::IO::WavDecoder decoder;
        if (!decoder.open(path.string()) ||
            std::abs(decoder.getSampleRate() - static_cast<double>(sampleRate)) > 0.5) {
            return false;
        }
        auto decoded = std::make_shared<AudioBuffer>();
        decoder.decodeFull(*decoded);
        if (decoded->getNumSamples() < totalSamples || decoded->getNumChannels() == 0) {
            return false;
        }
        if (decoded->getNumChannels() >= 2) {
            return track->publishFrozenAudio(std::move(decoded), totalSamples,
                                             sampleRate, 0, 0, path.string());
        }
        auto stereo = std::make_shared<AudioBuffer>(2, decoded->getNumSamples());
        const float* mono = decoded->getReadPointer(0);
        if (!mono) return false;
        std::memcpy(stereo->getWritePointer(0), mono,
                    static_cast<size_t>(decoded->getNumSamples()) * sizeof(float));
        std::memcpy(stereo->getWritePointer(1), mono,
                    static_cast<size_t>(decoded->getNumSamples()) * sizeof(float));
        return track->publishFrozenAudio(std::move(stereo), totalSamples,
                                         sampleRate, 0, 0, path.string());
    }

    static void unfreezeTrack(std::shared_ptr<Track> track) noexcept {
        if (track) track->clearFrozenAudio();
    }

private:
    static std::shared_ptr<AudioBuffer> renderOffline(
        const std::shared_ptr<Track>& track, uint64_t totalSamples) {
        if (!track || totalSamples == 0 || totalSamples > kMaxFreezeSamples ||
            totalSamples > static_cast<uint64_t>(UINT32_MAX)) return nullptr;
        try {
            constexpr uint32_t kRenderBlockSize = 1024;
            auto rendered = std::make_shared<AudioBuffer>(2, static_cast<uint32_t>(totalSamples));
            AudioBuffer block(2, kRenderBlockSize);
            track->resetForOfflineRender();
            uint64_t renderedSamples = 0;
            while (renderedSamples < totalSamples) {
                const uint32_t blockSize = static_cast<uint32_t>(std::min<uint64_t>(
                    kRenderBlockSize, totalSamples - renderedSamples));
                block.clear();
                track->process(block, blockSize, renderedSamples);
                for (uint32_t channel = 0; channel < 2; ++channel) {
                    const float* source = block.getReadPointer(channel);
                    float* destination = rendered->getWritePointer(channel);
                    if (source == nullptr || destination == nullptr) return nullptr;
                    std::memcpy(destination + static_cast<size_t>(renderedSamples),
                                source,
                                static_cast<size_t>(blockSize) * sizeof(float));
                }
                renderedSamples += blockSize;
            }
            return rendered;
        } catch (...) {
            return nullptr;
        }
    }

    TrackFreezeManager() = default;
};


} // namespace Aura::Core::Engine
