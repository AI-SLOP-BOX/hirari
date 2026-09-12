    Aura::Core::MidiBuffer consoleMidi;
    Aura::DSP::ProcessContext consoleContext{};
    consoleContext.sampleRate = 48'000.0; consoleContext.blockSize = 64;
    console.process(consoleBuffer, consoleMidi, consoleContext);
    for (uint32_t i = 0; i < 64; ++i)
        if (!std::isfinite(consoleBuffer.getReadPointer(0)[i])) return 72;
    const auto consoleState = console.getState();
    Aura::DSP::Effects::DivineConsoleStrip restoredConsole;
    if (!restoredConsole.setState(consoleState) || restoredConsole.getNumParameters() != 7) return 73;
    Aura::DSP::Effects::SidechainSpectralDucker ducker;
    ducker.prepareToPlay(48'000.0, 128);
    Aura::Core::AudioBuffer duckBuffer(2, 1024), sidechainBuffer(2, 1024);
    for (uint32_t i = 0; i < 1024; ++i) {
        duckBuffer.getWritePointer(0)[i] = 0.1f;
        duckBuffer.getWritePointer(1)[i] = -0.1f;
        sidechainBuffer.getWritePointer(0)[i] = (i % 32 == 0) ? 1.0f : 0.0f;
        sidechainBuffer.getWritePointer(1)[i] = sidechainBuffer.getReadPointer(0)[i];
    }
    Aura::DSP::ProcessContext duckContext{};
    duckContext.sampleRate = 48'000.0; duckContext.blockSize = 1024;
    duckContext.sidechainBuffer = &sidechainBuffer;
    Aura::Core::MidiBuffer duckMidi;
    ducker.process(duckBuffer, duckMidi, duckContext);
    for (uint32_t i = 0; i < 1024; ++i)
        if (!std::isfinite(duckBuffer.getReadPointer(0)[i]) || !std::isfinite(duckBuffer.getReadPointer(1)[i])) return 74;
    Aura::DSP::Effects::Arpeggiator arp;
    arp.prepareToPlay(48'000.0, 256);
    Aura::Core::AudioBuffer arpAudio(2, 256);
    Aura::Core::MidiBuffer arpMidi;
    arpMidi.addNoteOn(2, 60, 100, 0);
    arpMidi.addNoteOn(2, 64, 100, 1);
    Aura::DSP::ProcessContext arpContext{};
    arpContext.sampleRate = 48'000.0; arpContext.bpm = 120.0; arpContext.blockStart = 0; arpContext.blockSize = 256;
    arp.process(arpAudio, arpMidi, arpContext);
    Aura::Core::MidiBuffer releaseMidi;
    releaseMidi.addNoteOff(2, 64, 2);
    arp.process(arpAudio, releaseMidi, arpContext);
    if (arp.getNumParameters() != 1) return 75;
    Aura::DSP::Effects::StereoChorus chorus;
    chorus.prepareToPlay(48'000.0, 64);
    chorus.setParameter(0, 0.4f); chorus.setParameter(1, 0.65f);
    Aura::Core::AudioBuffer chorusBuffer(2, 64);
    for (uint32_t i = 0; i < 64; ++i) { chorusBuffer.getWritePointer(0)[i] = 0.2f; chorusBuffer.getWritePointer(1)[i] = -0.2f; }
    Aura::Core::MidiBuffer chorusMidi;
    chorus.process(chorusBuffer, chorusMidi, duckContext);
    for (uint32_t i = 0; i < 64; ++i)
        if (!std::isfinite(chorusBuffer.getReadPointer(0)[i]) || !std::isfinite(chorusBuffer.getReadPointer(1)[i])) return 76;
    Aura::DSP::Effects::StereoChorus restoredChorus;
    if (!restoredChorus.setState(chorus.getState()) || restoredChorus.getNumParameters() != 2) return 77;
    auto legacyDriver = Aura::IO::HardwareFactory::createDefault();
    if (!legacyDriver || !legacyDriver->initialize(48'000.0, 256) ||
        legacyDriver->getDeviceName().empty()) return 39;
    {
        using Transport = Aura::Core::Plugins::MidiFragmentReassembler;
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
        Aura::Core::MidiBuffer mailbox;
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
        auto extended = std::make_unique<Aura::Core::Plugins::MidiExtendedMessageRing>();
        if (mailbox.addFragment(largeTransport, largeFirst, extended.get()) != Transport::Result::Accepted ||
            mailbox.addFragment(largeTransport, largeSecond, extended.get()) != Transport::Result::Complete ||
            mailbox.size() != 1 || extended->size() != 1) return 57;
        Aura::Core::Plugins::MidiExtendedMessageRing::Message largeMessage;
        if (!extended->pop(largeMessage) || largeMessage.size != 2 * Transport::kFragmentPayloadBytes ||
            largeMessage.data[0] != 0x55 || largeMessage.data[largeMessage.size - 1] != 0xaa) return 58;
        Transport::Fragment wrongOrder = first;
        wrongOrder.messageId = 88;
        if (transport.push(wrongOrder) != Transport::Result::Accepted) return 55;
    }
    const auto wave64Path = std::filesystem::temp_directory_path() / "aura-native-wave64-contract.w64";
    const float waveLeft[] = {0.0f, 0.25f};
    const float waveRight[] = {-0.25f, 0.5f};
    if (!Aura::IO::Persistence::WavWriter::writeWave64(wave64Path.string(), waveLeft, waveRight, 2, 48'000)) return 43;
    {
        std::ifstream wave64(wave64Path, std::ios::binary);
        std::array<uint8_t, 24> header{};
        wave64.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!wave64 || header[0] != 0x52 || header[1] != 0x49 || header[2] != 0x46 || header[3] != 0x46) return 44;
    }
    Aura::IO::WavLoader::WavInfo waveInfo{};
    const auto waveChannels = Aura::IO::WavLoader::loadWave64(wave64Path.string(), waveInfo);
    if (waveInfo.sampleRate != 48'000 || waveInfo.numChannels != 2 || waveInfo.bitDepth != 32 ||
        waveChannels.size() != 2 || waveChannels[0].size() != 2 ||
        std::abs(waveChannels[1][1] - 0.5f) > 0.0001f) return 45;
    const auto wave64MultiPath = std::filesystem::temp_directory_path() / "aura-native-wave64-multichannel-contract.w64";
    const std::vector<std::vector<float>> wave64Multi{{0.0f, 0.25f}, {-0.25f, 0.5f}, {0.75f, -0.5f}};
    if (!Aura::IO::WavSaver::saveWave64(wave64MultiPath.string(), wave64Multi, 48'000)) return 52;
    Aura::IO::WavLoader::WavInfo wave64MultiInfo{};
    const auto wave64MultiDecoded = Aura::IO::WavLoader::loadWave64(
        wave64MultiPath.string(), wave64MultiInfo);
    if (wave64MultiInfo.numChannels != 3 || wave64MultiDecoded.size() != 3 ||
        wave64MultiDecoded[2].size() != 2 ||
        std::abs(wave64MultiDecoded[2][0] - 0.75f) > 0.0001f) return 53;
    const auto oversizedWave64Path = std::filesystem::temp_directory_path() /
                                     "aura-native-wave64-oversized-contract.w64";
    {
        std::ofstream oversized(oversizedWave64Path, std::ios::binary | std::ios::trunc);
        oversized.seekp(static_cast<std::streamoff>(Aura::IO::WavLoader::kMaximumDecodedBytes + 2ull * 1024ull * 1024ull - 1ull));
        oversized.put('\0');
    }
    bool oversizedRejected = false;
    try {
        Aura::IO::WavLoader::WavInfo oversizedInfo{};
        (void)Aura::IO::WavLoader::loadWave64(oversizedWave64Path.string(), oversizedInfo);
    } catch (const std::runtime_error&) {
        oversizedRejected = true;
    }
    std::filesystem::remove(oversizedWave64Path, offlineCleanup);
    if (!oversizedRejected) return 61;
    const auto reservedTempPath = std::filesystem::temp_directory_path() /
                                  "aura-native-wav-reservation-contract.tmp";
    std::filesystem::remove(reservedTempPath, offlineCleanup);
    if (!Aura::IO::Persistence::WavWriter::reserveTemporary(reservedTempPath) ||
        Aura::IO::Persistence::WavWriter::reserveTemporary(reservedTempPath)) return 62;
    std::filesystem::remove(reservedTempPath, offlineCleanup);
    const auto pcm16Path = std::filesystem::temp_directory_path() / "aura-native-pcm16-contract.wav";
    if (!Aura::Core::Utils::WavWriter::write(pcm16Path.string(), waveLeft, waveRight, 2, 48'000)) return 48;
    const auto pcm16MultiPath = std::filesystem::temp_directory_path() / "aura-native-pcm16-multichannel-contract.wav";
    const std::vector<std::vector<float>> pcm16Multi{{0.0f, 0.25f}, {-0.25f, 0.5f}, {0.75f, -0.5f}};
    if (!Aura::IO::Persistence::WavWriter::writePcm16Interleaved(
            pcm16MultiPath.string(), pcm16Multi, 48'000)) return 60;
    Aura::IO::WavLoader::WavInfo pcm16Info{};
    const auto pcm16Decoded = Aura::IO::WavLoader::load(pcm16MultiPath.string(), pcm16Info);
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
    const auto pcm24Path = std::filesystem::temp_directory_path() / "aura-native-pcm24-contract.wav";
    if (!Aura::Core::Utils::WavWriter::writePcm24(pcm24Path.string(), waveLeft, waveRight, 2, 48'000)) return 50;
    {
        std::ifstream pcm24(pcm24Path, std::ios::binary);
        std::array<uint8_t, 36> header{};
        pcm24.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!pcm24 || std::string(reinterpret_cast<const char*>(header.data()), 4) != "RIFF" ||
            header[34] != 24 || header[35] != 0) return 51;
    }
    const auto legacyBouncePath = std::filesystem::temp_directory_path() /
                                  "aura-native-legacy-bounce-contract.wav";
    Aura::Core::Engine::LegacyBounceEngine::ExportProgress legacyProgress;
    Aura::Core::Engine::LegacyBounceEngine::renderToFile(
        legacyBouncePath.string(), 0.001, 48'000.0, legacyProgress,
        [](float* left, float* right, uint32_t count) {
            for (uint32_t i = 0; i < count; ++i) { left[i] = 0.25f; right[i] = -0.25f; }
        });
    if (!legacyProgress.isDone.load() || legacyProgress.cancelled.load() ||
        !std::filesystem::is_regular_file(legacyBouncePath)) return 61;
    std::ifstream legacyBounce(legacyBouncePath, std::ios::binary);
    std::array<uint8_t, 36> legacyHeader{};
    legacyBounce.read(reinterpret_cast<char*>(legacyHeader.data()),
                      static_cast<std::streamsize>(legacyHeader.size()));
    if (!legacyBounce || std::string(legacyHeader.begin(), legacyHeader.begin() + 4) != "RIFF" ||
        legacyHeader[34] != 16 || legacyHeader[35] != 0) return 62;
    const auto streamPath = std::filesystem::temp_directory_path() / "aura-native-pcm24-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm24StreamWriter stream(
            streamPath.string(), 4, 48'000);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, waveRight, 2) ||
            !stream.writeFrames(waveLeft, waveRight, 2) || !stream.finish()) return 59;
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
                                 "aura-native-pcm16-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm16StreamWriter stream(
            pcm16StreamPath.string(), 4, 48'000);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, waveRight, 1) ||
            !stream.writeFrames(waveLeft + 1, waveRight + 1, 3) || !stream.finish()) return 63;
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
                                  "aura-native-wave64-stream-contract.w64";
    {
        Aura::IO::Persistence::WavWriter::Wave64FloatStreamWriter stream(
            wave64StreamPath.string(), 4, 48'000, 2);
        const float streamLeft[4] = {0.0f, 0.25f, 0.5f, 0.75f};
        const float streamRight[4] = {-0.25f, 0.5f, -0.75f, 1.0f};
        const float* channels[2] = {streamLeft, streamRight};
        if (!stream.isOpen() || !stream.writeFrames(channels, 2) ||
            !stream.writeFrames(channels, 2) || stream.framesWritten() != 4 ||
            !stream.finish()) return 76;
        Aura::IO::WavLoader::WavInfo info{};
        const auto decoded = Aura::IO::WavLoader::loadWave64(wave64StreamPath.string(), info);
        if (info.sampleRate != 48'000 || info.numChannels != 2 || info.bitDepth != 32 ||
            decoded.size() != 2 || decoded[0].size() != 4 ||
            std::abs(decoded[1][1] - 0.5f) > 0.0001f ||
            std::filesystem::file_size(wave64StreamPath) != 104 + 4 * 2 * sizeof(float)) return 77;
    }
    const auto monoPcm16StreamPath = std::filesystem::temp_directory_path() /
                                     "aura-native-pcm16-mono-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm16StreamWriter stream(
            monoPcm16StreamPath.string(), 4, 48'000, 1);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, nullptr, 4) || !stream.finish()) return 65;
        std::ifstream streamInput(monoPcm16StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || streamHeader[22] != 1 || streamHeader[34] != 16 ||
            std::filesystem::file_size(monoPcm16StreamPath) != 52) return 66;
    }
    const auto monoPcm24StreamPath = std::filesystem::temp_directory_path() /
                                     "aura-native-pcm24-mono-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm24StreamWriter stream(
            monoPcm24StreamPath.string(), 4, 48'000, false, 1);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, nullptr, 4) || !stream.finish()) return 67;
        std::ifstream streamInput(monoPcm24StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || streamHeader[22] != 1 || streamHeader[34] != 24 ||
            std::filesystem::file_size(monoPcm24StreamPath) != 56) return 68;
    }
    const auto floatStreamPath = std::filesystem::temp_directory_path() /
                                 "aura-native-float32-stream-contract.wav";
    {
        const float floatLeft[2] = {0.25f, std::numeric_limits<float>::quiet_NaN()};
        const float floatRight[2] = {-0.5f, 0.75f};
        const float* floatChannels[2] = {floatLeft, floatRight};
        Aura::IO::Persistence::WavWriter::Float32StreamWriter stream(
            floatStreamPath.string(), 48'000, 2);
        if (!stream.isOpen() || !stream.writeFrames(floatChannels, 2) ||
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
    const auto recordingPath = std::filesystem::temp_directory_path() /
                               "aura-native-recording-engine-contract.wav";
    {
        Aura::Core::RecordingEngine recorder;
        const float left[4] = {0.1f, 0.2f, 0.3f, 0.4f};
        const float right[4] = {-0.1f, -0.2f, -0.3f, -0.4f};
        if (!recorder.start(recordingPath.string(), 48'000.0) ||
            !recorder.write(left, right, 4)) return 72;
        recorder.stop();
        if (recorder.hasWriteError() || !std::filesystem::is_regular_file(recordingPath) ||
            std::filesystem::file_size(recordingPath) != 80 + 4 * 2 * sizeof(float)) return 73;
        std::ifstream recordingInput(recordingPath, std::ios::binary);
        std::array<uint8_t, 80> recordingHeader{};
        recordingInput.read(reinterpret_cast<char*>(recordingHeader.data()),
                            static_cast<std::streamsize>(recordingHeader.size()));
        if (!recordingInput || std::string(recordingHeader.begin(), recordingHeader.begin() + 4) != "RIFF" ||
            recordingHeader[56] != 3 || recordingHeader[57] != 0 ||
            recordingHeader[58] != 2 || recordingHeader[59] != 0) return 74;
    }
    const auto malformedWave64Path = std::filesystem::temp_directory_path() / "aura-native-wave64-malformed.w64";
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
        Aura::IO::WavLoader::WavInfo malformedInfo{};
        (void)Aura::IO::WavLoader::loadWave64(malformedWave64Path.string(), malformedInfo);
    } catch (const std::exception&) {
        malformedRejected = true;
    }
    if (!malformedRejected) return 47;
    const auto validWaveDiagnostic = Aura::IO::WavLoader::loadDiagnosticJson(
        wave64Path.string(), true);
    if (validWaveDiagnostic.find("\"ok\":true") == std::string::npos ||
        validWaveDiagnostic.find("\"format\":\"WAVE64\"") == std::string::npos ||
        validWaveDiagnostic.find("\"channels\":2") == std::string::npos) return 56;
    const auto malformedWaveDiagnostic = Aura::IO::WavLoader::loadDiagnosticJson(
        malformedWave64Path.string(), true);
    if (malformedWaveDiagnostic.find("\"ok\":false") == std::string::npos ||
        malformedWaveDiagnostic.find("\"error\":\"") == std::string::npos) return 57;
    const auto missingWaveDiagnostic = Aura::IO::WavLoader::loadDiagnosticJson(
        (wave64Path.parent_path() / "aura-wav-does-not-exist.w64").string(), true);
    if (missingWaveDiagnostic.find("\"ok\":false") == std::string::npos ||
        missingWaveDiagnostic.find("WAVE64 file not found") == std::string::npos) return 58;
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
    std::filesystem::remove(legacyBouncePath, wave64Cleanup);
    return 0;
}
