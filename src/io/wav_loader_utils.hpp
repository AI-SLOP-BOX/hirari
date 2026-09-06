#pragma once

#include <vector>
#include <array>
#include <string>
#include <fstream>
#include <stdexcept>
#include <algorithm>
#include <cstdint>
#include <cstring>
#include <limits>
#include <cmath>
#include <filesystem>
#include "persistence/wav_writer.hpp"
#include <chrono>
#include <atomic>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../core/io/audio_decoder.hpp"

namespace Aura::IO {

/**
 * @brief WavLoader: High-fidelity WAVE file loader and normalizer.
 * Addresses the "missing file loading logic" from the review.
 */
class WavLoader {
public:
    // Keep the WAVE64 path consistent with the canonical WAV decoder: the
    // reader is an offline helper, but it must still reject oversized input
    // before allocating a file-sized buffer.
    static constexpr uint64_t kMaximumDecodedBytes = 512ull * 1024ull * 1024ull;

    struct WavInfo {
        uint32_t sampleRate;
        uint16_t numChannels;
        uint16_t bitDepth;
        uint64_t numSamples;
    };

    struct DiagnosticResult {
        bool ok = false;
        std::string format;
        std::string error;
        WavInfo info{};
        std::vector<std::vector<float>> channels;
    };

    /// Structured native read boundary.  The throwing load APIs remain for
    /// legacy callers, while bridge/diagnostic callers can preserve the
    /// actual parser failure instead of collapsing it to `false`.
    static DiagnosticResult loadDiagnostic(const std::string& path,
                                           bool wave64 = false) noexcept {
        DiagnosticResult result;
        result.format = wave64 ? "WAVE64" : "WAVE";
        try {
            result.channels = wave64 ? loadWave64(path, result.info)
                                     : load(path, result.info);
            result.ok = true;
        } catch (const std::exception& exception) {
            result.error = exception.what();
        } catch (...) {
            result.error = "unknown WAVE decoder failure";
        }
        return result;
    }

    /// Stable JSON-shaped diagnostic for FFI and native contract callers.
    /// This is deliberately dependency-free because the reader is used by
    /// low-level native compile contracts as well as the application bridge.
    static std::string loadDiagnosticJson(const std::string& path,
                                          bool wave64 = false) noexcept {
        const auto result = loadDiagnostic(path, wave64);
        const auto escape = [](const std::string& value) {
            std::string escaped;
            escaped.reserve(value.size() + 8);
            for (const char character : value) {
                switch (character) {
                case '\\': escaped += "\\\\"; break;
                case '"': escaped += "\\\""; break;
                case '\n': escaped += "\\n"; break;
                case '\r': escaped += "\\r"; break;
                case '\t': escaped += "\\t"; break;
                default: escaped += character; break;
                }
            }
            return escaped;
        };
        std::string json = "{\"ok\":" + std::string(result.ok ? "true" : "false") +
            ",\"format\":\"" + escape(result.format) + "\"";
        if (result.ok) {
            json += ",\"sample_rate\":" + std::to_string(result.info.sampleRate) +
                ",\"channels\":" + std::to_string(result.info.numChannels) +
                ",\"bit_depth\":" + std::to_string(result.info.bitDepth) +
                ",\"samples\":" + std::to_string(result.info.numSamples);
            double peak = 0.0;
            long double sumSquares = 0.0L;
            uint64_t finiteSamples = 0;
            uint64_t nonFiniteSamples = 0;
            for (const auto& channel : result.channels) {
                for (const float sample : channel) {
                    if (!std::isfinite(sample)) {
                        ++nonFiniteSamples;
                        continue;
                    }
                    const double magnitude = std::abs(static_cast<double>(sample));
                    peak = std::max(peak, magnitude);
                    sumSquares += static_cast<long double>(sample) * sample;
                    ++finiteSamples;
                }
            }
            const double meanSquare = finiteSamples == 0
                ? 0.0 : static_cast<double>(sumSquares / finiteSamples);
            const double rmsLufsEstimate = meanSquare > 0.0
                ? std::max(-120.0, 10.0 * std::log10(meanSquare) - 0.691)
                : -120.0;
            json += ",\"peak_linear\":" + std::to_string(peak) +
                ",\"rms_lufs_estimate\":" + std::to_string(rmsLufsEstimate) +
                ",\"finite_samples\":" + std::to_string(finiteSamples) +
                ",\"non_finite_samples\":" + std::to_string(nonFiniteSamples);
        } else {
            json += ",\"error\":\"" + escape(result.error) + "\"";
        }
        return json + "}";
    }

