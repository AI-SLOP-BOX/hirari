/*
 * Hirari DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Hirari DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#pragma once

#include <string>
#include <vector>
#include <fstream>
#include <functional>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <iostream>
#include <limits>
#include <filesystem>
#include <atomic>
#include <memory>
#include <set>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../core/audio_buffer.hpp"
#include "../core/engine/timeline_system.hpp"
#include "../dsp/analysis/master_meter.hpp"
#include "persistence/wav_writer.hpp"

namespace Hirari::IO {

/**
 * @class AudioExportEngine
 * @brief High-speed Offline Rendering (Bouncing) Engine.
 * HONEST FIX: Performs a non-real-time render as fast as the CPU allows.
 */
class AudioExportEngine {
public:
    struct ExportOptions {
        std::string filename;
        double sampleRate = 44100.0;
        uint32_t bitDepth = 24;
        uint64_t startSample = 0;
        uint64_t endSample = 0;
        uint16_t channels = 2;
        bool normalize = false;
        bool renderStems = false;
        // 0=post-fader, 1=pre-insert, 2=pre-fader.
        uint32_t renderTap = 0;
        // Optional control-side cancellation flag. The render loop polls it
        // between bounded blocks; writers discard their temporary file when
        // the operation is cancelled before publication.
        const std::atomic<bool>* cancellation = nullptr;
        // Internal recursive selector used by renderStems. Zero means the
        // full timeline mix; track IDs are never zero in a valid project.
        uint32_t stemTrackId = 0;
    };

    struct MasteringReport {
        float lufsIntegrated;
        float truePeakMax;
        std::string advice;
    };

    /**
     * @brief EXPORT: The core non-real-time bounce loop with 16/24-bit PCM or
     * 32-bit IEEE-float output.
     */
    static bool bounce(Core::Engine::TimelineSystem& timeline, const ExportOptions& opts, std::function<void(float)> onProgress) {
        if (opts.renderStems && opts.stemTrackId == 0) {
            const auto tracks = timeline.getTracksSnapshot();
            if (tracks.empty() || opts.filename.empty()) return false;
            std::filesystem::path masterPath(opts.filename);
            const std::string extension = masterPath.extension().string();
            const std::string stemBase = masterPath.replace_extension().string();
            std::set<std::string> destinations;
            for (const auto& track : tracks) {
                if (!track || track->getId() == 0) continue;
                std::string label = track->getName();
                if (label.empty()) label = "track";
                for (char& character : label) {
                    const bool safe = (character >= 'a' && character <= 'z') ||
                        (character >= 'A' && character <= 'Z') ||
                        (character >= '0' && character <= '9') ||
                        character == '-' || character == '_';
                    if (!safe) character = '_';
                }
                const auto destination = stemBase + ".stem-" +
                    std::to_string(track->getId()) + "-" + label + extension;
                std::error_code destinationError;
                if (!destinations.insert(destination).second ||
                    std::filesystem::exists(destination, destinationError) ||
                    destinationError) return false;
            }
            size_t stemIndex = 0;
            std::vector<std::filesystem::path> publishedStems;
            publishedStems.reserve(destinations.size());
            const auto rollbackStems = [&]() noexcept {
                for (const auto& generated : publishedStems) {
                    std::error_code cleanup;
                    std::filesystem::remove(generated, cleanup);
                }
            };
            for (const auto& track : tracks) {
                if (!track || track->getId() == 0) continue;
                std::string label = track->getName();
                if (label.empty()) label = "track";
                for (char& character : label) {
                    const bool safe = (character >= 'a' && character <= 'z') ||
                        (character >= 'A' && character <= 'Z') ||
                        (character >= '0' && character <= '9') ||
                        character == '-' || character == '_';
                    if (!safe) character = '_';
                }
                ExportOptions stem = opts;
                stem.renderStems = false;
                stem.stemTrackId = track->getId();
                stem.filename = stemBase + ".stem-" +
                    std::to_string(track->getId()) + "-" + label + extension;
                const bool rendered = bounce(timeline, stem,
                    [&, stemIndex, total = tracks.size()](float progress) {
                        if (onProgress) {
                            onProgress(static_cast<float>(stemIndex) / total + progress / total);
                        }
                    });
                if (!rendered) {
                    rollbackStems();
                    return false;
                }
                publishedStems.emplace_back(stem.filename);
                ++stemIndex;
            }
            if (onProgress) onProgress(1.0f);
            return stemIndex != 0;
        }
        if (opts.filename.empty() || !std::isfinite(opts.sampleRate) ||
            opts.sampleRate < 8000.0 || opts.sampleRate > 384000.0 ||
            opts.renderTap > 2u) return false;

        struct RenderContext {
            Core::Engine::TimelineSystem& timeline;
            const std::vector<std::shared_ptr<Core::Engine::Track>>& tracks;
            Core::AudioBuffer buffer;
            uint32_t tap;
            uint32_t stemTrackId;
            const std::atomic<bool>* cancellation;
            std::function<void(float)>* onProgress;
        } context{timeline, timeline.getTracksSnapshot(), Core::AudioBuffer(2, 1024),
                  opts.renderTap, opts.stemTrackId, opts.cancellation, &onProgress};
        const auto reset = +[](void* opaque) -> uint8_t {
            auto& state = *static_cast<RenderContext*>(opaque);
            state.timeline.resetForOfflineRender();
            return 1;
        };
        const auto render = +[](void* opaque, uint64_t position, uint32_t frames,
                                float** output) -> uint8_t {
            auto& state = *static_cast<RenderContext*>(opaque);
            if (!output || !output[0] || !output[1] || frames == 0 || frames > 1024) return 0;
            state.buffer.clear();
            for (const auto& track : state.tracks) {
                if (state.stemTrackId != 0 &&
                    (!track || track->getId() != state.stemTrackId)) continue;
                if (track) track->processInto(state.buffer, frames, position,
                    static_cast<Core::Engine::Track::OfflineRenderTap>(state.tap));
            }
            const float* left = state.buffer.getReadPointer(0);
            const float* right = state.buffer.getReadPointer(1);
            if (!left || !right) return 0;
            std::copy_n(left, frames, output[0]);
            std::copy_n(right, frames, output[1]);
            return 1;
        };
        const auto cancelled = +[](void* opaque) -> uint8_t {
            const auto& state = *static_cast<RenderContext*>(opaque);
            return state.cancellation &&
                state.cancellation->load(std::memory_order_acquire) ? 1 : 0;
        };
        const auto reportProgress = +[](void* opaque, float value) {
            auto& state = *static_cast<RenderContext*>(opaque);
            if (*state.onProgress) (*state.onProgress)(value);
        };
        return hirari_audio_export_bounce(
            opts.filename.data(), opts.filename.size(),
            static_cast<uint32_t>(opts.sampleRate), opts.bitDepth, opts.channels,
            opts.startSample, opts.endSample, opts.normalize ? 1 : 0, &context,
            reset, render, cancelled, reportProgress) != 0;
    }

};

} // namespace Hirari::IO
