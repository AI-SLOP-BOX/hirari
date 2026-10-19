    const auto uppercasePath = std::filesystem::temp_directory_path() /
                               "hirari-admission-contract-uppercase.VST3";
    {
        std::ofstream output(uppercasePath, std::ios::binary | std::ios::trunc);
        output << "uppercase-extension";
    }
    if (!Hirari::Core::Plugins::PluginAdmission::isSafeCandidate(uppercasePath, "vst3")) return 32;
    std::filesystem::remove(uppercasePath, cleanup);

    const auto project = std::filesystem::temp_directory_path() / "hirari-region-id-contract.bin";
    Hirari::Core::ProjectSerializer::ProjectState state;
    state.version = 28;
    state.sampleRate = 48'000;
    state.bpm = 120.0;
    Hirari::Core::ProjectSerializer::TrackState baseTrack{};
    baseTrack.id = 7;
    baseTrack.volume = 1.0f;
    baseTrack.pan = 0.0f;
    state.tracks.push_back(baseTrack);
    Hirari::Core::ProjectSerializer::RegionState region;
    region.id = 9001;
    region.trackId = 7;
    region.samplePosition = 0;
    region.sampleLength = 128;
    region.filePath = "contract.wav";
    state.regions.push_back(region);
    if (!Hirari::Core::ProjectSerializer::saveAtomic(project.string(), state)) return 5;
    const auto restored = Hirari::Core::ProjectSerializer::load(project.string());
    if (!restored.valid || restored.regions.size() != 1 || restored.regions[0].id != 9001) return 6;

    Hirari::Core::ProjectSerializer::ProjectState duplicateIds = state;
    Hirari::Core::ProjectSerializer::TrackState firstTrack = baseTrack;
    Hirari::Core::ProjectSerializer::TrackState duplicateTrack = firstTrack;
    duplicateIds.tracks = {firstTrack, duplicateTrack};
    const auto duplicateTrackProject = std::filesystem::temp_directory_path() /
                                       "hirari-duplicate-track-id-contract.bin";
    if (!Hirari::Core::ProjectSerializer::saveAtomic(duplicateTrackProject.string(), duplicateIds)) return 61;
    const auto rejectedDuplicateTrack = Hirari::Core::ProjectSerializer::load(duplicateTrackProject.string());
    std::filesystem::remove(duplicateTrackProject, cleanup);
    if (rejectedDuplicateTrack.valid) return 62;

    Hirari::Core::ProjectSerializer::ProjectState orphanRegion = state;
    orphanRegion.regions[0].trackId = 999;
    const auto orphanRegionProject = std::filesystem::temp_directory_path() /
                                     "hirari-orphan-region-contract.bin";
    if (!Hirari::Core::ProjectSerializer::saveAtomic(orphanRegionProject.string(), orphanRegion)) return 63;
    const auto rejectedOrphanRegion = Hirari::Core::ProjectSerializer::load(orphanRegionProject.string());
    std::filesystem::remove(orphanRegionProject, cleanup);
    if (rejectedOrphanRegion.valid) return 64;

    {
        std::fstream corrupt(project, std::ios::in | std::ios::out | std::ios::binary);
        corrupt.seekg(12, std::ios::beg);
        char byte = 0;
        corrupt.read(&byte, 1);
        corrupt.seekp(12, std::ios::beg);
        byte ^= static_cast<char>(0x5a);
        corrupt.write(&byte, 1);
    }
    const auto rejectedCorruption = Hirari::Core::ProjectSerializer::load(project.string());
    std::filesystem::remove(project, cleanup);
    if (rejectedCorruption.valid) return 34;

    // The loader must reject an oversized project before allocating the file
    // buffer.  This is intentionally sparse so the contract stays cheap.
    const auto oversizedProject = std::filesystem::temp_directory_path() /
                                  "hirari-oversized-project-contract.bin";
    {
        std::ofstream oversized(oversizedProject, std::ios::binary | std::ios::trunc);
        if (!oversized) return 59;
        oversized.seekp(static_cast<std::streamoff>(Hirari::Core::ProjectSerializer::kMaxProjectBytes));
        oversized.put('\0');
    }
    const auto rejectedOversized = Hirari::Core::ProjectSerializer::load(oversizedProject.string());
    std::filesystem::remove(oversizedProject, cleanup);
    if (rejectedOversized.valid) return 60;



    // Exercise the production scheduler rather than only compiling its
    // headers. In particular, stolen tasks must execute exactly once and the
    // scheduler must be safely destructible after worker shutdown.
    Hirari::Core::Concurrency::AudioTaskStealingScheduler scheduler;
    scheduler.start(2);
    auto completed = std::make_shared<std::atomic<uint32_t>>(0);
    for (uint32_t i = 0; i < 64; ++i) {
        if (!scheduler.postTaskAsync(i, [completed]() {
                completed->fetch_add(1, std::memory_order_acq_rel);
            })) return 13;
    }
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (completed->load(std::memory_order_acquire) != 64 &&
           std::chrono::steady_clock::now() < deadline) {
        std::this_thread::yield();
    }
    scheduler.stop();
    if (completed->load(std::memory_order_acquire) != 64) return 14;

    std::error_code offlineCleanup;

    const auto saverPath = std::filesystem::temp_directory_path() / "hirari-wav-saver-contract.wav";
    const std::vector<std::vector<float>> saverAudio = {
        {0.25f, -0.25f, 0.0f, 0.5f},
        {-0.5f, 0.125f, 0.0f, -0.25f}
    };
    if (!Hirari::IO::WavSaver::save(saverPath.string(), saverAudio, 48'000)) return 22;
    Hirari::IO::WavLoader::WavInfo saverInfo{};
    try {
        const auto decoded = Hirari::IO::WavLoader::load(saverPath.string(), saverInfo);
        if (decoded.size() != 2 || decoded[0].size() != 4 || saverInfo.sampleRate != 48'000 ||
            std::abs(decoded[0][0] - 0.25f) > 0.01f || std::abs(decoded[1][1] - 0.125f) > 0.01f) return 23;
        Hirari::IO::WavLoader::WavInfo legacyInfo{};
        const auto legacyDecoded = Hirari::IO::WavLoader::loadLegacy(saverPath.string(), legacyInfo);
        if (legacyDecoded.size() != 2 || legacyDecoded[0].size() != 4 || legacyInfo.bitDepth != 24 ||
            std::abs(legacyDecoded[0][0] - 0.25f) > 0.01f) return 27;
    } catch (...) {
        return 24;
    }
    std::filesystem::remove(saverPath, offlineCleanup);

    const auto multichannelSaverPath = std::filesystem::temp_directory_path() / "hirari-wav-saver-multichannel-contract.wav";
    const std::vector<std::vector<float>> multichannelAudio = {
        {0.1f, 0.2f, 0.3f}, {-0.1f, -0.2f, -0.3f}, {0.0f, 0.4f, -0.4f}
    };
    if (!Hirari::IO::WavSaver::save(multichannelSaverPath.string(), multichannelAudio, 48'000)) return 28;
    try {
        Hirari::IO::WavLoader::WavInfo multichannelInfo{};
        const auto decoded = Hirari::IO::WavLoader::load(multichannelSaverPath.string(), multichannelInfo);
        if (decoded.size() != 3 || decoded[0].size() != 3 || multichannelInfo.bitDepth != 24 ||
            std::abs(decoded[2][1] - 0.4f) > 0.01f) return 29;
    } catch (...) {
        return 30;
    }
    std::filesystem::remove(multichannelSaverPath, offlineCleanup);

    // Exercise the canonical decoder across its bounded read chunk boundary;
    // this guards against regressions that only pass on tiny fixture files.
    const auto chunkedSaverPath = std::filesystem::temp_directory_path() / "hirari-wav-saver-chunked-contract.wav";
    std::vector<std::vector<float>> chunkedAudio(2, std::vector<float>(20'000));
    for (size_t frame = 0; frame < chunkedAudio[0].size(); ++frame) {
        chunkedAudio[0][frame] = (frame % 97 == 0) ? 0.75f : -0.125f;
        chunkedAudio[1][frame] = (frame % 113 == 0) ? -0.5f : 0.25f;
    }
    if (!Hirari::IO::WavSaver::save(chunkedSaverPath.string(), chunkedAudio, 48'000)) return 31;
    try {
        Hirari::Core::IO::WavDecoder chunkedDecoder;
        if (!chunkedDecoder.open(chunkedSaverPath.string())) return 32;
        Hirari::Core::AudioBuffer chunkedBuffer;
        chunkedDecoder.decodeFull(chunkedBuffer);
        if (chunkedBuffer.getNumChannels() != 2 || chunkedBuffer.getNumSamples() != 20'000 ||
            std::abs(chunkedBuffer.getReadPointer(0)[16'384] + 0.125f) > 0.01f ||
            std::abs(chunkedBuffer.getReadPointer(1)[19'999] - 0.25f) > 0.01f) return 33;
    } catch (...) {
        return 34;
    }
    std::filesystem::remove(chunkedSaverPath, offlineCleanup);

    const auto oversizedExpansionPath = std::filesystem::temp_directory_path() /
                                        "hirari-wav-expansion-limit-contract.wav";
    {
        constexpr uint32_t expansionDataBytes = 300u * 1024u * 1024u;
        std::ofstream oversized(oversizedExpansionPath, std::ios::binary | std::ios::trunc);
        const uint32_t riffSize = 36u + expansionDataBytes;
        const uint32_t fmtSize = 16;
        const uint16_t pcm = 1, channels = 2, align = 4, bits = 16;
        const uint32_t sampleRate = 48'000, byteRate = sampleRate * align;
        const uint32_t dataId = 0x61746164u;
        oversized.write("RIFF", 4);
        oversized.write(reinterpret_cast<const char*>(&riffSize), sizeof(riffSize));
        oversized.write("WAVEfmt ", 8);
        oversized.write(reinterpret_cast<const char*>(&fmtSize), sizeof(fmtSize));
        oversized.write(reinterpret_cast<const char*>(&pcm), sizeof(pcm));
        oversized.write(reinterpret_cast<const char*>(&channels), sizeof(channels));
        oversized.write(reinterpret_cast<const char*>(&sampleRate), sizeof(sampleRate));
        oversized.write(reinterpret_cast<const char*>(&byteRate), sizeof(byteRate));
        oversized.write(reinterpret_cast<const char*>(&align), sizeof(align));
        oversized.write(reinterpret_cast<const char*>(&bits), sizeof(bits));
        oversized.write(reinterpret_cast<const char*>(&dataId), sizeof(dataId));
        oversized.write(reinterpret_cast<const char*>(&expansionDataBytes), sizeof(expansionDataBytes));
        oversized.seekp(static_cast<std::streamoff>(44ull + expansionDataBytes - 1ull));
        oversized.put('\0');
    }
    // A sparse/expanded WAV must never be turned into a file-sized buffer.
    //
    // This contract previously required open() to succeed and then yield an
    // empty buffer. That encoded the old architecture where open() only parsed
    // the RIFF header and decodeFull did the work. open() now performs the whole
    // decode itself (decodeFull just copies the result), so the meaningful
    // guarantee is stronger: a file whose decoded PCM would exceed the 512 MiB
    // limit is refused outright rather than silently accepted and truncated.
    //
    // 300 MiB of 16-bit stereo PCM decodes to roughly 629 MB of f32, which is
    // over that limit, so open() must fail and must not publish any channels.
    Hirari::Core::IO::WavDecoder oversizedDecoder;
    Hirari::Core::AudioBuffer oversizedBuffer;
    const bool openedOversized = oversizedDecoder.open(oversizedExpansionPath.string());
    if (openedOversized) return 35;
    // A refused decode must leave the decoder with no buffer at all.
    oversizedDecoder.decodeFull(oversizedBuffer);
    std::filesystem::remove(oversizedExpansionPath, offlineCleanup);
    if (oversizedBuffer.getNumChannels() != 0 || oversizedBuffer.getNumSamples() != 0) return 36;

    auto& waveformCache = Hirari::UI::WaveformCache::getInstance();
    waveformCache.clearRegion(77);
    Hirari::UI::WaveformLevel waveform;
    waveform.minPeaks = {-0.5f, -0.25f};
    waveform.maxPeaks = {0.5f, 0.25f};
    if (!waveformCache.requestWaveform(77, 2, std::vector<float>(2'000'000, 0.1f))) return 29;
    waveformCache.putWaveform(77, 2, waveform);
    waveformCache.waitForPending();
    const auto* waveformSnapshot = waveformCache.getWaveform(77, 2);
    if (waveformSnapshot == nullptr || waveformSnapshot->minPeaks[0] != -0.5f) return 25;
    waveformCache.clearRegion(77);
    if (!waveformCache.requestWaveform(77, 2, std::vector<float>(2'000'000, 0.2f))) return 30;
    waveformCache.clearRegion(77);
    waveformCache.waitForPending();
    if (waveformCache.getWaveform(77, 2) != nullptr) return 31;


    const auto pluginFingerprintPath = std::filesystem::temp_directory_path() /
                                       "hirari-plugin-fingerprint-contract.clap";
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v1";
    }
    const auto fingerprintV1 = Hirari::Core::Plugins::PluginCacheManager::fingerprintForPath(
        pluginFingerprintPath);
    if (!fingerprintV1) return 32;
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v2-with-new-content";
    }
    const auto fingerprintV2 = Hirari::Core::Plugins::PluginCacheManager::fingerprintForPath(
        pluginFingerprintPath);
    auto& pluginCache = Hirari::Core::Plugins::PluginCacheManager::getInstance();
    pluginCache.recordScanFailure(pluginFingerprintPath.string(), 17);
    if (!pluginCache.isBlacklisted(pluginFingerprintPath.string())) return 43;
    const auto aliasPath = pluginFingerprintPath.parent_path() / "." /
                           pluginFingerprintPath.filename();
    if (!pluginCache.isBlacklisted(aliasPath.string())) return 47;
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v3-replaced";
    }
    if (pluginCache.isBlacklisted(pluginFingerprintPath.string())) return 44;
    pluginCache.clearScanFailure(aliasPath.string());
    std::filesystem::remove(pluginFingerprintPath, offlineCleanup);
    if (!fingerprintV2 || *fingerprintV1 == *fingerprintV2) return 33;



    Hirari::Core::Engine::TimelineSystem encodedTimeline;
    auto encodedTrack = std::make_shared<Hirari::Core::Engine::Track>(
        77, "VCA Contract", Hirari::Core::Engine::Track::Audio);
    auto midiRegion = std::make_shared<Hirari::Core::MidiRegion>(88, "MIDI Contract", 0.0, 4.0);
    midiRegion->addNote(Hirari::Core::MIDINote{60, 100, 0.0, 1.0});
    encodedTrack->addMidiRegion(midiRegion);
    encodedTimeline.addTrack(encodedTrack);
    {
        // Freeze playback is a real native path: publish happens off the
        // audio thread, while Track::process only copies from the immutable
        // bounded snapshot and bypasses the live region/effect graph.
        auto frozen = std::make_shared<Hirari::Core::AudioBuffer>(2, 8);
        for (uint32_t index = 0; index < 8; ++index) {
            frozen->getWritePointer(0)[index] = static_cast<float>(index + 1);
            frozen->getWritePointer(1)[index] = static_cast<float>(100 + index);
        }
        auto freezeTrack = std::make_shared<Hirari::Core::Engine::Track>(79, "Frozen", Hirari::Core::Engine::Track::Audio);
        freezeTrack->prepareToPlay(48'000.0, 8);
        if (!freezeTrack->publishFrozenAudio(frozen, 8, 48'000.0, 11, 13) ||
            !freezeTrack->isFrozen() || freezeTrack->frozenProjectGeneration() != 11 ||
            freezeTrack->frozenAudioGeneration() != 13) return 65;
        Hirari::Core::AudioBuffer rendered(2, 8);
        rendered.clear();
        freezeTrack->process(rendered, 8, 0);
        if (rendered.getReadPointer(0)[0] != 1.0f ||
            rendered.getReadPointer(0)[7] != 8.0f ||
            rendered.getReadPointer(1)[0] != 100.0f ||
            rendered.getReadPointer(1)[7] != 107.0f) return 66;
        freezeTrack->clearFrozenAudio();
        if (freezeTrack->isFrozen()) return 67;
        auto renderedTrack = std::make_shared<Hirari::Core::Engine::Track>(80, "Rendered Freeze", Hirari::Core::Engine::Track::Audio);
        Hirari::Core::Engine::Region sourceRegion{};
        sourceRegion.id = 1;
        sourceRegion.start = 0;
        sourceRegion.len = 8;
        sourceRegion.muted = false;
        sourceRegion.name = "freeze-source";
        sourceRegion.audio = frozen;
        renderedTrack->addRegion(sourceRegion);
        if (!Hirari::Core::Engine::TrackFreezeManager::freezeTrack(renderedTrack, 8, 48'000) ||
            !renderedTrack->isFrozen()) return 68;
        Hirari::Core::Engine::TrackFreezeManager::unfreezeTrack(renderedTrack);
        renderedTrack->collectRetiredFrozenAudio();
        if (renderedTrack->isFrozen()) return 69;
    }
    auto& vca = Hirari::Core::Engine::VCAControlSystem::getInstance();
    vca.createGroup("Contract VCA", {77});
    const auto encodedProject = Hirari::IO::Persistence::ProjectEncoder::encode(encodedTimeline);
    if (encodedProject.find("\"vca_groups\"") == std::string::npos ||
        encodedProject.find("Contract VCA") == std::string::npos ||
        encodedProject.find("77") == std::string::npos ||
        encodedProject.find("MIDI Contract") == std::string::npos ||
        encodedProject.find("\"pitch\": 60") == std::string::npos) return 42;

    Hirari::DSP::Analysis::SpectralProcessor spectral;
    Hirari::Core::AudioBuffer invalidSpectralBuffer(1, 128);
    spectral.applyMask(invalidSpectralBuffer, 48'000.0,
                       Hirari::DSP::Analysis::SpectralProcessor::Rect{
                           std::numeric_limits<float>::quiet_NaN(), 0.0f, 1.0f, 100.0f},
                       0.0f);
    if (spectral.canUndo()) return 80;
    std::string spectralFactoryError;
    auto spectralPlugin = Hirari::Core::Plugins::PluginFactory::create(
        Hirari::Core::Plugins::PluginDescription{"Spectral Restoration", "Hirari", Hirari::Core::Plugins::PluginFormat::Internal, ""},
        &spectralFactoryError);
    if (!spectralPlugin || spectralPlugin->getName() != "Spectral Restoration" ||
        !spectralFactoryError.empty()) return 75;
    if (spectralPlugin->getTailSamples() <= spectralPlugin->getLatencySamples()) return 92;
    spectralPlugin->setBypassed(true);
    spectralPlugin->setSidechainBus(7);
    const auto spectralState = spectralPlugin->getState();
    if (spectralState.size() != 24u) return 76;
    auto restoredSpectral = Hirari::Core::Plugins::PluginFactory::create(
        Hirari::Core::Plugins::PluginDescription{"Spectral Restoration", "Hirari", Hirari::Core::Plugins::PluginFormat::Internal, ""});
    if (!restoredSpectral || !restoredSpectral->setState(spectralState) ||
        !restoredSpectral->isBypassed() || restoredSpectral->getSidechainBus() != 7u) return 77;
    Hirari::Core::AudioBuffer spectralBuffer(1, 2048);
    spectralBuffer.getWritePointer(0)[0] = 1.0f;
    spectral.applyMask(spectralBuffer, 48'000.0,
                       Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 1.0f, 24'000.0f},
                       0.5f);
    if (!std::isfinite(spectralBuffer.getReadPointer(0)[0])) return 38;
    Hirari::Core::AudioBuffer multiRegion(1, 4096);
    for (uint32_t i = 0; i < multiRegion.getNumSamples(); ++i)
        multiRegion.getWritePointer(0)[i] = 0.25f;
    double energyBefore = 0.0;
    for (uint32_t i = 0; i < multiRegion.getNumSamples(); ++i)
        energyBefore += std::fabs(multiRegion.getReadPointer(0)[i]);
    const std::vector<Hirari::DSP::Analysis::SpectralProcessor::Rect> regions{
        {0.0f, 0.0f, 0.04f, 900.0f},
        {0.04f, 1'100.0f, 0.08f, 2'000.0f}};
    spectral.applySpectralGain(multiRegion, 48'000.0, regions, 0.0f);
    double energyAfter = 0.0;
    for (uint32_t i = 0; i < multiRegion.getNumSamples(); ++i)
        energyAfter += std::fabs(multiRegion.getReadPointer(0)[i]);
    if (!spectral.canUndo(multiRegion) || spectral.undoDepth(multiRegion) != 1u ||
        !(energyAfter < energyBefore)) return 88;
    if (!spectral.undo(multiRegion) || spectral.canUndo(multiRegion)) return 89;
    Hirari::Core::AudioBuffer brushRegion(1, 4096);
    for (uint32_t i = 0; i < brushRegion.getNumSamples(); ++i)
        brushRegion.getWritePointer(0)[i] = 0.25f;
    const std::vector<Hirari::DSP::Analysis::SpectralProcessor::RegionGain> brushMask{
        {{0.0f, 0.0f, 0.04f, 900.0f}, 0.0f},
        {{0.04f, 1'100.0f, 0.08f, 2'000.0f}, 2.0f}};
    spectral.applySpectralMask(brushRegion, 48'000.0, brushMask);
    if (!spectral.canUndo(brushRegion) || spectral.undoLabel(brushRegion) != "Spectral Brush" ||
        !spectral.undo(brushRegion)) return 93;
    Hirari::Core::AudioBuffer selectedClicks(1, 2048);
    selectedClicks.getWritePointer(0)[300] = 1.0f;
    selectedClicks.getWritePointer(0)[1500] = 1.0f;
    if (spectral.removeClicks(selectedClicks, 48'000.0,
                              Hirari::DSP::Analysis::SpectralProcessor::Rect{0.005f, 0.0f, 0.010f, 20'000.0f},
                              0.1f, 2) == 0 ||
        std::fabs(selectedClicks.getReadPointer(0)[1500] - 1.0f) > 1.0e-6f ||
        std::fabs(selectedClicks.getReadPointer(0)[300]) > 0.5f) return 90;
    Hirari::Core::AudioBuffer selectedClips(1, 2048);
    selectedClips.getWritePointer(0)[499] = 0.1f;
    selectedClips.getWritePointer(0)[500] = 1.0f;
    selectedClips.getWritePointer(0)[501] = 1.0f;
    selectedClips.getWritePointer(0)[502] = 0.1f;
    selectedClips.getWritePointer(0)[1500] = 1.0f;
    if (spectral.repairClipped(selectedClips, 48'000.0,
                               Hirari::DSP::Analysis::SpectralProcessor::Rect{0.009f, 0.0f, 0.011f, 20'000.0f},
                               0.98f) == 0 ||
        std::fabs(selectedClips.getReadPointer(0)[1500] - 1.0f) > 1.0e-6f ||
        std::fabs(selectedClips.getReadPointer(0)[500] - 1.0f) < 1.0e-3f) return 91;
    spectralBuffer.getWritePointer(0)[100] = 1.0f;
    spectralBuffer.getWritePointer(0)[101] = -1.0f;
    if (spectral.removeClicks(spectralBuffer, 0.25f, 2) == 0) return 68;
    spectralBuffer.getWritePointer(0)[200] = 1.0f;
    spectralBuffer.getWritePointer(0)[201] = 1.0f;
    if (spectral.repairClipped(spectralBuffer, 0.98f) == 0) return 67;
    spectral.reduceNoise(spectralBuffer, 48'000.0, 0.75f, 0.05f);
    spectral.interpolateRegion(
        spectralBuffer, 48'000.0,
        Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 200.0f, 0.02f, 2'000.0f},
        0.8f);
    for (uint32_t i = 0; i < spectralBuffer.getNumSamples(); ++i)
        if (!std::isfinite(spectralBuffer.getReadPointer(0)[i])) return 70;
    if (!spectral.canUndo() || !spectral.undo(spectralBuffer) || !spectral.canRedo() ||
        !spectral.redo(spectralBuffer)) return 74;
    if (spectral.undoDepth() == 0 || spectral.redoDepth() != 0) return 82;
    Hirari::DSP::Analysis::SpectralProcessor ownershipHistory;
    Hirari::Core::AudioBuffer ownerA(1, 128), ownerB(1, 128);
    ownershipHistory.applyMask(ownerA, 48'000.0,
                               Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 1.0f, 2'000.0f},
                               0.5f);
    if (ownershipHistory.undo(ownerB) || !ownershipHistory.canUndo()) return 85;
    ownershipHistory.applyMask(ownerB, 48'000.0,
                               Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 1.0f, 2'000.0f},
                               0.5f);
    if (!ownershipHistory.canUndo(ownerA) || !ownershipHistory.undo(ownerA) ||
        !ownershipHistory.canUndo(ownerB)) return 86;
    ownershipHistory.clearHistory(ownerA);
    if (ownershipHistory.canUndo(ownerA) || !ownershipHistory.canUndo(ownerB)) return 87;
    Hirari::DSP::Effects::SpectralRestorationProcessor restoration(48'000.0);
    restoration.applySpectralGain(
        spectralBuffer,
        Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 100.0f, 0.02f, 8'000.0f},
        0.25f);
    restoration.healRegion(
        spectralBuffer,
        Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 100.0f, 0.02f, 8'000.0f},
        0.5f);
    restoration.reduceNoise(
        spectralBuffer,
        Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 0.02f, 0.0f},
        0.5f, 0.05f);
    Hirari::Core::AudioBuffer localNoise(1, 4096);
    for (uint32_t i = 0; i < localNoise.getNumSamples(); ++i)
        localNoise.getWritePointer(0)[i] = 0.1f;
    restoration.reduceNoise(
        localNoise,
        Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 0.01f, 0.0f},
        0.5f, 0.05f);
    if (std::fabs(localNoise.getReadPointer(0)[3500] - 0.1f) > 1.0e-6f) return 79;
    restoration.eraseHarmonics(
        spectralBuffer, 60.0f,
        Hirari::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 0.02f, 4'000.0f},
        4, 4.0f);
    if (restoration.undoLabelOffline() != "Remove Hum") return 84;
    if (!restoration.canUndoOffline() || !restoration.undoOffline(spectralBuffer) ||
        !restoration.canRedoOffline() || restoration.redoLabelOffline() != "Remove Hum" ||
        !restoration.redoOffline(spectralBuffer)) return 78;
    if (restoration.undoDepthOffline() == 0 || restoration.redoDepthOffline() != 0) return 83;
    restoration.reset();
    if (restoration.canUndoOffline() || restoration.canRedoOffline()) return 81;
