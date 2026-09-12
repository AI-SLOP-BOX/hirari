                return false;
            }
            for (uint16_t channel = 0; channel < m_channels; ++channel) {
                if (!channels[channel]) {
                    m_error = "null WAVE64 stream channel";
                    return false;
                }
            }
            for (uint32_t frame = 0; frame < count; ++frame) {
                for (uint16_t channel = 0; channel < m_channels; ++channel) {
                    const float value = std::isfinite(channels[channel][frame])
                        ? channels[channel][frame] : 0.0f;
                    m_file.write(reinterpret_cast<const char*>(&value), sizeof(value));
                }
            }
            m_written += count;
            if (!m_file) { m_error = "WAVE64 stream write failed"; return false; }
            return true;
        }

        bool finish() {
            if (m_finished) return m_error.empty();
            if (!isOpen() || m_written != m_frames) {
                m_error = "WAVE64 stream frame count mismatch";
                discard();
                return false;
            }
            const uint8_t zero[7] = {};
            const auto padding = static_cast<size_t>(m_paddedPayload - m_payload);
            if (padding) m_file.write(reinterpret_cast<const char*>(zero), static_cast<std::streamsize>(padding));
            m_file.flush();
            const bool written = static_cast<bool>(m_file);
            m_file.close();
            if (!written) { m_error = "WAVE64 stream flush failed"; discard(); return false; }
#if !defined(_WIN32)
            const int fd = ::open(m_temporary.c_str(), O_RDONLY);
            if (fd < 0 || ::fsync(fd) != 0) {
                if (fd >= 0) ::close(fd);
                m_error = "WAVE64 stream sync failed";
                discard();
                return false;
            }
            ::close(fd);
#endif
            std::error_code ec;
            std::filesystem::rename(m_temporary, m_output, ec);
            if (ec) { m_error = "WAVE64 stream atomic rename failed"; discard(); return false; }
#if !defined(_WIN32)
            if (!WavWriter::syncParentDirectory(m_output)) {
                m_error = "WAVE64 stream directory sync failed";
                discard();
                return false;
            }
#endif
            m_finished = true;
            return true;
        }

    private:
        void put(const void* data, std::streamsize size) { m_file.write(static_cast<const char*>(data), size); }
        void putU16(uint16_t value) { const uint8_t b[2] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8)}; put(b, 2); }
        void putU32(uint32_t value) { const uint8_t b[4] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8), static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24)}; put(b, 4); }
        void putU64(uint64_t value) { const uint8_t b[8] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8), static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24), static_cast<uint8_t>(value >> 32), static_cast<uint8_t>(value >> 40), static_cast<uint8_t>(value >> 48), static_cast<uint8_t>(value >> 56)}; put(b, 8); }
        void discard() noexcept { if (m_file.is_open()) m_file.close(); std::error_code ec; if (!m_temporary.empty()) std::filesystem::remove(m_temporary, ec); m_finished = true; }
        std::filesystem::path m_output, m_temporary;
        std::ofstream m_file;
        uint64_t m_frames = 0, m_written = 0, m_payload = 0, m_paddedPayload = 0;
        uint16_t m_channels = 2;
        std::string m_error;
        bool m_open = false, m_finished = false;
    };

    static const std::string& lastError() noexcept { return errorStorage(); }
    /// Explicit WAVE64 float32 path for large-file delivery. The existing
    /// write() API remains RIFF/RF64-compatible for callers that require WAV.
    static bool writeWave64(const std::string& path, const float* l, const float* r,
                            uint64_t numSamples, uint32_t sampleRate) {
        if (path.empty() || l == nullptr || r == nullptr || numSamples == 0 ||
            sampleRate == 0 || sampleRate > 384000 ||
            numSamples > std::numeric_limits<uint64_t>::max() / 8u) {
            errorStorage() = "invalid WAVE64 output arguments";
            return false;
        }
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto id = sequence.fetch_add(1, std::memory_order_relaxed);
        const std::filesystem::path temporary = output.string() + ".tmp-aura-wave64-" +
            std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                0
#else
                ::getpid()
#endif
            )) + "-" + std::to_string(id);
        if (!reserveTemporary(temporary)) { errorStorage() = "WAVE64 temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open WAVE64 output"; return false; }
        const uint64_t payload = numSamples * 8u;
        const uint64_t dataChunk = 24u + payload + ((8u - (payload % 8u)) % 8u);
        const uint64_t fileSize = 40u + 40u + dataChunk;
        const uint8_t riff[16] = {0x52,0x49,0x46,0x46,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const uint8_t wave[16] = {0x57,0x41,0x56,0x45,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const uint8_t fmt[16] = {0x66,0x6d,0x74,0x20,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const uint8_t data[16] = {0x64,0x61,0x74,0x61,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const auto u16 = [&file](uint16_t v) { const uint8_t b[2] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8)}; file.write(reinterpret_cast<const char*>(b), 2); };
        const auto u32 = [&file](uint32_t v) { const uint8_t b[4] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8), static_cast<uint8_t>(v >> 16), static_cast<uint8_t>(v >> 24)}; file.write(reinterpret_cast<const char*>(b), 4); };
        const auto u64 = [&file](uint64_t v) { const uint8_t b[8] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8), static_cast<uint8_t>(v >> 16), static_cast<uint8_t>(v >> 24), static_cast<uint8_t>(v >> 32), static_cast<uint8_t>(v >> 40), static_cast<uint8_t>(v >> 48), static_cast<uint8_t>(v >> 56)}; file.write(reinterpret_cast<const char*>(b), 8); };
        file.write(reinterpret_cast<const char*>(riff), 16); u64(fileSize); file.write(reinterpret_cast<const char*>(wave), 16);
        file.write(reinterpret_cast<const char*>(fmt), 16); u64(40); u16(3); u16(2); u32(sampleRate); u32(sampleRate * 8u); u16(8); u16(32);
        file.write(reinterpret_cast<const char*>(data), 16); u64(dataChunk);
        for (uint64_t i = 0; i < numSamples; ++i) { const float left = std::isfinite(l[i]) ? l[i] : 0.0f; const float right = std::isfinite(r[i]) ? r[i] : 0.0f; file.write(reinterpret_cast<const char*>(&left), 4); file.write(reinterpret_cast<const char*>(&right), 4); }
        const uint8_t zero[7] = {}; const auto padding = static_cast<size_t>((8u - (payload % 8u)) % 8u); if (padding) file.write(reinterpret_cast<const char*>(zero), static_cast<std::streamsize>(padding));
        file.flush(); const bool written = static_cast<bool>(file); file.close();
        if (!written) { std::error_code ec; std::filesystem::remove(temporary, ec); errorStorage() = "WAVE64 write failed"; return false; }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY); if (fd < 0 || ::fsync(fd) != 0) { if (fd >= 0) ::close(fd); std::error_code ec; std::filesystem::remove(temporary, ec); errorStorage() = "WAVE64 sync failed"; return false; } ::close(fd);
