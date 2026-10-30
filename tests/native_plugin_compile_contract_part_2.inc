    // The ARUA container now lives in the Rust bridge, so the contract drives
    // the same entrypoints the engine calls to keep the cxx marshalling and
    // link surface covered. The full rejection matrix (duplicate track ids,
    // orphan regions, corruption and oversized sparse input) lives beside the
    // codec in
    // `native_project::tests::rejects_duplicate_tracks_orphan_regions_corruption_and_oversize`.
    {
        const auto project =
            std::filesystem::temp_directory_path() / "hirari-region-id-contract.bin";
        Hirari::Core::Bridge::NativeProjectState state;
        state.version = 28;
        state.sampleRate = 48'000;
        state.bpm = 120.0;
        Hirari::Core::Bridge::NativeTrackState track;
        track.id = 7;
        track.volume = 1.0f;
        track.pan = 0.0f;
        Hirari::Core::Bridge::NativeRegionState region;
        region.id = 9001;
        region.trackId = 7;
        region.samplePosition = 0;
        region.sampleLength = 128;
        region.filePath = "contract.wav";
        // The version-28 decoder requires a usable warp ratio and a positive
        // loop count; the C++ `RegionState` defaults supplied both.
        region.warpRatio = 1.0;
        region.loopCount = 1;
        state.tracks.push_back(std::move(track));
        state.regions.push_back(std::move(region));
        if (!Hirari::Core::Bridge::native_project_save(project.string(), state)) return 5;
        const auto restored = Hirari::Core::Bridge::native_project_load(project.string());
        if (!restored.valid || restored.regions.size() != 1 || restored.regions[0].id != 9001) {
            return 6;
        }
        std::filesystem::remove(project, cleanup);
    }

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

    const auto pluginFingerprintPath = std::filesystem::temp_directory_path() /
                                       "hirari-plugin-fingerprint-contract.clap";
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v1";
    }
    const auto fingerprintV1 = Hirari::Core::Plugins::PluginScanCache::fingerprintForPath(
        pluginFingerprintPath);
    if (!fingerprintV1) return 32;
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v2-with-new-content";
    }
    const auto fingerprintV2 = Hirari::Core::Plugins::PluginScanCache::fingerprintForPath(
        pluginFingerprintPath);
    Hirari::Core::Plugins::PluginScanCache::recordScanFailure(pluginFingerprintPath.string(), 17);
    if (Hirari::Core::Plugins::PluginScanCache::shouldScan(pluginFingerprintPath.string())) return 43;
    const auto aliasPath = pluginFingerprintPath.parent_path() / "." /
                           pluginFingerprintPath.filename();
    if (Hirari::Core::Plugins::PluginScanCache::shouldScan(aliasPath.string())) return 47;
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v3-replaced";
    }
    if (!Hirari::Core::Plugins::PluginScanCache::shouldScan(pluginFingerprintPath.string())) return 44;
    Hirari::Core::Plugins::PluginScanCache::recordScanSuccess(aliasPath.string());
    if (!Hirari::Core::Plugins::PluginScanCache::shouldScan(pluginFingerprintPath.string())) return 48;
    std::filesystem::remove(pluginFingerprintPath, offlineCleanup);
    if (!fingerprintV2 || *fingerprintV1 == *fingerprintV2) return 33;



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

