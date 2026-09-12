public:
    static bool syncParentDirectory(const std::filesystem::path& output) noexcept {
#if !defined(_WIN32)
        const auto parent = output.parent_path().empty() ? std::filesystem::path(".") : output.parent_path();
        const int fd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY);
        if (fd < 0) return false;
        const bool ok = ::fsync(fd) == 0;
        ::close(fd);
        return ok;
#else
        (void)output;
        return true;
#endif
    }

    // Reserve a temporary pathname without truncating an existing writer's
    // spool.  Export paths are frequently retried after cancellation, so a
    // stale-name cleanup followed by `ofstream(...|trunc)` is unsafe.
    static bool reserveTemporary(const std::filesystem::path& path) noexcept {
#if !defined(_WIN32)
        const int fd = ::open(path.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0600);
        if (fd < 0) return false;
        return ::close(fd) == 0;
#else
        std::error_code ec;
        if (std::filesystem::exists(path, ec) || ec) return false;
        std::ofstream file(path, std::ios::binary);
        return file.good();
#endif
    }

    /// Bounded-memory IEEE-float recorder.  Unlike the offline writers, the
    /// final frame count is not known when the file is opened, so a fixed
    /// JUNK reservation is patched into a RIFF/RF64 header at finish time.
    /// Publication, durability, and temporary-file ownership remain exactly
    /// the same as every other canonical writer.
    class Float32StreamWriter {
    public:
        Float32StreamWriter(const std::string& path, uint32_t sampleRate,
                            uint16_t channels = 2)
            : m_output(path), m_channels(channels), m_sampleRate(sampleRate) {
            if (path.empty() || sampleRate == 0 || sampleRate > 384000 ||
                channels == 0 || channels > 32) {
                m_error = "invalid float32 stream arguments";
                return;
            }
            static std::atomic<uint64_t> sequence{0};
            const auto id = sequence.fetch_add(1, std::memory_order_relaxed);
            m_temporary = m_output.string() + ".tmp-aura-f32-stream-" +
                std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                    0
#else
                    ::getpid()
#endif
                )) + "-" + std::to_string(id);
            if (!WavWriter::reserveTemporary(m_temporary)) {
                m_error = "float32 stream temporary path is busy";
                return;
            }
            m_file.open(m_temporary, std::ios::binary | std::ios::trunc);
            if (!m_file.is_open()) {
                m_error = "unable to open float32 stream output";
                return;
            }
            writePlaceholderHeader();
            m_open = static_cast<bool>(m_file);
            if (!m_open) m_error = "float32 stream header write failed";
        }

        Float32StreamWriter(const Float32StreamWriter&) = delete;
        Float32StreamWriter& operator=(const Float32StreamWriter&) = delete;
        ~Float32StreamWriter() { if (!m_finished) discard(); }

        bool isOpen() const noexcept { return m_open && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        uint64_t framesWritten() const noexcept { return m_frames; }

        bool writeFrames(const float* const* channels, uint32_t count) {
            if (!isOpen() || channels == nullptr || count == 0) {
                m_error = "invalid float32 stream frame write";
                return false;
            }
            for (uint16_t channel = 0; channel < m_channels; ++channel) {
                if (channels[channel] == nullptr) {
                    m_error = "missing float32 stream channel";
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
            if (!m_file || m_frames > std::numeric_limits<uint64_t>::max() - count) {
                m_error = "float32 stream write failed";
                return false;
            }
            m_frames += count;
            return true;
        }

        bool finish() {
            if (m_finished) return m_error.empty();
            if (!isOpen()) { discard(); return false; }
            const uint64_t bytesPerFrame = static_cast<uint64_t>(m_channels) * sizeof(float);
            if (m_frames > std::numeric_limits<uint64_t>::max() / bytesPerFrame) {
                m_error = "float32 stream size overflow";
                discard();
                return false;
            }
            const uint64_t dataSize = m_frames * bytesPerFrame;
            if (!writeFinalHeader(dataSize)) {
                discard();
                return false;
            }
            m_file.flush();
            if (!m_file) { m_error = "float32 stream flush failed"; discard(); return false; }
            m_file.close();
#if !defined(_WIN32)
            const int fd = ::open(m_temporary.c_str(), O_RDONLY);
            if (fd < 0 || ::fsync(fd) != 0) {
                if (fd >= 0) ::close(fd);
                m_error = "float32 stream sync failed";
                discard();
                return false;
            }
            ::close(fd);
#endif
            std::error_code ec;
            std::filesystem::rename(m_temporary, m_output, ec);
            if (ec) {
                m_error = "float32 stream atomic rename failed";
                discard();
                return false;
            }
#if !defined(_WIN32)
            if (!WavWriter::syncParentDirectory(m_output)) {
                m_error = "float32 stream directory sync failed";
                discard();
                return false;
            }
#endif
            m_finished = true;
            return true;
        }

    private:
        static void putU16(std::ostream& stream, uint16_t value) {
            const char bytes[2] = {static_cast<char>(value), static_cast<char>(value >> 8)};
            stream.write(bytes, sizeof(bytes));
        }
        static void putU32(std::ostream& stream, uint32_t value) {
            const char bytes[4] = {static_cast<char>(value), static_cast<char>(value >> 8),
                static_cast<char>(value >> 16), static_cast<char>(value >> 24)};
            stream.write(bytes, sizeof(bytes));
        }
        static void putU64(std::ostream& stream, uint64_t value) {
            const char bytes[8] = {static_cast<char>(value), static_cast<char>(value >> 8),
                static_cast<char>(value >> 16), static_cast<char>(value >> 24),
                static_cast<char>(value >> 32), static_cast<char>(value >> 40),
                static_cast<char>(value >> 48), static_cast<char>(value >> 56)};
            stream.write(bytes, sizeof(bytes));
        }
        void writePlaceholderHeader() {
            m_file.write("RIFF", 4); putU32(m_file, 0); m_file.write("WAVE", 4);
            m_file.write("JUNK", 4); putU32(m_file, 28);
            const std::array<char, 28> zeros{};
            m_file.write(zeros.data(), static_cast<std::streamsize>(zeros.size()));
            writeFormatAndDataHeader(0, false);
        }
        void writeFormatAndDataHeader(uint64_t dataSize, bool rf64) {
            m_file.write("fmt ", 4); putU32(m_file, 16); putU16(m_file, 3);
            putU16(m_file, m_channels); putU32(m_file, m_sampleRate);
            const uint64_t bytesPerFrame = static_cast<uint64_t>(m_channels) * sizeof(float);
            putU32(m_file, static_cast<uint32_t>(m_sampleRate * bytesPerFrame));
            putU16(m_file, static_cast<uint16_t>(bytesPerFrame)); putU16(m_file, 32);
            m_file.write("data", 4);
            putU32(m_file, rf64 ? std::numeric_limits<uint32_t>::max()
                               : static_cast<uint32_t>(dataSize));
        }
        bool writeFinalHeader(uint64_t dataSize) {
            const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 72u;
            const uint64_t riffSize = rf64 ? 72u + dataSize : 72u + dataSize;
            m_file.seekp(0, std::ios::beg);
            if (!m_file) { m_error = "float32 stream header seek failed"; return false; }
            m_file.write(rf64 ? "RF64" : "RIFF", 4);
            putU32(m_file, rf64 ? std::numeric_limits<uint32_t>::max()
                               : static_cast<uint32_t>(riffSize));
            m_file.write("WAVE", 4);
            if (rf64) {
                m_file.write("ds64", 4); putU32(m_file, 28);
                putU64(m_file, riffSize); putU64(m_file, dataSize); putU64(m_file, m_frames); putU32(m_file, 0);
            } else {
                m_file.write("JUNK", 4); putU32(m_file, 28);
                const std::array<char, 28> zeros{};
                m_file.write(zeros.data(), static_cast<std::streamsize>(zeros.size()));
            }
            writeFormatAndDataHeader(dataSize, rf64);
            return static_cast<bool>(m_file);
        }
        void discard() noexcept {
            if (m_file.is_open()) m_file.close();
            std::error_code ec;
            if (!m_temporary.empty()) std::filesystem::remove(m_temporary, ec);
            m_finished = true;
        }
        std::filesystem::path m_output, m_temporary;
        std::ofstream m_file;
        uint16_t m_channels = 0;
        uint32_t m_sampleRate = 0;
        uint64_t m_frames = 0;
        std::string m_error;
        bool m_open = false, m_finished = false;
    };

    /// Streaming 24-bit PCM publisher used by offline renderers.  Header
    /// sizes are known up front, so the writer can keep bounded memory while
    /// sharing the canonical RF64, durability, and atomic-publish contract.
    class Pcm24StreamWriter {
    public:
        Pcm24StreamWriter(const std::string& path, uint64_t frames, uint32_t sampleRate,
                          bool broadcastWave = false, uint16_t channels = 2)
            : m_output(path), m_frames(frames), m_channels(channels) {
            if (path.empty() || frames == 0 || sampleRate == 0 || sampleRate > 384000 ||
                (channels != 1 && channels != 2) ||
                frames > std::numeric_limits<uint64_t>::max() / (3u * channels)) {
                m_error = "invalid PCM24 stream arguments";
                return;
            }
            static std::atomic<uint64_t> sequence{0};
            const auto id = sequence.fetch_add(1, std::memory_order_relaxed);
            m_temporary = m_output.string() + ".tmp-aura-pcm24-stream-" +
                std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                    0
#else
                    ::getpid()
#endif
                )) + "-" + std::to_string(id);
            if (!WavWriter::reserveTemporary(m_temporary)) {
                m_error = "PCM24 stream temporary path is busy";
                return;
            }
            m_file.open(m_temporary, std::ios::binary | std::ios::trunc);
            if (!m_file.is_open()) { m_error = "unable to open PCM24 stream output"; return; }
            const uint64_t bytesPerFrame = 3u * channels;
            const uint64_t dataSize = frames * bytesPerFrame;
            const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 36u;
            const uint64_t riffSize = broadcastWave ? (rf64 ? 682u : 646u) + dataSize
                                                     : (rf64 ? 72u : 36u) + dataSize;
            put(rf64 ? "RF64" : "RIFF", 4); putU32(rf64 ? std::numeric_limits<uint32_t>::max()
                                         : static_cast<uint32_t>(riffSize));
            put("WAVE", 4);
            if (broadcastWave) {
                put("bext", 4); putU32(602); const std::array<uint8_t, 602> bext{};
                m_file.write(reinterpret_cast<const char*>(bext.data()), static_cast<std::streamsize>(bext.size()));
            }
            if (rf64) {
                put("ds64", 4); putU32(28); putU64(riffSize);
                putU64(dataSize); putU64(frames); putU32(0);
            }
            put("fmt ", 4); putU32(16); putU16(1); putU16(channels);
            putU32(sampleRate); putU32(sampleRate * bytesPerFrame);
            putU16(static_cast<uint16_t>(bytesPerFrame)); putU16(24);
            put("data", 4); putU32(rf64 ? std::numeric_limits<uint32_t>::max()
                                         : static_cast<uint32_t>(dataSize));
            m_open = static_cast<bool>(m_file);
            if (!m_open) m_error = "PCM24 stream header write failed";
        }

        Pcm24StreamWriter(const Pcm24StreamWriter&) = delete;
        Pcm24StreamWriter& operator=(const Pcm24StreamWriter&) = delete;
        ~Pcm24StreamWriter() { if (!m_finished) discard(); }

        bool isOpen() const noexcept { return m_open && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }

        bool writeFrames(const float* left, const float* right, uint32_t count) {
            if (!isOpen() || left == nullptr || (m_channels == 2 && right == nullptr) || count == 0 ||
                m_written > m_frames || count > m_frames - m_written) {
                m_error = "invalid PCM24 stream frame write";
                return false;
            }
            for (uint32_t i = 0; i < count; ++i) {
                const float l = std::isfinite(left[i]) ? std::clamp(left[i], -1.0f, 1.0f) : 0.0f;
                const float r = m_channels == 2 && std::isfinite(right[i]) ? std::clamp(right[i], -1.0f, 1.0f) : 0.0f;
                const int32_t li = static_cast<int32_t>(l * 8388607.0f);
                const int32_t ri = static_cast<int32_t>(r * 8388607.0f);
                const char leftBytes[3] = {static_cast<char>(li & 0xff), static_cast<char>((li >> 8) & 0xff), static_cast<char>((li >> 16) & 0xff)};
                m_file.write(leftBytes, sizeof(leftBytes));
                if (m_channels == 2) {
                    const char rightBytes[3] = {static_cast<char>(ri & 0xff), static_cast<char>((ri >> 8) & 0xff), static_cast<char>((ri >> 16) & 0xff)};
                    m_file.write(rightBytes, sizeof(rightBytes));
                }
            }
            m_written += count;
            if (!m_file) { m_error = "PCM24 stream write failed"; return false; }
            return true;
        }

        bool finish() {
            if (m_finished) return m_error.empty();
            if (!isOpen() || m_written != m_frames) { m_error = "PCM24 stream frame count mismatch"; discard(); return false; }
            m_file.flush(); const bool written = static_cast<bool>(m_file); m_file.close();
            if (!written) { m_error = "PCM24 stream flush failed"; discard(); return false; }
#if !defined(_WIN32)
            const int fd = ::open(m_temporary.c_str(), O_RDONLY);
            if (fd < 0 || ::fsync(fd) != 0) { if (fd >= 0) ::close(fd); m_error = "PCM24 stream sync failed"; discard(); return false; }
            ::close(fd);
#endif
            std::error_code ec; std::filesystem::rename(m_temporary, m_output, ec);
            if (ec) { m_error = "PCM24 stream atomic rename failed"; discard(); return false; }
#if !defined(_WIN32)
            if (!WavWriter::syncParentDirectory(m_output)) { m_error = "PCM24 stream directory sync failed"; discard(); return false; }
#endif
            m_finished = true; return true;
        }

    private:
        void put(const char* data, std::streamsize size) { m_file.write(data, size); }
        void putU16(uint16_t value) { const char b[2] = {static_cast<char>(value), static_cast<char>(value >> 8)}; put(b, 2); }
        void putU32(uint32_t value) { const char b[4] = {static_cast<char>(value), static_cast<char>(value >> 8), static_cast<char>(value >> 16), static_cast<char>(value >> 24)}; put(b, 4); }
        void putU64(uint64_t value) { const char b[8] = {static_cast<char>(value), static_cast<char>(value >> 8), static_cast<char>(value >> 16), static_cast<char>(value >> 24), static_cast<char>(value >> 32), static_cast<char>(value >> 40), static_cast<char>(value >> 48), static_cast<char>(value >> 56)}; put(b, 8); }
        void discard() noexcept { if (m_file.is_open()) m_file.close(); std::error_code ec; if (!m_temporary.empty()) std::filesystem::remove(m_temporary, ec); m_finished = true; }
        std::filesystem::path m_output, m_temporary;
        std::ofstream m_file;
        uint64_t m_frames = 0, m_written = 0;
        uint16_t m_channels = 2;
        std::string m_error;
        bool m_open = false, m_finished = false;
    };

    /// Compatibility streaming publisher for legacy stereo PCM16 callbacks.
    /// It intentionally shares the same publication lifecycle as PCM24 while
    /// preserving the old API's on-disk format.
    class Pcm16StreamWriter {
    public:
        Pcm16StreamWriter(const std::string& path, uint64_t frames, uint32_t sampleRate,
                          uint16_t channels = 2)
            : m_output(path), m_frames(frames), m_channels(channels) {
            if (path.empty() || frames == 0 || sampleRate == 0 || sampleRate > 384000 ||
                (channels != 1 && channels != 2) ||
                frames > std::numeric_limits<uint64_t>::max() / (2u * channels)) {
                m_error = "invalid PCM16 stream arguments"; return;
            }
            static std::atomic<uint64_t> sequence{0};
            m_temporary = m_output.string() + ".tmp-aura-pcm16-stream-" +
                std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                    0
#else
                    ::getpid()
#endif
                )) + "-" + std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
            if (!WavWriter::reserveTemporary(m_temporary)) {
                m_error = "PCM16 stream temporary path is busy";
                return;
            }
            m_file.open(m_temporary, std::ios::binary | std::ios::trunc);
            if (!m_file.is_open()) { m_error = "unable to open PCM16 stream output"; return; }
            const uint64_t bytesPerFrame = 2u * channels;
            const uint64_t dataSize = frames * bytesPerFrame;
            const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 36u;
            put(rf64 ? "RF64" : "RIFF", 4); putU32(rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(36u + dataSize)); put("WAVE", 4);
            if (rf64) { put("ds64", 4); putU32(28); putU64(72u + dataSize); putU64(dataSize); putU64(frames); putU32(0); }
            put("fmt ", 4); putU32(16); putU16(1); putU16(channels); putU32(sampleRate);
            putU32(sampleRate * bytesPerFrame); putU16(static_cast<uint16_t>(bytesPerFrame)); putU16(16);
            put("data", 4); putU32(rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(dataSize));
            m_open = static_cast<bool>(m_file); if (!m_open) m_error = "PCM16 stream header write failed";
        }
        Pcm16StreamWriter(const Pcm16StreamWriter&) = delete;
        Pcm16StreamWriter& operator=(const Pcm16StreamWriter&) = delete;
        ~Pcm16StreamWriter() { if (!m_finished) discard(); }
        bool isOpen() const noexcept { return m_open && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        bool writeFrames(const float* left, const float* right, uint32_t count) {
            if (!isOpen() || !left || (m_channels == 2 && !right) || count == 0 || m_written > m_frames || count > m_frames - m_written) { m_error = "invalid PCM16 stream frame write"; return false; }
            for (uint32_t i = 0; i < count; ++i) {
                const auto convert = [](float value) { const float safe = std::isfinite(value) ? std::clamp(value, -1.0f, 1.0f) : 0.0f; return static_cast<int16_t>(std::lrint(safe * 32767.0f)); };
                putU16(static_cast<uint16_t>(convert(left[i])));
                if (m_channels == 2) putU16(static_cast<uint16_t>(convert(right[i])));
            }
            m_written += count; if (!m_file) { m_error = "PCM16 stream write failed"; return false; } return true;
        }
        bool finish() {
            if (m_finished) return m_error.empty();
            if (!isOpen() || m_written != m_frames) { m_error = "PCM16 stream frame count mismatch"; discard(); return false; }
            m_file.flush(); const bool written = static_cast<bool>(m_file); m_file.close();
            if (!written) { m_error = "PCM16 stream flush failed"; discard(); return false; }
#if !defined(_WIN32)
            const int fd = ::open(m_temporary.c_str(), O_RDONLY); if (fd < 0 || ::fsync(fd) != 0) { if (fd >= 0) ::close(fd); m_error = "PCM16 stream sync failed"; discard(); return false; } ::close(fd);
#endif
            std::error_code ec; std::filesystem::rename(m_temporary, m_output, ec); if (ec) { m_error = "PCM16 stream atomic rename failed"; discard(); return false; }
#if !defined(_WIN32)
            if (!WavWriter::syncParentDirectory(m_output)) { m_error = "PCM16 stream directory sync failed"; discard(); return false; }
#endif
            m_finished = true; return true;
        }
    private:
        void put(const char* data, std::streamsize size) { m_file.write(data, size); }
        void putU16(uint16_t value) { const char b[2] = {static_cast<char>(value), static_cast<char>(value >> 8)}; put(b, 2); }
        void putU32(uint32_t value) { const char b[4] = {static_cast<char>(value), static_cast<char>(value >> 8), static_cast<char>(value >> 16), static_cast<char>(value >> 24)}; put(b, 4); }
        void putU64(uint64_t value) { const char b[8] = {static_cast<char>(value), static_cast<char>(value >> 8), static_cast<char>(value >> 16), static_cast<char>(value >> 24), static_cast<char>(value >> 32), static_cast<char>(value >> 40), static_cast<char>(value >> 48), static_cast<char>(value >> 56)}; put(b, 8); }
        void discard() noexcept { if (m_file.is_open()) m_file.close(); std::error_code ec; if (!m_temporary.empty()) std::filesystem::remove(m_temporary, ec); m_finished = true; }
        std::filesystem::path m_output, m_temporary; std::ofstream m_file; uint64_t m_frames = 0, m_written = 0; uint16_t m_channels = 2; std::string m_error; bool m_open = false, m_finished = false;
    };

    /// Bounded-memory WAVE64 IEEE-float publisher for long offline renders.
    /// The writer accepts one block at a time and only publishes after the
    /// complete payload has been flushed, synced, and atomically renamed.
    class Wave64FloatStreamWriter {
    public:
        Wave64FloatStreamWriter(const std::string& path, uint64_t frames,
                                uint32_t sampleRate, uint16_t channels = 2)
            : m_output(path), m_frames(frames), m_channels(channels) {
            const uint64_t bytesPerFrame = static_cast<uint64_t>(channels) * sizeof(float);
            if (path.empty() || frames == 0 || sampleRate == 0 || sampleRate > 384000 ||
                (channels != 1 && channels != 2) ||
                frames > std::numeric_limits<uint64_t>::max() / bytesPerFrame ||
                static_cast<uint64_t>(sampleRate) > std::numeric_limits<uint32_t>::max() /
                    bytesPerFrame) {
                m_error = "invalid WAVE64 stream arguments";
                return;
            }
            m_payload = frames * bytesPerFrame;
            m_paddedPayload = m_payload + ((8u - (m_payload % 8u)) % 8u);
            if (m_paddedPayload > std::numeric_limits<uint64_t>::max() - 104u) {
                m_error = "WAVE64 stream size overflow";
                return;
            }
            static std::atomic<uint64_t> sequence{0};
            m_temporary = m_output.string() + ".tmp-aura-wave64-stream-" +
                std::to_string(static_cast<unsigned long>(
#if defined(_WIN32)
                    0
#else
                    ::getpid()
#endif
                )) + "-" + std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
            if (!WavWriter::reserveTemporary(m_temporary)) {
                m_error = "WAVE64 stream temporary path is busy";
                return;
            }
            m_file.open(m_temporary, std::ios::binary | std::ios::trunc);
            if (!m_file.is_open()) {
                m_error = "unable to open WAVE64 stream output";
                return;
            }
            const uint8_t riff[16] = {0x52,0x49,0x46,0x46,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
            const uint8_t wave[16] = {0x57,0x41,0x56,0x45,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
            const uint8_t fmt[16] = {0x66,0x6d,0x74,0x20,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
            const uint8_t data[16] = {0x64,0x61,0x74,0x61,0x2e,0x91,0xcf,0x11,0xa5,0xd6,0x28,0xdb,0x04,0xc1,0x00,0x00};
            put(riff, sizeof(riff)); putU64(104u + m_paddedPayload); put(wave, sizeof(wave));
            put(fmt, sizeof(fmt)); putU64(40u); putU16(3); putU16(channels);
            putU32(sampleRate); putU32(static_cast<uint32_t>(sampleRate * bytesPerFrame));
            putU16(static_cast<uint16_t>(bytesPerFrame)); putU16(32);
            put(data, sizeof(data)); putU64(24u + m_paddedPayload);
            m_open = static_cast<bool>(m_file);
            if (!m_open) m_error = "WAVE64 stream header write failed";
        }
        Wave64FloatStreamWriter(const Wave64FloatStreamWriter&) = delete;
        Wave64FloatStreamWriter& operator=(const Wave64FloatStreamWriter&) = delete;
        ~Wave64FloatStreamWriter() { if (!m_finished) discard(); }
        bool isOpen() const noexcept { return m_open && m_error.empty(); }
        const std::string& error() const noexcept { return m_error; }
        uint64_t framesWritten() const noexcept { return m_written; }

        bool writeFrames(const float* const* channels, uint32_t count) {
            if (!isOpen() || !channels || count == 0 || m_written > m_frames ||
                count > m_frames - m_written) {
                m_error = "invalid WAVE64 stream frame write";