#endif
        std::error_code ec; std::filesystem::rename(temporary, output, ec); if (ec) std::filesystem::remove(temporary, ec);
        if (ec) { errorStorage() = "WAVE64 atomic rename failed"; return false; }
#if !defined(_WIN32)
        if (!syncParentDirectory(output)) { errorStorage() = "WAVE64 directory sync failed"; return false; }
#endif
        errorStorage().clear(); return true;
    }

    /// Canonical interleaved WAVE64 float32 writer for multichannel exports.
    /// The channel-major input is validated before publication so callers can
    /// never receive a partially shaped file.
    static bool writeWave64Interleaved(const std::string& path,
                                       const std::vector<std::vector<float>>& channels,
                                       uint32_t sampleRate) {
        if (path.empty() || channels.empty() || channels.size() > 65535u ||
            sampleRate == 0 || sampleRate > 384000 || channels.front().empty()) {
            errorStorage() = "invalid WAVE64 interleaved arguments";
            return false;
        }
        const uint64_t count = channels.front().size();
        for (const auto& channel : channels) {
            if (channel.size() != count) {
                errorStorage() = "WAVE64 channel lengths differ";
                return false;
            }
        }
        const uint64_t channelCount = channels.size();
        const uint64_t payload = count * channelCount * 4u;
        if (channelCount > std::numeric_limits<uint16_t>::max() ||
            count > std::numeric_limits<uint64_t>::max() / (channelCount * 4u) ||
            payload > std::numeric_limits<uint64_t>::max() - 7u) {
            errorStorage() = "WAVE64 data is too large";
            return false;
        }
        const uint64_t paddedPayload = payload + ((8u - (payload % 8u)) % 8u);
        const uint64_t dataChunk = 24u + paddedPayload;
        const uint64_t fileSize = 40u + 40u + dataChunk;
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto temporary = output.string() + ".tmp-aura-wave64-mc-" +
            std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                0
#else
                ::getpid()
#endif
            )) + "-" + std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
        std::error_code ec;
        if (!reserveTemporary(temporary)) { errorStorage() = "WAVE64 temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open WAVE64 output"; return false; }
        const uint8_t riff[16] = {0x52,0x49,0x46,0x46,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const uint8_t wave[16] = {0x57,0x41,0x56,0x45,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const uint8_t fmt[16] = {0x66,0x6d,0x74,0x20,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const uint8_t data[16] = {0x64,0x61,0x74,0x61,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
        const auto u16 = [&file](uint16_t v) { const uint8_t b[2] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8)}; file.write(reinterpret_cast<const char*>(b), 2); };
        const auto u32 = [&file](uint32_t v) { const uint8_t b[4] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8), static_cast<uint8_t>(v >> 16), static_cast<uint8_t>(v >> 24)}; file.write(reinterpret_cast<const char*>(b), 4); };
        const auto u64 = [&file](uint64_t v) { const uint8_t b[8] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8), static_cast<uint8_t>(v >> 16), static_cast<uint8_t>(v >> 24), static_cast<uint8_t>(v >> 32), static_cast<uint8_t>(v >> 40), static_cast<uint8_t>(v >> 48), static_cast<uint8_t>(v >> 56)}; file.write(reinterpret_cast<const char*>(b), 8); };
        file.write(reinterpret_cast<const char*>(riff), 16); u64(fileSize); file.write(reinterpret_cast<const char*>(wave), 16);
        file.write(reinterpret_cast<const char*>(fmt), 16); u64(40); u16(3); u16(static_cast<uint16_t>(channelCount));
        u32(sampleRate); u32(sampleRate * static_cast<uint32_t>(channelCount) * 4u);
        u16(static_cast<uint16_t>(channelCount * 4u)); u16(32);
        file.write(reinterpret_cast<const char*>(data), 16); u64(dataChunk);
        for (uint64_t i = 0; i < count; ++i) for (const auto& channel : channels) {
            const float value = std::isfinite(channel[i]) ? channel[i] : 0.0f;
            file.write(reinterpret_cast<const char*>(&value), 4);
        }
        const uint8_t zero[7] = {}; const auto padding = static_cast<size_t>(paddedPayload - payload);
        if (padding) file.write(reinterpret_cast<const char*>(zero), static_cast<std::streamsize>(padding));
        file.flush(); const bool written = static_cast<bool>(file); file.close();
        if (!written) { std::filesystem::remove(temporary, ec); errorStorage() = "WAVE64 write failed"; return false; }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) { if (fd >= 0) ::close(fd); std::filesystem::remove(temporary, ec); errorStorage() = "WAVE64 sync failed"; return false; }
        ::close(fd);
