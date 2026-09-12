            frames > std::numeric_limits<uint64_t>::max() / blockAlign) {
            errorStorage() = "interleaved PCM16 output is too large";
            return false;
        }
        const uint64_t dataSize = frames * blockAlign;
        const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 36u;
        const uint64_t riffSize = (rf64 ? 72u : 36u) + dataSize;
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto temporary = output.string() + ".tmp-aura-pcm16-interleaved-" +
            std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
        std::error_code ec;
        std::filesystem::create_directories(
            output.parent_path().empty() ? std::filesystem::path(".") : output.parent_path(), ec);
        if (ec) { errorStorage() = "unable to create PCM16 output directory"; return false; }
        if (!reserveTemporary(temporary)) { errorStorage() = "interleaved PCM16 temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open interleaved PCM16 output"; return false; }
        const auto u16 = [&file](uint16_t value) { const char b[2] = {
            static_cast<char>(value), static_cast<char>(value >> 8)}; file.write(b, 2); };
        const auto u32 = [&file](uint32_t value) { const char b[4] = {
            static_cast<char>(value), static_cast<char>(value >> 8),
            static_cast<char>(value >> 16), static_cast<char>(value >> 24)}; file.write(b, 4); };
        const auto u64 = [&file](uint64_t value) { const char b[8] = {
            static_cast<char>(value), static_cast<char>(value >> 8), static_cast<char>(value >> 16),
            static_cast<char>(value >> 24), static_cast<char>(value >> 32),
            static_cast<char>(value >> 40), static_cast<char>(value >> 48),
            static_cast<char>(value >> 56)}; file.write(b, 8); };
        file.write(rf64 ? "RF64" : "RIFF", 4);
        u32(rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(riffSize));
        file.write("WAVE", 4);
        if (rf64) { file.write("ds64", 4); u32(28); u64(riffSize); u64(dataSize); u64(frames); u32(0); }
        file.write("fmt ", 4); u32(16); u16(1); u16(static_cast<uint16_t>(count));
        u32(sampleRate); u32(static_cast<uint32_t>(sampleRate * blockAlign));
        u16(static_cast<uint16_t>(blockAlign)); u16(16);
        file.write("data", 4); u32(rf64 ? std::numeric_limits<uint32_t>::max() : static_cast<uint32_t>(dataSize));
        for (uint64_t frame = 0; frame < frames; ++frame) {
            for (const auto& channel : channels) {
                const float safe = std::isfinite(channel[frame]) ? std::clamp(channel[frame], -1.0f, 1.0f) : 0.0f;
                u16(static_cast<uint16_t>(static_cast<int16_t>(std::lrint(safe * 32767.0f))));
            }
        }
        file.flush();
        const bool written = static_cast<bool>(file);
        file.close();
        if (!written) { std::filesystem::remove(temporary, ec); errorStorage() = "PCM16 write failed"; return false; }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) { if (fd >= 0) ::close(fd); std::filesystem::remove(temporary, ec); errorStorage() = "PCM16 sync failed"; return false; }
        ::close(fd);