    /**
     * @brief Loads a WAV file into a multi-channel buffer with proper chunk-seeking.
     */
    /// Canonical decode entrypoint. All normal callers now use the same
    /// checked RIFF/RF64 parser as the core importer and native contract.
    static std::vector<std::vector<float>> load(const std::string& path, WavInfo& outInfo) {
        ::Aura::Core::IO::WavDecoder decoder;
        if (!decoder.open(path)) {
            throw std::runtime_error("Invalid or unsupported WAVE file: " + path);
        }
        ::Aura::Core::AudioBuffer decoded;
        decoder.decodeFull(decoded);
        if (decoded.isEmpty()) {
            throw std::runtime_error("WAVE file contains no decodable samples: " + path);
        }

        outInfo.sampleRate = static_cast<uint32_t>(decoder.getSampleRate());
        outInfo.numChannels = static_cast<uint16_t>(decoder.getNumChannels());
        outInfo.bitDepth = static_cast<uint16_t>(decoder.getBitsPerSample());
        outInfo.numSamples = decoded.getNumSamples();

        std::vector<std::vector<float>> result(
            decoded.getNumChannels(), std::vector<float>(decoded.getNumSamples()));
        for (uint32_t channel = 0; channel < decoded.getNumChannels(); ++channel) {
            const float* source = decoded.getReadPointer(channel);
            std::copy(source, source + decoded.getNumSamples(), result[channel].begin());
        }
        return result;
    }

