#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include "../../io/persistence/wav_writer.hpp"

namespace Aura::Core::Utils {

// Compatibility facade for older native callers. Encoding, atomic
// publication, fsync, RF64 metadata, and error reporting belong exclusively
// to the persistence writer so product paths cannot drift apart.
class WavWriter {
public:
    static bool writeWave64Interleaved(
        const std::string& path, const std::vector<std::vector<float>>& channels,
        uint32_t sampleRate) {
        return Aura::IO::Persistence::WavWriter::writeWave64Interleaved(
            path, channels, sampleRate);
    }

    static bool writeWave64(const std::string& path, const float* left,
                            const float* right, uint32_t samples,
                            uint32_t sampleRate) {
        return Aura::IO::Persistence::WavWriter::writeWave64(
            path, left, right, static_cast<uint64_t>(samples), sampleRate);
    }

    static bool write(const std::string& path, const float* left,
                      const float* right, uint32_t samples,
                      uint32_t sampleRate) {
        return Aura::IO::Persistence::WavWriter::writePcm16(
            path, left, right, static_cast<uint64_t>(samples), sampleRate);
    }

    static bool writePcm24(const std::string& path, const float* left,
                           const float* right, uint32_t samples,
                           uint32_t sampleRate) {
        return Aura::IO::Persistence::WavWriter::writePcm24(
            path, left, right, static_cast<uint64_t>(samples), sampleRate);
    }
};

} // namespace Aura::Core::Utils
