/*
 * Aura DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
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

namespace Aura::IO {

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
            const uint32_t bytesPerSample = opts.bitDepth / 8;
        if (opts.filename.empty() || !std::isfinite(opts.sampleRate) ||
            opts.sampleRate < 8000.0 || opts.sampleRate > 384000.0 ||
            (opts.bitDepth != 16 && opts.bitDepth != 24 && opts.bitDepth != 32) ||
            (opts.channels != 1 && opts.channels != 2) ||
            opts.renderTap > 2u ||
            opts.endSample <= opts.startSample || bytesPerSample == 0) {
            return false;
        }

        const uint64_t totalSamples = opts.endSample - opts.startSample;
        const uint64_t kChannels = opts.channels;
        const uint64_t bytesPerFrame = kChannels * bytesPerSample;
        if (totalSamples > std::numeric_limits<uint64_t>::max() / bytesPerFrame) return false;

        std::error_code fsError;
        const std::filesystem::path output(opts.filename);
        if (output.has_parent_path()) {
            std::filesystem::create_directories(output.parent_path(), fsError);
            if (fsError) return false;
        }

        // Render directly into the canonical atomic streaming writer.  The
        // previous implementation accumulated the complete song in RAM,
        // which made long exports compete with the audio engine. Normalized
        // exports intentionally use two bounded-memory passes.
        const uint32_t sampleRate = static_cast<uint32_t>(opts.sampleRate);
        std::unique_ptr<Persistence::WavWriter::Pcm16StreamWriter> pcm16;
        std::unique_ptr<Persistence::WavWriter::Pcm24StreamWriter> pcm24;
        std::unique_ptr<Persistence::WavWriter::Float32StreamWriter> float32;
        auto openWriter = [&]() -> bool {
            if (opts.bitDepth == 16) {
                pcm16 = std::make_unique<Persistence::WavWriter::Pcm16StreamWriter>(
                    opts.filename, totalSamples, sampleRate, opts.channels);
                return pcm16->isOpen();
            }
            if (opts.bitDepth == 32) {
                float32 = std::make_unique<Persistence::WavWriter::Float32StreamWriter>(
                    opts.filename, sampleRate, opts.channels);
                return float32->isOpen();
            }
            pcm24 = std::make_unique<Persistence::WavWriter::Pcm24StreamWriter>(
                opts.filename, totalSamples, sampleRate, false, opts.channels);
            return pcm24->isOpen();
        };

        const auto tracks = timeline.getTracksSnapshot();
        constexpr uint32_t blockSize = 1024;
        Core::AudioBuffer buffer(opts.channels, blockSize);
        float normalizationGain = 1.0f;
        const uint32_t writePass = opts.normalize ? 1u : 0u;
        try {
            for (uint32_t pass = 0; pass < (opts.normalize ? 2u : 1u); ++pass) {
                timeline.resetForOfflineRender();
                if (pass == writePass && !openWriter()) return false;
                DSP::Analysis::MasterMeter analyzer(opts.sampleRate);
                uint64_t rendered = 0;
                uint64_t currentPos = opts.startSample;
                float peak = 0.0f;
                while (rendered < totalSamples) {
                    if (opts.cancellation &&
                        opts.cancellation->load(std::memory_order_acquire)) return false;
                    const uint32_t toRender = static_cast<uint32_t>(std::min(
                        static_cast<uint64_t>(blockSize), totalSamples - rendered));
                    buffer.clear();
                    for (const auto& track : tracks) {
                        if (opts.stemTrackId != 0 &&
                            (!track || track->getId() != opts.stemTrackId)) continue;
                        if (track) track->processInto(
                            buffer, toRender, currentPos,
                            static_cast<Core::Engine::Track::OfflineRenderTap>(opts.renderTap));
                    }
                    const float* left = buffer.getReadPointer(0);
                    const float* right = opts.channels > 1 ? buffer.getReadPointer(1) : left;
                    if (!left || !right) return false;
                    analyzer.process(left, right, toRender);
                    for (uint32_t i = 0; i < toRender; ++i) {
                        const float l = std::isfinite(left[i]) ? left[i] : 0.0f;
                        const float r = std::isfinite(right[i]) ? right[i] : 0.0f;
                        peak = std::max(peak, std::abs(l));
                        if (opts.channels > 1) peak = std::max(peak, std::abs(r));
                    }
                    if (pass == writePass) {
                        if (opts.normalize && normalizationGain != 1.0f) {
                            float* writableLeft = buffer.getWritePointer(0);
                            float* writableRight = opts.channels > 1 ? buffer.getWritePointer(1) : writableLeft;
                            if (!writableLeft || !writableRight) return false;
                            for (uint32_t i = 0; i < toRender; ++i) {
                                writableLeft[i] = std::isfinite(writableLeft[i])
                                    ? writableLeft[i] * normalizationGain : 0.0f;
                                if (opts.channels > 1) {
                                    writableRight[i] = std::isfinite(writableRight[i])
                                        ? writableRight[i] * normalizationGain : 0.0f;
                                }
                            }
                            left = writableLeft;
                            right = writableRight;
                        }
                        if (opts.bitDepth == 16) {
                            if (!pcm16->writeFrames(left, opts.channels > 1 ? right : nullptr,
                                                     toRender)) return false;
                        } else if (opts.bitDepth == 24) {
                            if (!pcm24->writeFrames(left, opts.channels > 1 ? right : nullptr,
                                                     toRender)) return false;
                        } else {
                            const float* channels[2] = {left, right};
                            if (!float32->writeFrames(channels, toRender)) return false;
                        }
                    }
                    rendered += toRender;
                    currentPos += toRender;
                    if (onProgress) {
                        const float progress = static_cast<float>(rendered) / totalSamples;
                        onProgress(opts.normalize && pass == 0 ? progress * 0.5f
                                                               : (opts.normalize ? 0.5f + progress * 0.5f : progress));
                    }
                }
                if (opts.normalize && pass == 0 && peak > 0.0f) {
                    normalizationGain = 1.0f / peak;
                }
            }
        } catch (...) {
            return false;
        }
        if (opts.bitDepth == 16) return pcm16 && pcm16->finish();
        if (opts.bitDepth == 24) return pcm24 && pcm24->finish();
        return float32 && float32->finish();
    }

};

} // namespace Aura::IO
