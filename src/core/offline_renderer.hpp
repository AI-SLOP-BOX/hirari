#pragma once
#include <vector>
#include <fstream>
#include <algorithm>
#include <chrono>
#include <limits>
#include <cmath>
#include <functional>
#include <cstdint>
#include <string>
#include <array>
#include <filesystem>
#include <atomic>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "audio_processor_graph.hpp"
#include "../io/persistence/wav_writer.hpp"

namespace Aura::Core {

/**
 * @class OfflineRenderer
 * @brief Industrial Cluster-Parallel Rendering Engine.
 * Supports BWF v2.0, TPDF Dithering, and Distributed Dispatch.
 */
class OfflineRenderer {
public:
    using RenderCallback = std::function<void(AudioBuffer&, uint32_t)>;

    explicit OfflineRenderer(double sr, RenderCallback callback = {})
        : m_sampleRate(sr), m_renderCallback(std::move(callback)) {}

    void setRenderCallback(RenderCallback callback) {
        m_renderCallback = std::move(callback);
    }

    /**
     * @brief Renders project to file with CLUSTER SOVEREIGNTY.
     */
    void renderToFile(const std::string& path, double durationSeconds) {
        if (path.empty() || !std::isfinite(durationSeconds) || durationSeconds <= 0.0 ||
            !std::isfinite(m_sampleRate) || m_sampleRate <= 0.0 ||
            m_sampleRate > static_cast<double>(std::numeric_limits<uint32_t>::max())) return;
        const long double sampleCount = static_cast<long double>(durationSeconds) * m_sampleRate;
        if (sampleCount < 1.0L || sampleCount > static_cast<long double>(std::numeric_limits<uint64_t>::max())) return;
        const uint64_t totalSamples = static_cast<uint64_t>(sampleCount);
        const uint32_t blockSize = 1024;

        // Guard the byte-count multiplication before handing the value to
        // the streaming writer.  A malformed/very long duration must not
        // wrap into a small RIFF payload and silently produce a truncated
        // render.
        const auto dataBytes = totalSamples > std::numeric_limits<uint64_t>::max() / 6u
            ? 0u
            : totalSamples * 6u;
        if (dataBytes == 0) return;
        ::Aura::IO::Persistence::WavWriter::Pcm24StreamWriter stream(
            path, totalSamples, static_cast<uint32_t>(m_sampleRate), true);
        if (!stream.isOpen()) return;

        AudioBuffer buffer(2, blockSize);
        uint64_t rendered = 0;
        while (rendered < totalSamples) {
            const uint32_t currentBlock = static_cast<uint32_t>(std::min<uint64_t>(blockSize, totalSamples - rendered));
            buffer.clear(currentBlock);

            // A missing callback is a valid silent render, but never pretend
            // that a graph was processed. Hosts can supply the graph through
            // setRenderCallback without making OfflineRenderer own it.
            if (m_renderCallback) m_renderCallback(buffer, currentBlock);

            applyDither(buffer, currentBlock, 24); // Render at 24-bit

            if (!stream.writeFrames(buffer.getReadPointer(0), buffer.getReadPointer(1), currentBlock)) break;
            rendered += currentBlock;
        }
        if (rendered != totalSamples) {
            return;
        }
        (void)stream.finish();
    }

private:
    /**
     * @brief Writes Broadcast Wave Format (BWFv2) header with SMPTE metadata.
     */
    static void writeU16(std::ofstream& file, uint16_t value) {
        const uint8_t b[2] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8)};
        file.write(reinterpret_cast<const char*>(b), 2);
    }
    static void writeU32(std::ofstream& file, uint32_t value) {
        const uint8_t b[4] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
            static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24)};
        file.write(reinterpret_cast<const char*>(b), 4);
    }
    static void writeU64(std::ofstream& file, uint64_t value) {
        const uint8_t b[8] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
            static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24),
            static_cast<uint8_t>(value >> 32), static_cast<uint8_t>(value >> 40),
            static_cast<uint8_t>(value >> 48), static_cast<uint8_t>(value >> 56)};
        file.write(reinterpret_cast<const char*>(b), 8);
    }

    static void write24(std::ofstream& file, float value) {
        const float finite = std::isfinite(value) ? value : 0.0f;
        const int32_t sample = static_cast<int32_t>(std::lrint(std::clamp(finite, -1.0f, 1.0f) * 8388607.0f));
        const uint8_t b[3] = {static_cast<uint8_t>(sample), static_cast<uint8_t>(sample >> 8),
            static_cast<uint8_t>(sample >> 16)};
        file.write(reinterpret_cast<const char*>(b), 3);
    }

    void writeBWFHeader(std::ofstream& file, uint64_t totalSamples, bool rf64) {
        const uint64_t dataBytes = totalSamples * 6u;
        // BWF contains a 602-byte bext payload. RF64 adds a 36-byte ds64
        // chunk after the RIFF header, so its riffSize includes that extra
        // chunk as well (the value excludes the first 8 RIFF bytes).
        const uint64_t riffBytes = (rf64 ? 682u : 646u) + dataBytes;
        file.write(rf64 ? "RF64" : "RIFF", 4);
        writeU32(file, rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(riffBytes));
        file.write("WAVE", 4);

        file.write("bext", 4);
        writeU32(file, 602);
        const std::array<uint8_t, 602> bext{};
        file.write(reinterpret_cast<const char*>(bext.data()), static_cast<std::streamsize>(bext.size()));

        if (rf64) {
            file.write("ds64", 4);
            writeU32(file, 28);
            writeU64(file, riffBytes);
            writeU64(file, dataBytes);
            writeU64(file, totalSamples);
            writeU32(file, 0);
        }

        file.write("fmt ", 4);
        writeU32(file, 16); writeU16(file, 1); writeU16(file, 2);
        if (m_sampleRate > static_cast<double>(std::numeric_limits<uint32_t>::max())) return;
        const uint32_t sr = static_cast<uint32_t>(m_sampleRate);
        writeU32(file, sr); writeU32(file, sr * 6u); writeU16(file, 6); writeU16(file, 24);

        file.write("data", 4);
        writeU32(file, rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(dataBytes));
    }

    void applyDither(AudioBuffer& buffer, uint32_t sz, int bitDepth) {
        float lsb = 1.0f / std::pow(2.0f, (float)bitDepth - 1);
        for (uint32_t c = 0; c < 2; ++c) {
            float* ptr = buffer.getWritePointer(c);
            for (uint32_t i = 0; i < sz; ++i) {
                // TPDF Dither: Two independent random samples
                m_rng = m_rng * 1664525u + 1013904223u;
                const float r1 = static_cast<float>(m_rng >> 8) / 16777216.0f - 0.5f;
                m_rng = m_rng * 1664525u + 1013904223u;
                const float r2 = static_cast<float>(m_rng >> 8) / 16777216.0f - 0.5f;
                ptr[i] += (r1 + r2) * lsb;
            }
        }
    }

    double m_sampleRate;
    RenderCallback m_renderCallback;
    uint32_t m_rng = 0xA0A22026u;
};

} // namespace Aura::Core