#endif
        std::filesystem::rename(temporary, output, ec);
        if (ec) { std::filesystem::remove(temporary, ec); errorStorage() = "PCM16 rename failed"; return false; }
        if (!syncParentDirectory(output)) { errorStorage() = "PCM16 directory sync failed"; return false; }
        errorStorage().clear();
        return true;
    }

    static bool writePcm24Interleaved(const std::string& path,
                                      const std::vector<std::vector<float>>& channels,
                                      uint32_t sampleRate) {
        if (path.empty() || channels.empty() || channels.size() > std::numeric_limits<uint16_t>::max() ||
            sampleRate == 0 || sampleRate > 384000 || channels.front().empty()) {
            errorStorage() = "invalid interleaved PCM24 output arguments";
            return false;
        }
        const uint64_t count = channels.front().size();
        for (const auto& channel : channels) if (channel.size() != count) {
            errorStorage() = "interleaved PCM24 channel lengths differ";
            return false;
        }
        const uint64_t channelsCount = channels.size();
        const uint64_t blockAlign = channelsCount * 3u;
        const uint64_t dataSize = count > std::numeric_limits<uint64_t>::max() / blockAlign
            ? 0 : count * blockAlign;
        if (dataSize == 0 || blockAlign > std::numeric_limits<uint16_t>::max() ||
            static_cast<uint64_t>(sampleRate) * blockAlign > std::numeric_limits<uint32_t>::max()) {
            errorStorage() = "interleaved PCM24 output is too large";
            return false;
        }
        const bool rf64 = dataSize > std::numeric_limits<uint32_t>::max() - 36u;
        const uint64_t padding = dataSize & 1u;
        const uint64_t riffSize = (rf64 ? 72u : 36u) + dataSize + padding;
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> sequence{0};
        const auto temporary = output.string() + ".tmp-aura-pcm24-interleaved-" +
            std::to_string(sequence.fetch_add(1, std::memory_order_relaxed));
        std::error_code ec;
        std::filesystem::create_directories(output.parent_path().empty() ? std::filesystem::path(".") : output.parent_path(), ec);
        if (ec) { errorStorage() = "unable to create PCM24 output directory"; return false; }
        if (!reserveTemporary(temporary)) { errorStorage() = "interleaved PCM24 temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open interleaved PCM24 output"; return false; }
        const auto u16 = [&file](uint16_t v) { const uint8_t b[2] = {uint8_t(v), uint8_t(v >> 8)}; file.write(reinterpret_cast<const char*>(b), 2); };
        const auto u32 = [&file](uint32_t v) { const uint8_t b[4] = {uint8_t(v), uint8_t(v >> 8), uint8_t(v >> 16), uint8_t(v >> 24)}; file.write(reinterpret_cast<const char*>(b), 4); };
        const auto u64 = [&file](uint64_t v) { const uint8_t b[8] = {uint8_t(v), uint8_t(v >> 8), uint8_t(v >> 16), uint8_t(v >> 24), uint8_t(v >> 32), uint8_t(v >> 40), uint8_t(v >> 48), uint8_t(v >> 56)}; file.write(reinterpret_cast<const char*>(b), 8); };
        file.write(rf64 ? "RF64" : "RIFF", 4); u32(rf64 ? UINT32_MAX : uint32_t(riffSize)); file.write("WAVE", 4);
        if (rf64) { file.write("ds64", 4); u32(28); u64(riffSize); u64(dataSize); u64(count); u32(0); }
        file.write("fmt ", 4); u32(16); u16(1); u16(uint16_t(channelsCount));
        u32(sampleRate); u32(uint32_t(sampleRate * blockAlign)); u16(uint16_t(blockAlign)); u16(24);
        file.write("data", 4); u32(rf64 ? UINT32_MAX : uint32_t(dataSize));
        for (uint64_t sample = 0; sample < count; ++sample) for (const auto& channel : channels) {
            const float value = std::isfinite(channel[sample]) ? std::clamp(channel[sample], -1.0f, 1.0f) : 0.0f;
            const int32_t pcm = static_cast<int32_t>(std::lrint(value * 8388607.0f));
            const uint8_t b[3] = {uint8_t(pcm), uint8_t(pcm >> 8), uint8_t(pcm >> 16)};
            file.write(reinterpret_cast<const char*>(b), 3);
        }
        if (padding) { const char zero = 0; file.write(&zero, 1); }
        file.flush(); const bool written = static_cast<bool>(file); file.close();
        if (!written) { std::filesystem::remove(temporary, ec); errorStorage() = "interleaved PCM24 write failed"; return false; }
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) { if (fd >= 0) ::close(fd); std::filesystem::remove(temporary, ec); errorStorage() = "interleaved PCM24 sync failed"; return false; }
        ::close(fd);
#endif
        std::filesystem::rename(temporary, output, ec);
        if (ec) { std::filesystem::remove(temporary, ec); errorStorage() = "interleaved PCM24 rename failed"; return false; }
#if !defined(_WIN32)
        if (!syncParentDirectory(output)) { errorStorage() = "interleaved PCM24 directory sync failed"; return false; }
