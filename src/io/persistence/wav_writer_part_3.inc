    static bool writePcm24Interleaved(const std::string& path,
                                      const std::vector<std::vector<float>>& channels,
                                      uint32_t sampleRate) {
        if (channels.empty() || channels.front().empty()) {
            errorStorage() = "invalid interleaved PCM24 output arguments";
            return false;
        }
        const auto frames = channels.front().size();
        std::vector<const float*> planes;
        planes.reserve(channels.size());
        for (const auto& channel : channels) {
            if (channel.size() != frames) {
                errorStorage() = "interleaved PCM24 channel lengths differ";
                return false;
            }
            planes.push_back(channel.data());
        }
        return writeThroughRust(path, planes.data(), planes.size(), frames, sampleRate, 2);
    }

    static bool write(const std::string& path, const float* l, const float* r,
                      uint64_t numSamples, uint32_t sampleRate) {
        const float* channels[] = {l, r};
        return writeThroughRust(path, channels, 2, numSamples, sampleRate, 0);
    }

private:
    static bool writeThroughRust(const std::string& path, const float* const* channels,
                                 size_t channelCount, uint64_t frames,
                                 uint32_t sampleRate, uint8_t format) {
        const uint8_t result = hirari_wav_write_planar(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), channels,
            channelCount, frames, sampleRate, format);
        switch (result) {
            case 0: errorStorage().clear(); return true;
            case 1: errorStorage() = "invalid Rust WAVE export arguments"; break;
            case 2: errorStorage() = "missing Rust WAVE export channel"; break;
            case 3: errorStorage() = "unable to create WAVE output directory"; break;
            case 4: errorStorage() = "Rust WAVE stream writer could not be opened"; break;
            case 5: errorStorage() = "Rust WAVE stream write failed"; break;
            default: errorStorage() = "Rust WAVE stream publication failed"; break;
        }
        return false;
    }

    static std::string& errorStorage() noexcept {
        thread_local std::string error;
        return error;
    }
};