#endif
        std::filesystem::rename(temporary, output, ec);
        if (ec) { std::filesystem::remove(temporary, ec); errorStorage() = "WAVE64 atomic rename failed"; return false; }
#if !defined(_WIN32)
        if (!syncParentDirectory(output)) { errorStorage() = "WAVE64 directory sync failed"; return false; }
#endif
        errorStorage().clear(); return true;
    }

    /// Compatibility PCM16 path for legacy native callers.  It shares the
    /// same atomic publication and durability contract as the float writer.
    static bool writePcm16(const std::string& path, const float* l, const float* r,
                           uint64_t numSamples, uint32_t sampleRate) {
        if (path.empty() || l == nullptr || r == nullptr || numSamples == 0 ||
            sampleRate == 0 || sampleRate > 384000 ||
            numSamples > std::numeric_limits<uint64_t>::max() / 4u) {
            errorStorage() = "invalid PCM16 output arguments";
            return false;
        }
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto temporary = output.string() + ".tmp-aura-pcm16-" +
            std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                0
#else
                ::getpid()
#endif
            )) + "-" + std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
        std::error_code ignored;
        if (!reserveTemporary(temporary)) { errorStorage() = "PCM16 temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open PCM16 output"; return false; }

        const uint64_t dataSize = numSamples * 4u;
        const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 36u;
        const auto u16 = [&file](uint16_t value) {
            const uint8_t b[2] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8)};
            file.write(reinterpret_cast<const char*>(b), 2);
        };
        const auto u32 = [&file](uint32_t value) {
            const uint8_t b[4] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
                static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24)};
            file.write(reinterpret_cast<const char*>(b), 4);
        };
        const auto u64 = [&file](uint64_t value) {
            const uint8_t b[8] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
                static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24),
                static_cast<uint8_t>(value >> 32), static_cast<uint8_t>(value >> 40),
                static_cast<uint8_t>(value >> 48), static_cast<uint8_t>(value >> 56)};
            file.write(reinterpret_cast<const char*>(b), 8);
        };
        file.write(rf64 ? "RF64" : "RIFF", 4);
        u32(rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(36u + dataSize));
        file.write("WAVE", 4);
        if (rf64) {
            file.write("ds64", 4); u32(28);
            u64(72u + dataSize); u64(dataSize); u64(numSamples); u32(0);
        }
        file.write("fmt ", 4); u32(16); u16(1); u16(2);
        u32(sampleRate); u32(sampleRate * 4u); u16(4); u16(16);
        file.write("data", 4); u32(rf64 ? std::numeric_limits<uint32_t>::max()
                                            : static_cast<uint32_t>(dataSize));
        for (uint64_t i = 0; i < numSamples; ++i) {
            const float left = std::isfinite(l[i]) ? std::clamp(l[i], -1.0f, 1.0f) : 0.0f;
            const float right = std::isfinite(r[i]) ? std::clamp(r[i], -1.0f, 1.0f) : 0.0f;
            const auto leftValue = static_cast<int16_t>(std::lrint(left * 32767.0f));
            const auto rightValue = static_cast<int16_t>(std::lrint(right * 32767.0f));
            u16(static_cast<uint16_t>(leftValue)); u16(static_cast<uint16_t>(rightValue));
        }
        file.flush();
        const bool written = static_cast<bool>(file);
        file.close();
        if (!written) { std::filesystem::remove(temporary, ignored); errorStorage() = "PCM16 write failed"; return false; }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::filesystem::remove(temporary, ignored);
            errorStorage() = "PCM16 sync failed";
            return false;
        }
        ::close(fd);
