    {
        using Transport = Hirari::Core::Plugins::MidiFragmentReassembler;
        Transport transport;
        Transport::Fragment first{};
        first.messageId = 77; first.index = 0; first.total = 2; first.sampleOffset = 12;
        first.size = 2; first.payload[0] = 0xf0; first.payload[1] = 0x01;
        Transport::Fragment second{};
        second.messageId = 77; second.index = 1; second.total = 2;
        second.size = 2; second.payload[0] = 0x02; second.payload[1] = 0xf7;
        if (transport.push(first) != Transport::Result::Accepted ||
            transport.push(second) != Transport::Result::Complete ||
            !transport.complete() || transport.size() != 4 ||
            transport.data()[0] != 0xf0 || transport.data()[3] != 0xf7) return 54;
        Hirari::Core::MidiBuffer mailbox;
        Transport mailboxTransport;
        if (mailbox.addFragment(mailboxTransport, first) != Transport::Result::Accepted ||
            mailbox.addFragment(mailboxTransport, second) != Transport::Result::Complete ||
            mailbox.size() != 1 || mailbox.getEvents()[0].size != 4 ||
            mailbox.getEvents()[0].data[3] != 0xf7) return 56;
        Transport::Fragment largeFirst{};
        largeFirst.messageId = 91; largeFirst.index = 0; largeFirst.total = 2;
        largeFirst.size = Transport::kFragmentPayloadBytes;
        Transport::Fragment largeSecond{};
        largeSecond.messageId = 91; largeSecond.index = 1; largeSecond.total = 2;
        largeSecond.size = Transport::kFragmentPayloadBytes;
        std::fill(largeFirst.payload.begin(), largeFirst.payload.end(), 0x55);
        std::fill(largeSecond.payload.begin(), largeSecond.payload.end(), 0xaa);
        Transport largeTransport;
        auto extended = std::make_unique<Hirari::Core::Plugins::MidiExtendedMessageRing>();
        if (mailbox.addFragment(largeTransport, largeFirst, extended.get()) != Transport::Result::Accepted ||
            mailbox.addFragment(largeTransport, largeSecond, extended.get()) != Transport::Result::Complete ||
            mailbox.size() != 1 || extended->size() != 1) return 57;
        Hirari::Core::Plugins::MidiExtendedMessageRing::Message largeMessage;
        if (!extended->pop(largeMessage) || largeMessage.size != 2 * Transport::kFragmentPayloadBytes ||
            largeMessage.data[0] != 0x55 || largeMessage.data[largeMessage.size - 1] != 0xaa) return 58;
        Transport::Fragment wrongOrder = first;
        wrongOrder.messageId = 88;
        if (transport.push(wrongOrder) != Transport::Result::Accepted) return 55;
    }
    const auto wave64Path = std::filesystem::temp_directory_path() / "hirari-native-wave64-contract.w64";
    const float waveLeft[] = {0.0f, 0.25f};
    const float waveRight[] = {-0.25f, 0.5f};
    {
        const std::string wave64String = wave64Path.string();
        const float* wave64Channels[2] = {waveLeft, waveRight};
        if (hirari_wav_write_planar(
                reinterpret_cast<const uint8_t*>(wave64String.data()), wave64String.size(),
                wave64Channels, 2, 2, 48'000, 3) != 0) return 43;
    }
    {
        std::ifstream wave64(wave64Path, std::ios::binary);
        std::array<uint8_t, 24> header{};
        wave64.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!wave64 || header[0] != 0x52 || header[1] != 0x49 || header[2] != 0x46 || header[3] != 0x46) return 44;
    }
    Hirari::IO::WavLoader::WavInfo waveInfo{};
    const auto waveChannels = Hirari::IO::WavLoader::loadWave64(wave64Path.string(), waveInfo);
    if (waveInfo.sampleRate != 48'000 || waveInfo.numChannels != 2 || waveInfo.bitDepth != 32 ||
        waveChannels.size() != 2 || waveChannels[0].size() != 2 ||
        std::abs(waveChannels[1][1] - 0.5f) > 0.0001f) return 45;
    const auto wave64MultiPath = std::filesystem::temp_directory_path() / "hirari-native-wave64-multichannel-contract.w64";
    const std::vector<std::vector<float>> wave64Multi{{0.0f, 0.25f}, {-0.25f, 0.5f}, {0.75f, -0.5f}};
    if (!Hirari::IO::WavSaver::saveWave64(wave64MultiPath.string(), wave64Multi, 48'000)) return 52;
    Hirari::IO::WavLoader::WavInfo wave64MultiInfo{};
    const auto wave64MultiDecoded = Hirari::IO::WavLoader::loadWave64(
        wave64MultiPath.string(), wave64MultiInfo);
    if (wave64MultiInfo.numChannels != 3 || wave64MultiDecoded.size() != 3 ||
        wave64MultiDecoded[2].size() != 2 ||
        std::abs(wave64MultiDecoded[2][0] - 0.75f) > 0.0001f) return 53;
    const auto oversizedWave64Path = std::filesystem::temp_directory_path() /
                                     "hirari-native-wave64-oversized-contract.w64";
    {
        std::ofstream oversized(oversizedWave64Path, std::ios::binary | std::ios::trunc);
        oversized.seekp(static_cast<std::streamoff>(Hirari::IO::WavLoader::kMaximumDecodedBytes + 2ull * 1024ull * 1024ull - 1ull));
        oversized.put('\0');
    }
    bool oversizedRejected = false;
    try {
        Hirari::IO::WavLoader::WavInfo oversizedInfo{};
        (void)Hirari::IO::WavLoader::loadWave64(oversizedWave64Path.string(), oversizedInfo);
    } catch (const std::runtime_error&) {
        oversizedRejected = true;
    }
    std::filesystem::remove(oversizedWave64Path, offlineCleanup);
    if (!oversizedRejected) return 61;
    // Durable-publication coverage for the Rust-owned WAV writer.
    //
    // WavWriter::reserveTemporary used to open the destination with
    // O_WRONLY|O_CREAT|O_EXCL and was removed when export moved to Rust. The
    // exclusive-create primitive now applies to the writer's internal spool
    // (OpenOptions::create_new(true) in export.rs) while the destination is
    // published afterwards, so the old "a second reserve of the same path
    // fails" behaviour is no longer observable through the public API.
    //
    // What remains observable, and is what that reservation protected, is that
    // an export publishes a complete WAV. That is asserted here directly: the
    // frame count must round-trip exactly, and a re-export over an existing
    // destination must replace it wholesale rather than leave a truncated
    // remnant.
    const auto reservedTempPath = std::filesystem::temp_directory_path() /
                                  "hirari-native-wav-reservation-contract.wav";
    std::filesystem::remove(reservedTempPath, offlineCleanup);
    const std::vector<float> reservationLeft{0.0f, 0.5f, -0.25f, 0.125f};
    const std::vector<float> reservationRight{0.0f, -0.5f, 0.25f, -0.125f};
    {
        const std::string pathString = reservedTempPath.string();
        const float* planes[2] = {reservationLeft.data(), reservationRight.data()};
        if (hirari_wav_write_planar(
                reinterpret_cast<const uint8_t*>(pathString.data()), pathString.size(),
                planes, 2, reservationLeft.size(), 48'000, 0) != 0) return 62;
    }
    {
        Hirari::IO::WavLoader::WavInfo reservationInfo{};
        const auto decoded =
            Hirari::IO::WavLoader::load(reservedTempPath.string(), reservationInfo);
        if (decoded.size() != 2 || reservationInfo.sampleRate != 48'000 ||
            reservationInfo.numChannels != 2) return 62;
        if (decoded[0].size() != reservationLeft.size() ||
            decoded[1].size() != reservationRight.size()) return 62;
    }
    // Re-export different, longer content over the existing destination.
    const std::vector<float> replacedLeft{0.25f, -0.25f, 0.75f, -0.5f, 0.125f, -0.125f};
    const std::vector<float> replacedRight{-0.25f, 0.25f, 0.5f, -0.75f, 0.25f, -0.25f};
    {
        const std::string pathString = reservedTempPath.string();
        const float* planes[2] = {replacedLeft.data(), replacedRight.data()};
        if (hirari_wav_write_planar(
                reinterpret_cast<const uint8_t*>(pathString.data()), pathString.size(),
                planes, 2, replacedLeft.size(), 48'000, 0) != 0) return 62;
    }
    {
        Hirari::IO::WavLoader::WavInfo reservationInfo{};
        const auto decoded =
            Hirari::IO::WavLoader::load(reservedTempPath.string(), reservationInfo);
        if (decoded.size() != 2 || reservationInfo.sampleRate != 48'000 ||
            reservationInfo.numChannels != 2) return 62;
        if (decoded[0].size() != replacedLeft.size() ||
            decoded[1].size() != replacedRight.size()) return 62;
        if (decoded[0] != replacedLeft || decoded[1] != replacedRight) return 62;
    }
    std::filesystem::remove(reservedTempPath, offlineCleanup);
    const auto pcm16Path = std::filesystem::temp_directory_path() / "hirari-native-pcm16-contract.wav";
    {
        const std::string pcm16String = pcm16Path.string();
        const float* pcm16Channels[2] = {waveLeft, waveRight};
        if (hirari_wav_write_planar(
                reinterpret_cast<const uint8_t*>(pcm16String.data()), pcm16String.size(),
                pcm16Channels, 2, 2, 48'000, 1) != 0) return 48;
    }
    const auto pcm16MultiPath = std::filesystem::temp_directory_path() / "hirari-native-pcm16-multichannel-contract.wav";
    const std::vector<std::vector<float>> pcm16Multi{{0.0f, 0.25f}, {-0.25f, 0.5f}, {0.75f, -0.5f}};
    {
        const std::string pathString = pcm16MultiPath.string();
        std::vector<const float*> planes;
        for (const auto& channel : pcm16Multi) planes.push_back(channel.data());
        if (hirari_wav_write_planar(
                reinterpret_cast<const uint8_t*>(pathString.data()), pathString.size(),
                planes.data(), planes.size(), pcm16Multi.front().size(), 48'000, 1) != 0) return 60;
    }
    Hirari::IO::WavLoader::WavInfo pcm16Info{};
    const auto pcm16Decoded = Hirari::IO::WavLoader::load(pcm16MultiPath.string(), pcm16Info);
    if (pcm16Info.numChannels != 3 || pcm16Info.bitDepth != 16 || pcm16Decoded.size() != 3 ||
        pcm16Decoded[2].size() != 2 || std::abs(pcm16Decoded[2][0] - 0.75f) > 0.001f ||
        std::abs(pcm16Decoded[1][1] - 0.5f) > 0.001f) return 61;
    std::filesystem::remove(pcm16MultiPath, offlineCleanup);
    {
        std::ifstream pcm16(pcm16Path, std::ios::binary);
        std::array<uint8_t, 36> header{};
        pcm16.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!pcm16 || std::string(reinterpret_cast<const char*>(header.data()), 4) != "RIFF" ||
            header[34] != 16 || header[35] != 0) return 49;
    }
    const auto pcm24Path = std::filesystem::temp_directory_path() / "hirari-native-pcm24-contract.wav";
    {
        const std::string pcm24String = pcm24Path.string();
        const float* pcm24Channels[2] = {waveLeft, waveRight};
        if (hirari_wav_write_planar(
                reinterpret_cast<const uint8_t*>(pcm24String.data()), pcm24String.size(),
                pcm24Channels, 2, 2, 48'000, 2) != 0) return 50;
    }
    {
        std::ifstream pcm24(pcm24Path, std::ios::binary);
        std::array<uint8_t, 36> header{};
        pcm24.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!pcm24 || std::string(reinterpret_cast<const char*>(header.data()), 4) != "RIFF" ||
            header[34] != 24 || header[35] != 0) return 51;
    }
    // The native stream façade was folded into the Rust C ABI, so the
    // contract exercises the owning writers directly.
    struct ContractStream final {
        void* handle = nullptr;
        uint8_t kind = 0;  // 0 PCM, 1 WAVE64, 2 float32
        ContractStream(void* value, uint8_t streamKind) : handle(value), kind(streamKind) {}
        ContractStream(const ContractStream&) = delete;
        ContractStream& operator=(const ContractStream&) = delete;
        ~ContractStream() {
            if (kind == 0) hirari_wav_pcm_stream_free(handle);
            else if (kind == 1) hirari_wav_wave64_stream_free(handle);
            else hirari_wav_float32_stream_free(handle);
        }
        bool isOpen() const { return handle != nullptr; }
        bool write(const float* const* channels, uint32_t count) const {
            if (!handle) return false;
            if (kind == 0) return hirari_wav_pcm_stream_write(handle, channels, count);
            if (kind == 1) return hirari_wav_wave64_stream_write(handle, channels, count);
            return hirari_wav_float32_stream_write(handle, channels, count);
        }
        uint64_t framesWritten() const {
            if (kind == 1) return hirari_wav_wave64_stream_frames(handle);
            if (kind == 2) return hirari_wav_float32_stream_frames(handle);
            return 0;
        }
        bool finish() const {
            if (!handle) return false;
            if (kind == 0) return hirari_wav_pcm_stream_finish(handle);
            if (kind == 1) return hirari_wav_wave64_stream_finish(handle);
            return hirari_wav_float32_stream_finish(handle);
        }
    };
    const auto streamPath = std::filesystem::temp_directory_path() / "hirari-native-pcm24-stream-contract.wav";
    const std::string streamPathString = streamPath.string();
    {
        const float* channels[2] = {waveLeft, waveRight};
        ContractStream stream(hirari_wav_pcm_stream_create(
            streamPathString.data(), streamPathString.size(), 4, 48'000, 2, 24, 0), 0);
        if (!stream.isOpen() || !stream.write(channels, 2) ||
            !stream.write(channels, 2) || !stream.finish()) return 59;
        std::ifstream streamInput(streamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || std::string(streamHeader.begin(), streamHeader.begin() + 4) != "RIFF" ||
            std::string(streamHeader.begin() + 8, streamHeader.begin() + 12) != "WAVE" ||
            streamHeader[34] != 24 || streamHeader[35] != 0 ||
            std::filesystem::file_size(streamPath) != 68) return 60;
    }
    const auto pcm16StreamPath = std::filesystem::temp_directory_path() /
                                 "hirari-native-pcm16-stream-contract.wav";
    {
        const std::string pathString = pcm16StreamPath.string();
        ContractStream stream(hirari_wav_pcm_stream_create(
            pathString.data(), pathString.size(), 4, 48'000, 2, 16, 0), 0);
        const float* second[2] = {waveLeft + 1, waveRight + 1};
        const float* first[2] = {waveLeft, waveRight};
        if (!stream.isOpen() || !stream.write(first, 1) ||
            !stream.write(second, 3) || !stream.finish()) return 63;
        std::ifstream streamInput(pcm16StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || std::string(streamHeader.begin(), streamHeader.begin() + 4) != "RIFF" ||
            std::string(streamHeader.begin() + 8, streamHeader.begin() + 12) != "WAVE" ||
            streamHeader[34] != 16 || streamHeader[35] != 0 ||
            std::filesystem::file_size(pcm16StreamPath) != 60) return 64;
    }
    const auto wave64StreamPath = std::filesystem::temp_directory_path() /
                                  "hirari-native-wave64-stream-contract.w64";
    {
        const std::string pathString = wave64StreamPath.string();
        ContractStream stream(hirari_wav_wave64_stream_create(
            pathString.data(), pathString.size(), 4, 48'000, 2), 1);
        const float streamLeft[4] = {0.0f, 0.25f, 0.5f, 0.75f};
        const float streamRight[4] = {-0.25f, 0.5f, -0.75f, 1.0f};
        const float* channels[2] = {streamLeft, streamRight};
        if (!stream.isOpen() || !stream.write(channels, 2) ||
            !stream.write(channels, 2) || stream.framesWritten() != 4 ||
            !stream.finish()) return 76;
        Hirari::IO::WavLoader::WavInfo info{};
        const auto decoded = Hirari::IO::WavLoader::loadWave64(wave64StreamPath.string(), info);
        if (info.sampleRate != 48'000 || info.numChannels != 2 || info.bitDepth != 32 ||
            decoded.size() != 2 || decoded[0].size() != 4 ||
            std::abs(decoded[1][1] - 0.5f) > 0.0001f ||
            std::filesystem::file_size(wave64StreamPath) != 104 + 4 * 2 * sizeof(float)) return 77;
    }
    const auto monoPcm16StreamPath = std::filesystem::temp_directory_path() /
                                     "hirari-native-pcm16-mono-stream-contract.wav";
    {
        const std::string pathString = monoPcm16StreamPath.string();
        ContractStream stream(hirari_wav_pcm_stream_create(
            pathString.data(), pathString.size(), 4, 48'000, 1, 16, 0), 0);
        const float* monoChannels[1] = {waveLeft};
        if (!stream.isOpen() || !stream.write(monoChannels, 4) || !stream.finish()) return 65;
        std::ifstream streamInput(monoPcm16StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || streamHeader[22] != 1 || streamHeader[34] != 16 ||
            std::filesystem::file_size(monoPcm16StreamPath) != 52) return 66;
    }
    const auto monoPcm24StreamPath = std::filesystem::temp_directory_path() /
                                     "hirari-native-pcm24-mono-stream-contract.wav";
    {
        const std::string pathString = monoPcm24StreamPath.string();
        ContractStream stream(hirari_wav_pcm_stream_create(
            pathString.data(), pathString.size(), 4, 48'000, 1, 24, 0), 0);
        const float* monoChannels[1] = {waveLeft};
        if (!stream.isOpen() || !stream.write(monoChannels, 4) || !stream.finish()) return 67;
        std::ifstream streamInput(monoPcm24StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || streamHeader[22] != 1 || streamHeader[34] != 24 ||
            std::filesystem::file_size(monoPcm24StreamPath) != 56) return 68;
    }
    const auto floatStreamPath = std::filesystem::temp_directory_path() /
                                 "hirari-native-float32-stream-contract.wav";
    {
        const float floatLeft[2] = {0.25f, std::numeric_limits<float>::quiet_NaN()};
        const float floatRight[2] = {-0.5f, 0.75f};
        const float* floatChannels[2] = {floatLeft, floatRight};
        const std::string pathString = floatStreamPath.string();
        ContractStream stream(hirari_wav_float32_stream_create(
            pathString.data(), pathString.size(), 48'000, 2), 2);
        if (!stream.isOpen() || !stream.write(floatChannels, 2) ||
            stream.framesWritten() != 2 || !stream.finish()) return 69;
        std::ifstream streamInput(floatStreamPath, std::ios::binary);
        std::array<uint8_t, 80> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || std::string(streamHeader.begin(), streamHeader.begin() + 4) != "RIFF" ||
            std::string(streamHeader.begin() + 8, streamHeader.begin() + 12) != "WAVE" ||
            streamHeader[56] != 3 || streamHeader[57] != 0 ||
            streamHeader[58] != 2 || streamHeader[59] != 0 ||
            std::filesystem::file_size(floatStreamPath) != 80 + 2 * 2 * sizeof(float)) return 70;
        float decoded[4] = {};
        streamInput.read(reinterpret_cast<char*>(decoded), sizeof(decoded));
        if (!streamInput || decoded[0] != 0.25f || decoded[1] != -0.5f ||
            decoded[2] != 0.0f || decoded[3] != 0.75f) return 71;
    }
    const auto malformedWave64Path = std::filesystem::temp_directory_path() / "hirari-native-wave64-malformed.w64";
    {
        std::ifstream input(wave64Path, std::ios::binary);
        std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
        if (bytes.size() < 64) return 46;
        for (size_t i = 56; i < 64; ++i) bytes[i] = 0; // fmt chunk size must be >= 40
        std::ofstream output(malformedWave64Path, std::ios::binary | std::ios::trunc);
        output.write(reinterpret_cast<const char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
    }
    bool malformedRejected = false;
    try {
        Hirari::IO::WavLoader::WavInfo malformedInfo{};
        (void)Hirari::IO::WavLoader::loadWave64(malformedWave64Path.string(), malformedInfo);
    } catch (const std::exception&) {
        malformedRejected = true;
    }
    if (!malformedRejected) return 47;
    const auto validWaveDiagnostic = Hirari::IO::WavLoader::loadDiagnosticJson(
        wave64Path.string(), true);
    if (validWaveDiagnostic.find("\"ok\":true") == std::string::npos ||
        validWaveDiagnostic.find("\"format\":\"WAVE64\"") == std::string::npos ||
        validWaveDiagnostic.find("\"channels\":2") == std::string::npos) return 56;
    const auto malformedWaveDiagnostic = Hirari::IO::WavLoader::loadDiagnosticJson(
        malformedWave64Path.string(), true);
    if (malformedWaveDiagnostic.find("\"ok\":false") == std::string::npos ||
        malformedWaveDiagnostic.find("\"error\":\"") == std::string::npos) return 57;
    // A missing WAVE64 file must be reported as a failure, and the diagnostic must
    // identify which file was missing. The exact wording of the error text is an
    // implementation detail (it currently embeds the OS error and the resolved
    // path), so assert the behaviour rather than a literal string.
    const auto missingWaveDiagnostic = Hirari::IO::WavLoader::loadDiagnosticJson(
        (wave64Path.parent_path() / "hirari-wav-does-not-exist.w64").string(), true);
    if (missingWaveDiagnostic.find("\"ok\":false") == std::string::npos ||
        missingWaveDiagnostic.find("\"error\":\"") == std::string::npos ||
        missingWaveDiagnostic.find("No such file or directory") == std::string::npos ||
        missingWaveDiagnostic.find("hirari-wav-does-not-exist.w64") == std::string::npos) return 58;
    std::error_code wave64Cleanup;
    std::filesystem::remove(wave64Path, wave64Cleanup);
    std::filesystem::remove(wave64MultiPath, wave64Cleanup);
    std::filesystem::remove(malformedWave64Path, wave64Cleanup);
    std::filesystem::remove(pcm16Path, wave64Cleanup);
    std::filesystem::remove(pcm24Path, wave64Cleanup);
    std::filesystem::remove(streamPath, wave64Cleanup);
    std::filesystem::remove(pcm16StreamPath, wave64Cleanup);
    std::filesystem::remove(wave64StreamPath, wave64Cleanup);
    std::filesystem::remove(monoPcm16StreamPath, wave64Cleanup);
    std::filesystem::remove(monoPcm24StreamPath, wave64Cleanup);
    return 0;
}
