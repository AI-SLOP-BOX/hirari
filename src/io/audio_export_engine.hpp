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
        if (opts.filename.empty() || !std::isfinite(opts.sampleRate) ||
            opts.sampleRate < 8000.0 || opts.sampleRate > 384000.0 ||
            opts.renderTap > 2u) return false;
        struct RenderContext {
            Core::Engine::TimelineSystem& timeline;
            const std::vector<std::shared_ptr<Core::Engine::Track>>& tracks;
            Core::AudioBuffer buffer;
            uint32_t tap;
            uint32_t stemTrackId;
            uint16_t channels;
            const std::atomic<bool>* cancellation;
            std::function<void(float)>* onProgress;
        } context{timeline, timeline.getTracksSnapshot(), Core::AudioBuffer(2, 1024),
                  opts.renderTap, opts.stemTrackId, opts.channels, opts.cancellation,
                  &onProgress};
        const auto selectTrack = +[](void* opaque, uint32_t id) -> uint8_t {
            static_cast<RenderContext*>(opaque)->stemTrackId = id;
            return 1;
        };
        const auto reset = +[](void* opaque) -> uint8_t {
            static_cast<RenderContext*>(opaque)->timeline.resetForOfflineRender();
            return 1;
        };
        const auto render = +[](void* opaque, uint64_t position, uint32_t frames,
                                float** output) -> uint8_t {
            auto& state = *static_cast<RenderContext*>(opaque);
            if (!output || !output[0] || (state.channels == 2 && !output[1]) ||
                frames == 0 || frames > 1024) return 0;
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
            if (state.channels == 2) std::copy_n(right, frames, output[1]);
            else for (uint32_t i = 0; i < frames; ++i)
                output[0][i] = 0.5f * (left[i] + right[i]);
            return 1;
        };
        const auto cancelled = +[](void* opaque) -> uint8_t {
            const auto* flag = static_cast<RenderContext*>(opaque)->cancellation;
            return flag && flag->load(std::memory_order_acquire) ? 1 : 0;
        };
        const auto reportProgress = +[](void* opaque, float value) {
            auto& state = *static_cast<RenderContext*>(opaque);
            if (state.onProgress && *state.onProgress) (*state.onProgress)(value);
        };
        if (opts.renderStems && opts.stemTrackId == 0) {
            const auto& tracks = context.tracks;
            if (tracks.empty() || opts.filename.empty()) return false;
            std::vector<std::string> names;
            std::vector<HirariAudioExportStemTrack> stemTracks;
            names.reserve(tracks.size());
            stemTracks.reserve(tracks.size());
            for (const auto& track : tracks) {
                if (!track || track->getId() == 0) continue;
                names.push_back(track->getName());
                const auto& name = names.back();
                stemTracks.push_back({track->getId(),
                    reinterpret_cast<const uint8_t*>(name.data()), name.size()});
            }
            return hirari_audio_export_stems(
                reinterpret_cast<const uint8_t*>(opts.filename.data()), opts.filename.size(),
                stemTracks.data(), stemTracks.size(), tracks.size(),
                static_cast<uint32_t>(opts.sampleRate),
                opts.bitDepth, opts.channels, opts.startSample, opts.endSample,
                opts.normalize ? 1 : 0, &context, selectTrack, reset, render,
                cancelled, reportProgress) != 0;
        }
        return hirari_audio_export_bounce(
            opts.filename.data(), opts.filename.size(),
            static_cast<uint32_t>(opts.sampleRate), opts.bitDepth, opts.channels,
            opts.startSample, opts.endSample, opts.normalize ? 1 : 0, &context,
            reset, render, cancelled, reportProgress) != 0;
    }

};

} // namespace Hirari::IO