#endif
        std::error_code publishError;
        std::filesystem::rename(temporary, output, publishError);
        if (publishError) {
            std::error_code cleanupError;
            std::filesystem::remove(temporary, cleanupError);
            errorStorage() = "PCM16 rename failed: " + publishError.message();
            return false;
        }
#if !defined(_WIN32)
        if (!syncParentDirectory(output)) { errorStorage() = "PCM16 directory sync failed"; return false; }
#endif
        errorStorage().clear();
        return true;
    }

    /// Canonical little-endian PCM24 stereo output for legacy bounce paths.
    static bool writePcm24(const std::string& path, const float* l, const float* r,
                           uint64_t numSamples, uint32_t sampleRate) {
        if (path.empty() || l == nullptr || r == nullptr || numSamples == 0 ||
            sampleRate == 0 || sampleRate > 384000 ||
            numSamples > std::numeric_limits<uint64_t>::max() / 6u) {
            errorStorage() = "invalid PCM24 output arguments";
            return false;
        }
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto temporary = output.string() + ".tmp-aura-pcm24-" +
            std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                0
#else
                ::getpid()
#endif
            )) + "-" + std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
        std::error_code ignored;
        if (!reserveTemporary(temporary)) { errorStorage() = "PCM24 temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open PCM24 output"; return false; }

        const uint64_t dataSize = numSamples * 6u;
        const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 36u;
        const auto u16 = [&file](uint16_t value) {
            const uint8_t b[2] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8)};
            file.write(reinterpret_cast<const char*>(b), 2);
        };
        const auto u32 = [&file](uint32_t value) {
            const uint8_t b[4] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
                static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24)};
            file.write(reinterpret_cast<const char*>(b), 4);
        };
        const auto u64 = [&file](uint64_t value) {
            const uint8_t b[8] = {static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
                static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24),
                static_cast<uint8_t>(value >> 32), static_cast<uint8_t>(value >> 40),
                static_cast<uint8_t>(value >> 48), static_cast<uint8_t>(value >> 56)};
            file.write(reinterpret_cast<const char*>(b), 8);
        };
        file.write(rf64 ? "RF64" : "RIFF", 4);
        u32(rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(36u + dataSize));
        file.write("WAVE", 4);
        if (rf64) {
            file.write("ds64", 4); u32(28);
            u64(72u + dataSize); u64(dataSize); u64(numSamples); u32(0);
        }
        file.write("fmt ", 4); u32(16); u16(1); u16(2);
        u32(sampleRate); u32(sampleRate * 6u); u16(6); u16(24);
        file.write("data", 4); u32(rf64 ? std::numeric_limits<uint32_t>::max()
                                            : static_cast<uint32_t>(dataSize));
        for (uint64_t i = 0; i < numSamples; ++i) {
            const float left = std::isfinite(l[i]) ? std::clamp(l[i], -1.0f, 1.0f) : 0.0f;
            const float right = std::isfinite(r[i]) ? std::clamp(r[i], -1.0f, 1.0f) : 0.0f;
            const auto writeSample = [&file](float value) {
                const int32_t sample = static_cast<int32_t>(std::lrint(value * 8388607.0f));
                const uint8_t b[3] = {static_cast<uint8_t>(sample), static_cast<uint8_t>(sample >> 8),
                    static_cast<uint8_t>(sample >> 16)};
                file.write(reinterpret_cast<const char*>(b), 3);
            };
            writeSample(left); writeSample(right);
        }
        file.flush();
        const bool written = static_cast<bool>(file);
        file.close();
        if (!written) { std::filesystem::remove(temporary, ignored); errorStorage() = "PCM24 write failed"; return false; }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::filesystem::remove(temporary, ignored);
            errorStorage() = "PCM24 sync failed";
            return false;
        }
        ::close(fd);
