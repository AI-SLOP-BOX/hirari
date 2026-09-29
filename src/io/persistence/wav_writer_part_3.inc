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
        if (path.empty() || channels == nullptr || channelCount == 0 || channelCount > 32 ||
            frames == 0 || sampleRate < 8000 || sampleRate > 384000) {
            errorStorage() = "invalid Rust WAVE export arguments";
            return false;
        }
        for (size_t channel = 0; channel < channelCount; ++channel) {
            if (channels[channel] == nullptr) {
                errorStorage() = "missing Rust WAVE export channel";
                return false;
            }
        }
        const std::filesystem::path output(path);
        std::error_code directoryError;
        const auto parent = output.parent_path().empty() ? std::filesystem::path(".") : output.parent_path();
        std::filesystem::create_directories(parent, directoryError);
        if (directoryError) {
            errorStorage() = "unable to create WAVE output directory";
            return false;
        }
        void* stream = nullptr;
        if (format == 0) {
            stream = hirari_wav_float32_stream_create(path.data(), path.size(), sampleRate,
                                                       static_cast<uint16_t>(channelCount));
        } else if (format == 3) {
            stream = hirari_wav_wave64_stream_create(path.data(), path.size(), frames, sampleRate,
                                                       static_cast<uint16_t>(channelCount));
        } else if (format == 1 || format == 2) {
            stream = hirari_wav_pcm_stream_create(path.data(), path.size(), frames, sampleRate,
                static_cast<uint16_t>(channelCount), format == 1 ? 16 : 24, 0);
        }
        if (stream == nullptr) {
            errorStorage() = "Rust WAVE stream writer could not be opened";
            return false;
        }
        struct StreamGuard {
            void* stream;
            uint8_t format;
            ~StreamGuard() {
                if (format == 0) hirari_wav_float32_stream_free(stream);
                else if (format == 3) hirari_wav_wave64_stream_free(stream);
                else hirari_wav_pcm_stream_free(stream);
            }
        } guard{stream, format};
        constexpr uint32_t kBlockFrames = 65'536;
        std::vector<const float*> block(channelCount);
        for (uint64_t offset = 0; offset < frames;) {
            const uint32_t count = static_cast<uint32_t>(std::min<uint64_t>(kBlockFrames, frames - offset));
            for (size_t channel = 0; channel < channelCount; ++channel) {
                block[channel] = channels[channel] + static_cast<size_t>(offset);
            }
            uint8_t written = 0;
            if (format == 0) written = hirari_wav_float32_stream_write(stream, block.data(), count);
            else if (format == 3) written = hirari_wav_wave64_stream_write(stream, block.data(), count);
            else written = hirari_wav_pcm_stream_write(stream, block.data(), count);
            if (!written) {
                errorStorage() = "Rust WAVE stream write failed";
                return false;
            }
            offset += count;
        }
        const uint8_t published = format == 0 ? hirari_wav_float32_stream_finish(stream)
            : format == 3 ? hirari_wav_wave64_stream_finish(stream)
                          : hirari_wav_pcm_stream_finish(stream);
        if (!published) {
            errorStorage() = "Rust WAVE stream publication failed";
            return false;
        }
        errorStorage().clear();
        return true;
    }

    static std::string& errorStorage() noexcept {
        thread_local std::string error;
        return error;
    }
};
