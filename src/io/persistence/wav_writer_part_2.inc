    static const std::string& lastError() noexcept { return errorStorage(); }
    /// Explicit WAVE64 float32 path for large-file delivery. The existing
    /// write() API remains RIFF/RF64-compatible for callers that require WAV.
    static bool writeWave64(const std::string& path, const float* l, const float* r,
                            uint64_t numSamples, uint32_t sampleRate) {
        const float* channels[] = {l, r};
        return writeThroughRust(path, channels, 2, numSamples, sampleRate, 3);
    }

    /// Canonical interleaved WAVE64 float32 writer for multichannel exports.
    /// The channel-major input is validated before publication so callers can
    /// never receive a partially shaped file.
    static bool writeWave64Interleaved(const std::string& path,
                                       const std::vector<std::vector<float>>& channels,
                                       uint32_t sampleRate) {
        if (channels.empty() || channels.front().empty()) {
            errorStorage() = "invalid WAVE64 interleaved arguments";
            return false;
        }
        const auto frames = channels.front().size();
        std::vector<const float*> planes;
        planes.reserve(channels.size());
        for (const auto& channel : channels) {
            if (channel.size() != frames) {
                errorStorage() = "WAVE64 channel lengths differ";
                return false;
            }
            planes.push_back(channel.data());
        }
        return writeThroughRust(path, planes.data(), planes.size(), frames, sampleRate, 3);
    }

    /// Compatibility PCM16 path for legacy native callers.  It shares the
    /// same atomic publication and durability contract as the float writer.
    static bool writePcm16(const std::string& path, const float* l, const float* r,
                           uint64_t numSamples, uint32_t sampleRate) {
        const float* channels[] = {l, r};
        return writeThroughRust(path, channels, 2, numSamples, sampleRate, 1);
    }

    /// Canonical little-endian PCM24 stereo output for legacy bounce paths.
    static bool writePcm24(const std::string& path, const float* l, const float* r,
                           uint64_t numSamples, uint32_t sampleRate) {
        const float* channels[] = {l, r};
        return writeThroughRust(path, channels, 2, numSamples, sampleRate, 2);
    }

    static bool writePcm16Interleaved(const std::string& path,
                                      const std::vector<std::vector<float>>& channels,
                                      uint32_t sampleRate) {
        if (channels.empty() || channels.front().empty()) {
            errorStorage() = "invalid interleaved PCM16 output arguments";
            return false;
        }
        const auto frames = channels.front().size();
        std::vector<const float*> planes;
        planes.reserve(channels.size());
        for (const auto& channel : channels) {
            if (channel.size() != frames) {
                errorStorage() = "interleaved PCM16 channel lengths differ";
                return false;
            }
            planes.push_back(channel.data());
        }
        return writeThroughRust(path, planes.data(), planes.size(), frames, sampleRate, 1);
    }