#endif
        std::error_code publishError;
        std::filesystem::rename(temporary, output, publishError);
        if (publishError) {
            std::error_code cleanupError;
            std::filesystem::remove(temporary, cleanupError);
            errorStorage() = "PCM24 rename failed: " + publishError.message();
            return false;
        }
#if !defined(_WIN32)
        if (!syncParentDirectory(output)) { errorStorage() = "PCM24 directory sync failed"; return false; }
#endif
        errorStorage().clear();
        return true;
    }

    static bool writePcm16Interleaved(const std::string& path,
                                      const std::vector<std::vector<float>>& channels,
                                      uint32_t sampleRate) {
        if (path.empty() || channels.empty() || channels.size() > std::numeric_limits<uint16_t>::max() ||
            sampleRate == 0 || sampleRate > 384000 || channels.front().empty()) {
            errorStorage() = "invalid interleaved PCM16 output arguments";
            return false;
        }
        const uint64_t frames = channels.front().size();
        const uint64_t count = channels.size();
        for (const auto& channel : channels) if (channel.size() != frames) {
            errorStorage() = "interleaved PCM16 channel lengths differ";
            return false;
        }
        const uint64_t blockAlign = count * 2u;
        if (blockAlign > std::numeric_limits<uint16_t>::max() ||
