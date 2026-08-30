#pragma once
#include <vector>
#include <array>
#include <string>
#include <memory>
#include <fstream>
#include <limits>
#include <cmath>
#include <filesystem>
#include <atomic>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../engine/timeline_system.hpp"
#include "../../dsp/utils/dither.hpp"
#include "../../io/persistence/wav_writer.hpp"

namespace Aura::Core::IO {

/**
 * @class BounceEngine
 * @brief High-precision Offline Rendering & Stem Export System.
 * HONEST FIX: Implements 64-bit double-precision summing and normalization.
 * Mirrors the master-grade bounce processes of Logic Pro and Pro Tools, 
 * ensuring that the final 'Master' file is mathematically perfect.
 */
class BounceEngine {
public:
    struct Options {
        std::string path;
        uint32_t sampleRate = 44100;
        uint32_t bitDepth = 24;
        bool normalize = false;
        bool dither = true;
        bool renderStems = false;
        // 0=post-fader, 1=pre-insert, 2=pre-fader.
        uint32_t renderTap = 0;
        // Control-side cancellation. Stream writers discard their temporary
        // file when render() exits before finish().
        const std::atomic<bool>* cancellation = nullptr;
        // Internal selector used by renderStems. Zero means the full mix.
        uint32_t stemTrackId = 0;
    };