#endif
        errorStorage().clear(); return true;
    }

    static bool write(const std::string& path, const float* l, const float* r, uint64_t numSamples, uint32_t sampleRate) {
        if (path.empty() || l == nullptr || r == nullptr || numSamples == 0 ||
            sampleRate == 0 || sampleRate > 384000 ||
            numSamples > std::numeric_limits<uint64_t>::max() / 8u) {
            errorStorage() = "invalid WAV output arguments";
            return false;
        }
        const std::filesystem::path output(path);
        static std::atomic<uint64_t> writeSequence{0};
        const auto sequence = writeSequence.fetch_add(1, std::memory_order_relaxed);
#if defined(_WIN32)
        const std::filesystem::path temporary = output.string() + ".tmp-aura-f32-" + std::to_string(sequence);
#else
        const std::filesystem::path temporary = output.string() + ".tmp-aura-f32-" +
            std::to_string(static_cast<unsigned long>(::getpid())) + "-" + std::to_string(sequence);
#endif
        if (!reserveTemporary(temporary)) { errorStorage() = "WAV temporary path is busy"; return false; }
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file.is_open()) { errorStorage() = "unable to open WAV output"; return false; }

        uint32_t numChannels = 2;
        uint32_t bitsPerSample = 32;
        const uint32_t byteRate = sampleRate * numChannels * bitsPerSample / 8;
        const uint32_t blockAlign = numChannels * bitsPerSample / 8;
        const uint64_t dataSize64 = numSamples * blockAlign;
        const bool rf64 = dataSize64 > (std::numeric_limits<uint32_t>::max() - 36u);
        const uint32_t dataSize32 = rf64 ? std::numeric_limits<uint32_t>::max() :
            static_cast<uint32_t>(dataSize64);
        const uint32_t chunkSize = rf64 ? std::numeric_limits<uint32_t>::max() :
            static_cast<uint32_t>(36u + dataSize64);

        const auto writeU16 = [&file](uint16_t value) {
            const uint8_t bytes[2] = { static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8) };
            file.write(reinterpret_cast<const char*>(bytes), sizeof(bytes));
        };
        const auto writeU32 = [&file](uint32_t value) {
            const uint8_t bytes[4] = { static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
                static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24) };
            file.write(reinterpret_cast<const char*>(bytes), sizeof(bytes));
        };
        const auto writeU64 = [&file](uint64_t value) {
            const uint8_t bytes[8] = { static_cast<uint8_t>(value), static_cast<uint8_t>(value >> 8),
                static_cast<uint8_t>(value >> 16), static_cast<uint8_t>(value >> 24),
                static_cast<uint8_t>(value >> 32), static_cast<uint8_t>(value >> 40),
                static_cast<uint8_t>(value >> 48), static_cast<uint8_t>(value >> 56) };
            file.write(reinterpret_cast<const char*>(bytes), sizeof(bytes));
        };

        // RIFF Header
        file.write(rf64 ? "RF64" : "RIFF", 4);
        writeU32(chunkSize);
        file.write("WAVE", 4);

        if (rf64) {
            file.write("ds64", 4);
            writeU32(28);
            // RF64 ds64.riffSize excludes the 8-byte RIFF header.  The
            // fixed bytes before the data payload are 72 bytes.
            writeU64(72u + dataSize64);
            writeU64(dataSize64);
            writeU64(numSamples);
            writeU32(0); // no additional ds64 table entries
        }

        // FMT Chunk
        file.write("fmt ", 4);
        uint32_t subChunk1Size = 16;
        uint16_t audioFormat = 3; // Float
        writeU32(subChunk1Size);
        writeU16(audioFormat);
        writeU16(static_cast<uint16_t>(numChannels));
        writeU32(sampleRate);
        writeU32(byteRate);
        writeU16(static_cast<uint16_t>(blockAlign));
        writeU16(static_cast<uint16_t>(bitsPerSample));

        // DATA Chunk
        file.write("data", 4);
        writeU32(dataSize32);

        // Interleave L and R channels
        for (uint64_t i = 0; i < numSamples; ++i) {
            const float left = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float right = std::isfinite(r[i]) ? r[i] : 0.0f;
            file.write(reinterpret_cast<const char*>(&left), 4);
            file.write(reinterpret_cast<const char*>(&right), 4);
        }
        file.flush();
        const bool written = static_cast<bool>(file);
        file.close();
        if (!written) { std::error_code ec; std::filesystem::remove(temporary, ec); errorStorage() = "WAV write failed"; return false; }
#if !defined(_WIN32)
        const int fileFd = ::open(temporary.c_str(), O_RDONLY);
        if (fileFd < 0 || ::fsync(fileFd) != 0) {
            if (fileFd >= 0) ::close(fileFd);
            std::error_code ec;
            std::filesystem::remove(temporary, ec);
            errorStorage() = "WAV sync failed";
            return false;
        }
        ::close(fileFd);
#endif
        std::error_code ec;
        std::filesystem::rename(temporary, output, ec);
        if (ec) std::filesystem::remove(temporary, ec);
        if (ec) errorStorage() = "WAV atomic rename failed";
        else {
#if !defined(_WIN32)
            if (!syncParentDirectory(output)) { errorStorage() = "float WAVE directory sync failed"; return false; }
#endif
            errorStorage().clear();
        }
        return !ec;
    }

private:
    static std::string& errorStorage() noexcept {
        thread_local std::string error;
        return error;
    }
};
