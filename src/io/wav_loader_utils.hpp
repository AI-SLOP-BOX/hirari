#pragma once

#include <vector>
#include <string>
#include <stdexcept>
#include <algorithm>
#include <cstdint>
#include <limits>
#include <cmath>
#include <filesystem>
#include "persistence/wav_writer.hpp"
#include "../core/io/audio_decoder.hpp"

namespace Hirari::IO {

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
        ::Hirari::Core::IO::WavDecoder decoder;
        if (!decoder.open(path)) {
            throw std::runtime_error("Invalid or unsupported WAVE file: " + path);
        }
        ::Hirari::Core::AudioBuffer decoded;
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

    /// Loads the bounded WAVE64 float32 export format through the Rust decoder.
    /// RIFF/RF64 continues to use the canonical Symphonia-backed decoder above.
    static std::vector<std::vector<float>> loadWave64(const std::string& path, WavInfo& outInfo) {
        HirariWave64DecodedOwned decoded{};
        const uint8_t ok = hirari_wave64_decode(path.data(), path.size(), &decoded);
        struct DecodedGuard {
            HirariWave64DecodedOwned* decoded;
            ~DecodedGuard() { hirari_wave64_decoded_free(decoded); }
        } guard{&decoded};
        if (!ok) {
            const std::string detail = decoded.error
                ? std::string(reinterpret_cast<const char*>(decoded.error), decoded.error_size)
                : std::string("unsupported or invalid file");
            throw std::runtime_error("WAVE64 decode failed: " + detail + ": " + path);
        }
        if (decoded.channels == 0 || decoded.channels > 32 || decoded.bit_depth != 32 ||
            decoded.frames > std::numeric_limits<size_t>::max() ||
            decoded.frames > std::numeric_limits<size_t>::max() / decoded.channels ||
            decoded.sample_count != static_cast<size_t>(decoded.frames) * decoded.channels ||
            decoded.sample_count > kMaximumDecodedBytes / sizeof(float) ||
            (decoded.sample_count != 0 && decoded.samples == nullptr)) {
            throw std::runtime_error("WAVE64 decoder returned invalid metadata: " + path);
        }
        outInfo.sampleRate = decoded.sample_rate;
        outInfo.numChannels = decoded.channels;
        outInfo.bitDepth = decoded.bit_depth;
        outInfo.numSamples = decoded.frames;
        std::vector<std::vector<float>> result(
            decoded.channels, std::vector<float>(static_cast<size_t>(decoded.frames)));
        for (uint64_t frame = 0; frame < decoded.frames; ++frame) {
            for (uint16_t channel = 0; channel < decoded.channels; ++channel) {
                result[channel][static_cast<size_t>(frame)] =
                    decoded.samples[static_cast<size_t>(frame) * decoded.channels + channel];
            }
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

} // namespace Hirari::IO