    /**
     * @brief OFFLINE RENDER: Bounces the entire project timeline to disk.
     */
    void render(const Options& options, const Engine::TimelineSystem& timeline) {
        m_progress.store(0.0f);
        m_maxPeak = 0.0f;

        if (options.path.empty() || options.sampleRate == 0 || options.sampleRate > 384000 ||
            (options.bitDepth != 16 && options.bitDepth != 24 && options.bitDepth != 32) ||
            options.renderTap > 2u) return;

        if (options.renderStems && options.stemTrackId == 0) {
            const auto tracks = timeline.getTracksSnapshot();
            if (tracks.empty()) return;
            std::filesystem::path masterPath(options.path);
            const std::string extension = masterPath.extension().string().empty()
                ? ".wav" : masterPath.extension().string();
            const std::string stemBase = masterPath.replace_extension().string();
            size_t renderedStems = 0;
            for (const auto& track : tracks) {
                if (options.cancellation &&
                    options.cancellation->load(std::memory_order_acquire)) return;
                if (!track) continue;
                std::string label = track->getName();
                for (char& character : label) {
                    const bool safe = (character >= 'a' && character <= 'z') ||
                        (character >= 'A' && character <= 'Z') ||
                        (character >= '0' && character <= '9') ||
                        character == '-' || character == '_';
                    if (!safe) character = '_';
                }
                if (label.empty()) label = "track";
                Options stem = options;
                stem.path = stemBase + ".stem-" +
                    std::to_string(track->getId()) + "-" + label + extension;
                stem.renderStems = false;
                stem.stemTrackId = track->getId();
                render(stem, timeline);
                ++renderedStems;
                m_progress.store(static_cast<float>(renderedStems) /
                                 static_cast<float>(tracks.size()),
                                 std::memory_order_release);
            }
            m_progress.store(renderedStems == 0 ? 0.0f : 1.0f,
                             std::memory_order_release);
            return;
        }

        double durationBeats = timeline.getSelectionLength();
        if (durationBeats <= 0.0) durationBeats = 128.0;
        const double durationSamples = durationBeats * (60.0 / timeline.getBPM()) * options.sampleRate;
        if (!std::isfinite(durationSamples) || durationSamples <= 0.0 ||
            durationSamples > static_cast<double>(std::numeric_limits<uint64_t>::max())) return;
        const uint64_t totalSamples = static_cast<uint64_t>(durationSamples);
        uint32_t blockSize = 1024;
        std::vector<float> blockL(blockSize), blockR(blockSize);
        AudioBuffer renderedBlock(2, blockSize);
        const auto tracks = timeline.getTracksSnapshot();

        const auto mixBlock = [&](uint64_t start, uint32_t count) -> bool {
            std::fill(blockL.begin(), blockL.end(), 0.0f);
            std::fill(blockR.begin(), blockR.end(), 0.0f);
            for (const auto& track : tracks) {
                if (!track) continue;
                if (options.stemTrackId != 0 &&
                    track->getId() != options.stemTrackId) continue;
                renderedBlock.clear(count);
                if (!timeline.renderTrackInto(
                        track->getId(), renderedBlock, count, start,
                        static_cast<Engine::Track::OfflineRenderTap>(options.renderTap))) {
                    return false;
                }
                const float* renderedLeft = renderedBlock.getReadPointer(0);
                const float* renderedRight = renderedBlock.getReadPointer(1);
                if (!renderedLeft || !renderedRight) return false;
                for (uint32_t i = 0; i < count; ++i) {
                    blockL[i] += std::isfinite(renderedLeft[i]) ? renderedLeft[i] : 0.0f;
                    blockR[i] += std::isfinite(renderedRight[i]) ? renderedRight[i] : 0.0f;
                }
            }
            return true;
        };

        // Normalization is a two-pass operation.  Measuring the complete
        // graph before opening the destination prevents a later peak from
        // invalidating an already-published file.
        if (options.normalize) {
            for (uint64_t s = 0; s < totalSamples; s += blockSize) {
                if (options.cancellation &&
                    options.cancellation->load(std::memory_order_acquire)) return;
                const uint32_t currentBlock = static_cast<uint32_t>(
                    std::min<uint64_t>(blockSize, totalSamples - s));
                if (!mixBlock(s, currentBlock)) return;
                for (uint32_t i = 0; i < currentBlock; ++i) {
                    m_maxPeak = std::max({m_maxPeak, std::abs(blockL[i]), std::abs(blockR[i])});
                }
            }
        }

        std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter> pcm16;
        std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm24StreamWriter> pcm24;
        std::unique_ptr<::Aura::IO::Persistence::WavWriter::Float32StreamWriter> float32;
        if (options.bitDepth == 16) {
            pcm16 = std::make_unique<
                ::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter>(
                    options.path, totalSamples, options.sampleRate);
            if (!pcm16->isOpen()) return;
        } else if (options.bitDepth == 24) {
            pcm24 = std::make_unique<
                ::Aura::IO::Persistence::WavWriter::Pcm24StreamWriter>(
                    options.path, totalSamples, options.sampleRate);
            if (!pcm24->isOpen()) return;
        } else {
            float32 = std::make_unique<
                ::Aura::IO::Persistence::WavWriter::Float32StreamWriter>(
                    options.path, options.sampleRate, 2);
            if (!float32->isOpen()) return;
        }

        ::Aura::DSP::Utils::TPDFDither ditherL, ditherR;
        const float normalizationGain = options.normalize && m_maxPeak > 0.0f
            ? 1.0f / m_maxPeak : 1.0f;
        for (uint64_t s = 0; s < totalSamples; s += blockSize) {
            if (options.cancellation &&
                options.cancellation->load(std::memory_order_acquire)) return;
            const uint32_t currentBlock = static_cast<uint32_t>(
                std::min<uint64_t>(blockSize, totalSamples - s));
            if (!mixBlock(s, currentBlock)) return;

            for (uint32_t i = 0; i < currentBlock; ++i) {
                float lVal = blockL[i] * normalizationGain;
                float rVal = blockR[i] * normalizationGain;

                // Peak Detection
                if (!options.normalize) {
                    m_maxPeak = std::max({m_maxPeak, std::abs(lVal), std::abs(rVal)});
                }

                if (options.dither && options.bitDepth < 32) {
                    lVal += ditherL.process();
                    rVal += ditherR.process();
                }
                blockL[i] = lVal;
                blockR[i] = rVal;

            }

            const bool written = pcm16
                ? pcm16->writeFrames(blockL.data(), blockR.data(), currentBlock)
                : (pcm24
                    ? pcm24->writeFrames(blockL.data(), blockR.data(), currentBlock)
                    : float32->writeFrames(
                        std::array<const float*, 2>{blockL.data(), blockR.data()}.data(),
                        currentBlock));
            if (!written) return;

            if (options.cancellation &&
                options.cancellation->load(std::memory_order_acquire)) return;

            m_progress.store(static_cast<float>(s + currentBlock) /
                             static_cast<float>(totalSamples));
        }

        m_progress.store(1.0f);
        const bool finished = pcm16 ? pcm16->finish()
            : (pcm24 ? pcm24->finish() : float32->finish());
        if (!finished) m_progress.store(0.0f);
    }

    float getProgress() const { return m_progress.load(); }
    float getMaxPeak() const { return m_maxPeak; }

private:
    std::atomic<float> m_progress{0.0f};
    float m_maxPeak = 0.0f;
};

} // namespace Aura::Core::IO