    /// Loads the bounded WAVE64 float32 export format. RIFF/RF64 continues to
    /// use the canonical decoder above; this explicit entrypoint avoids making
    /// the legacy parser guess between four-byte and GUID chunk layouts.
    static std::vector<std::vector<float>> loadWave64(const std::string& path, WavInfo& outInfo) {
        const std::array<uint8_t, 16> riff{0x52,0x49,0x46,0x46,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const std::array<uint8_t, 16> wave{0x57,0x41,0x56,0x45,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const std::array<uint8_t, 16> fmtId{0x66,0x6d,0x74,0x20,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const std::array<uint8_t, 16> dataId{0x64,0x61,0x74,0x61,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        std::ifstream file(path, std::ios::binary);
        if (!file.is_open()) throw std::runtime_error("WAVE64 file not found: " + path);
        file.seekg(0, std::ios::end); const auto end = file.tellg();
        if (end < 40) throw std::runtime_error("WAVE64 header is truncated.");
        const auto size = static_cast<uint64_t>(end);
        if (size > kMaximumDecodedBytes + 1024ull * 1024ull ||
            size > static_cast<uint64_t>(std::numeric_limits<size_t>::max())) {
            throw std::runtime_error("WAVE64 file exceeds the decoded size limit.");
        }
        file.seekg(0, std::ios::beg);
        std::vector<uint8_t> bytes(static_cast<size_t>(size));
        file.read(reinterpret_cast<char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
        if (!file || !std::equal(riff.begin(), riff.end(), bytes.begin()) || !std::equal(wave.begin(), wave.end(), bytes.begin() + 24)) throw std::runtime_error("Invalid WAVE64 header.");
        const auto u64 = [](const uint8_t* p) { uint64_t v = 0; for (unsigned i = 0; i < 8; ++i) v |= static_cast<uint64_t>(p[i]) << (i * 8); return v; };
        if (u64(bytes.data() + 16) != size) throw std::runtime_error("WAVE64 file size is inconsistent.");
        bool haveFmt = false, haveData = false; uint64_t dataStart = 0, dataSize = 0;
        size_t pos = 40;
        while (pos < bytes.size()) {
            if (bytes.size() - pos < 24) throw std::runtime_error("WAVE64 chunk header is truncated.");
            const uint64_t chunkSize = u64(bytes.data() + pos + 16);
            if (chunkSize < 24 || chunkSize > bytes.size() - pos) throw std::runtime_error("WAVE64 chunk exceeds file bounds.");
            const size_t payload = pos + 24; const size_t payloadSize = static_cast<size_t>(chunkSize - 24);
            if (std::equal(fmtId.begin(), fmtId.end(), bytes.begin() + pos)) {
                if (payloadSize < 16 || bytes[payload] != 3 || bytes[payload + 1] != 0) throw std::runtime_error("Unsupported WAVE64 format.");
                outInfo.numChannels = static_cast<uint16_t>(bytes[payload + 2] | (bytes[payload + 3] << 8));
                outInfo.sampleRate = static_cast<uint32_t>(bytes[payload + 4] | (bytes[payload + 5] << 8) | (bytes[payload + 6] << 16) | (bytes[payload + 7] << 24));
                outInfo.bitDepth = static_cast<uint16_t>(bytes[payload + 14] | (bytes[payload + 15] << 8));
                if (outInfo.numChannels == 0 || outInfo.numChannels > 32 || outInfo.sampleRate == 0 || outInfo.bitDepth != 32) throw std::runtime_error("Unsupported WAVE64 format values.");
                haveFmt = true;
            } else if (std::equal(dataId.begin(), dataId.end(), bytes.begin() + pos)) { dataStart = payload; dataSize = payloadSize; haveData = true; }
            if (chunkSize > std::numeric_limits<uint64_t>::max() - 7u)
                throw std::runtime_error("WAVE64 chunk alignment overflows.");
            const uint64_t aligned = (chunkSize + 7u) & ~uint64_t{7u};
            if (aligned > static_cast<uint64_t>(bytes.size() - pos))
                throw std::runtime_error("WAVE64 chunk padding exceeds file bounds.");
            pos += static_cast<size_t>(aligned);
        }
        if (!haveFmt || !haveData || dataSize % 4 != 0 || (dataSize / 4) % outInfo.numChannels != 0) throw std::runtime_error("WAVE64 fmt/data chunks are invalid.");
        outInfo.numSamples = dataSize / 4 / outInfo.numChannels;
        std::vector<std::vector<float>> result(outInfo.numChannels, std::vector<float>(outInfo.numSamples));
        for (uint64_t frame = 0; frame < outInfo.numSamples; ++frame) for (uint16_t channel = 0; channel < outInfo.numChannels; ++channel) {
            float value = 0.0f; std::memcpy(&value, bytes.data() + dataStart + (frame * outInfo.numChannels + channel) * 4, 4);
            if (!std::isfinite(value)) throw std::runtime_error("WAVE64 contains a non-finite sample.");
            result[channel][frame] = value;
        }
        return result;
    }

    /// Compatibility entrypoint.  Keep the old symbol for downstream clients,
    /// but route it through the canonical checked RIFF/RF64 decoder so there
    /// is only one WAVE parsing implementation and one set of format rules.
    static std::vector<std::vector<float>> loadLegacy(const std::string& path, WavInfo& outInfo) {
        return load(path, outInfo);
    }
};

/**
 * @brief WavSaver: Professional-grade WAVE exporter for Bouncing and Master export.
 */
class WavSaver {
public:
    /// Explicit large-file export entrypoint. Keeping this separate from
    /// `save` prevents a file extension or size guess from silently changing
    /// the interchange format, while still routing every channel layout
    /// through the canonical persistence writer.
    static bool saveWave64(const std::string& path,
                           const std::vector<std::vector<float>>& buffer,
                           uint32_t sampleRate) {
        if (path.empty() || buffer.empty() || sampleRate == 0 || sampleRate > 384000 ||
            buffer.size() > std::numeric_limits<uint16_t>::max() || buffer.front().empty()) {
            return false;
        }
        const auto sampleCount = buffer.front().size();
        for (const auto& channel : buffer) {
            if (channel.size() != sampleCount) return false;
        }
        if (std::filesystem::path(path).has_parent_path()) {
            std::error_code directoryError;
            std::filesystem::create_directories(std::filesystem::path(path).parent_path(), directoryError);
            if (directoryError) return false;
        }
        return Persistence::WavWriter::writeWave64Interleaved(path, buffer, sampleRate);
    }

    static bool save(const std::string& path, const std::vector<std::vector<float>>& buffer, uint32_t sampleRate) {
        if (path.empty() || buffer.empty() || sampleRate == 0 || sampleRate > 384000 ||
            buffer.size() > std::numeric_limits<uint16_t>::max() || buffer.front().empty()) return false;

        const uint16_t numChannels = static_cast<uint16_t>(buffer.size());
        const size_t sampleCount = buffer[0].size();
        for (const auto& channel : buffer) {
            if (channel.size() != sampleCount) return false;
        }
        const std::filesystem::path output(path);
        // Stereo PCM24 is the dominant bounce path. Use the canonical writer
        // so atomic publication, RF64 metadata, and durability cannot drift
        // from the native compatibility APIs. Keep the multichannel path
        // below for layouts the canonical stereo API does not represent.
        if (numChannels == 2) {
            if (output.has_parent_path()) {
                std::error_code directoryError;
                std::filesystem::create_directories(output.parent_path(), directoryError);
                if (directoryError) return false;
            }
            return Persistence::WavWriter::writePcm24(
                path, buffer[0].data(), buffer[1].data(),
                static_cast<uint64_t>(sampleCount), sampleRate);
        }
        // All other channel layouts use the same canonical interleaved writer.
        return Persistence::WavWriter::writePcm24Interleaved(path, buffer, sampleRate);
    }
};

} // namespace Aura::IO
