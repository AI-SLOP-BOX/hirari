#pragma once

#include <vector>
#include <memory>
#include <string>
#include <stdexcept>
#include <algorithm>
#include <cstdint>
#include <limits>
#include <cmath>
#include "../core/rust_ffi.hpp"
#include "../core/io/audio_decoder.hpp"

namespace Hirari::IO {

/**
 * @brief WavLoader: High-fidelity WAVE file loader and normalizer.
 * Addresses the "missing file loading logic" from the review.
 */
class WavLoader {
public:
    // Mirrors MAX_WAVE64_BYTES in export_wave64.rs, which enforces the limit.
    // Retained so the native compile contract can size an oversize probe file.
    static constexpr uint64_t kMaximumDecodedBytes = 512ull * 1024ull * 1024ull;

    struct WavInfo {
        uint32_t sampleRate;
        uint16_t numChannels;
        uint16_t bitDepth;
        uint64_t numSamples;
    };

    /// Stable JSON-shaped diagnostic for FFI and native contract callers.
    /// Decoding, failure wording and statistics all run in Rust; this only
    /// copies the returned bytes.
    static std::string loadDiagnosticJson(const std::string& path,
                                          bool wave64 = false) noexcept {
        void* raw = hirari_wav_diagnostic_json_for_path(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), wave64);
        if (!raw) return {};
        std::unique_ptr<char, decltype(&hirari_wav_diagnostic_json_free)>
            json(static_cast<char*>(raw), &hirari_wav_diagnostic_json_free);
        const auto* data = hirari_wav_diagnostic_json_data(json.get());
        const size_t size = hirari_wav_diagnostic_json_size(json.get());
        return data ? std::string(reinterpret_cast<const char*>(data), size) : std::string();
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
        return writePlanar(path, buffer, sampleRate, 3);
    }

    static bool save(const std::string& path, const std::vector<std::vector<float>>& buffer, uint32_t sampleRate) {
        return writePlanar(path, buffer, sampleRate, 2);
    }

private:
    static bool writePlanar(const std::string& path,
                            const std::vector<std::vector<float>>& buffer,
                            uint32_t sampleRate, uint8_t format) {
        if (path.empty() || buffer.empty() || buffer.size() > std::numeric_limits<uint16_t>::max() ||
            buffer.front().empty()) return false;
        const size_t frames = buffer.front().size();
        std::vector<const float*> planes;
        planes.reserve(buffer.size());
        for (const auto& channel : buffer) {
            if (channel.size() != frames) return false;
            planes.push_back(channel.data());
        }
        return hirari_wav_write_planar(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), planes.data(),
            planes.size(), frames, sampleRate, format) == 0;
    }
};

} // namespace Hirari::IO
