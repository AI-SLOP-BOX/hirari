int main() {
    {
        auto& driver = Aura::Core::External::AudioDriverPro::getInstance();
        driver.closeStream();
        bool callbackOpened = false;
        bool callbackClosed = false;
        uint32_t callbackFrames = 0;
        driver.bindExternalBackend(
            Aura::Core::External::AudioDriverPro::Backend::WASAPI,
            [&](double rate, int frames) {
                callbackOpened = rate == 48'000.0 && frames == 256;
                return callbackOpened;
            },
            [&] { callbackClosed = true; },
            [&](float* const* outputs, float* const*, uint32_t frames) {
                callbackFrames = frames;
                if (outputs && outputs[0]) outputs[0][0] = 0.25f;
            });
        if (!driver.openStream(Aura::Core::External::AudioDriverPro::Backend::WASAPI,
                               0, 48'000.0, 256) || !callbackOpened) return 83;
        float output[2][4]{};
        float input[2][4]{};
        float* outputs[] = {output[0], output[1]};
        float* inputs[] = {input[0], input[1]};
        if (!driver.processBlock(outputs, inputs, 4) || callbackFrames != 4 || output[0][0] != 0.25f)
            return 82;
        driver.closeStream();
        if (!callbackClosed) return 80;
        driver.bindExternalBackend(
            Aura::Core::External::AudioDriverPro::Backend::WASAPI,
            [](double, int) { return false; }, [] {});

        if (driver.openStream(Aura::Core::External::AudioDriverPro::Backend::WASAPI,
                              0, 48'000.0, 256)) return 84;
        if (driver.isOpen() || driver.lastError().empty()) return 85;
#if !defined(AURA_ENABLE_ASIO_SDK)
        if (driver.openStream(Aura::Core::External::AudioDriverPro::Backend::ASIO,
                              0, 48'000.0, 256)) return 86;
        if (driver.isOpen() || driver.lastError().find("ASIO SDK") == std::string::npos)
            return 87;
#endif
        auto& jack = Aura::Core::External::JackBridgeDeep::getInstance();
        jack.shutdown();
#if !defined(AURA_ENABLE_JACK)
        if (jack.tryInitialize("aura-contract") || jack.isRunning() ||
            jack.lastError().find("AURA_ENABLE_JACK") == std::string::npos) return 88;
#endif
    }
    const std::string xmlProbe = "C:\\Audio & Mix/<take>\"lead\"'";
    if (Aura::Core::IO::SessionIO::unescapeXmlAttribute(
            Aura::Core::IO::SessionIO::escapeXmlAttribute(xmlProbe)) != xmlProbe) return 81;
    const auto sessionProbe = std::filesystem::temp_directory_path() / "aura-session-atomic-contract.xml";
    Aura::Core::IO::SessionIO::getInstance().saveProject(sessionProbe.string(), {});
    Aura::Core::IO::SessionIO::getInstance().saveProject(
        sessionProbe.string(), {std::shared_ptr<Aura::Core::Engine::Track>{}});
    std::ifstream sessionInput(sessionProbe, std::ios::binary);
    const std::string sessionXml((std::istreambuf_iterator<char>(sessionInput)), {});
    std::error_code sessionCleanup;
    std::filesystem::remove(sessionProbe, sessionCleanup);
    if (sessionXml.find("<AuraProject") == std::string::npos ||
        sessionXml.find("</AuraProject>") == std::string::npos) return 82;
    for (const auto& entry : std::filesystem::directory_iterator(sessionProbe.parent_path())) {
        const auto name = entry.path().filename().string();
        if (name.rfind("aura-session-atomic-contract.xml.tmp-", 0) == 0) return 83;
    }
    {
        SDKContractFactory factory;
        const auto descriptor = factory.getDescriptor();
        if (descriptor.sdkVersion != Aura::SDK::kSDKVersion ||
            descriptor.identifier != "com.aura.sdk.contract" ||
            descriptor.parameters.size() != 1 || !descriptor.supportsSidechain ||
            descriptor.stateSchemaVersion == 0 || descriptor.guiStateSchemaVersion == 0 ||
            !descriptor.isValid()) return 65;
        auto invalid = descriptor;
        invalid.parameters[0].defaultValue = 2.0f;
        if (invalid.isValid()) return 69;
        const auto cache_key = descriptor.stateCacheKey();
        if (cache_key.find("com.aura.sdk.contract@1.0.0") == std::string::npos) return 70;
        if (Aura::Core::Plugins::PluginCacheManager::stateCacheKey(descriptor) != cache_key) return 72;
        invalid = descriptor;
        invalid.stateSchemaVersion++;
        if (invalid.stateCacheKey() == cache_key) return 71;
        invalid = descriptor;
        invalid.parameters.push_back({1, "Duplicate", 0.0f, 1.0f, 0.0f, true});
        if (invalid.isValid()) return 73;
        invalid = descriptor;
        invalid.parameters[0].defaultValue = std::numeric_limits<float>::quiet_NaN();
        if (invalid.isValid()) return 74;
        invalid = descriptor;
        invalid.outputChannels = 33;
        if (invalid.isValid()) return 75;
        invalid = descriptor;
        invalid.supportsSidechain = true;
        invalid.sidechainChannels = 0;
        if (invalid.isValid()) return 76;
        invalid = descriptor;
        invalid.components.push_back({"main", 1, 1, false});
        if (invalid.isValid()) return 79;
        invalid = descriptor;
        invalid.components.push_back({"other", 1, 1, true});
        invalid.supportsSidechain = false;
        if (invalid.isValid()) return 80;
        Aura::SDK::ParameterAutomation automation;
        if (!automation.setPoints({{100, 1.0f}, {0, 0.0f}, {200, 0.5f}}) ||
            automation.valueAt(50, -1.0f) != 0.5f ||
            automation.valueAt(250, -1.0f) != 0.5f) return 77;
        if (automation.setPoints({{0, 0.0f}, {0, 1.0f}}) ||
            automation.setPoints({{0, std::numeric_limits<float>::quiet_NaN()}})) return 78;
        auto processor = factory.create();
        if (!processor || processor->getName() != "SDK Contract") return 66;
        if (!processor->loadState(processor->saveState()) ||
            !processor->loadGuiState(processor->saveGuiState())) return 67;
        factory.destroy(std::move(processor));
    }
    {
        auto& legacySidechain = Aura::Core::DSP::Mixing::SidechainManager::getInstance();
        const float source[4] = {1.0f, 2.0f, 3.0f, 4.0f};
        if (!legacySidechain.writeSource(7, source, 4)) return 63;
        float copy[4] = {};
        const size_t frames = legacySidechain.copySource(7, copy, 4);
        if (frames != 4 || copy[0] != 1.0f || copy[3] != 4.0f) return 64;
    }
    // This translation unit intentionally exercises the production native
    // plugin boundary without loading a third-party binary.
    auto& pool = Aura::Core::Concurrency::ThreadPool::getInstance();
    auto poolResult = pool.enqueue([] { return 42; });
    if (poolResult.get() != 42) return 47;
    const auto peakWav = std::filesystem::temp_directory_path() / "aura-audio-pool-peaks.wav";
    {
        std::vector<uint8_t> wav;
        const auto u16 = [&wav](uint16_t value) {
            wav.push_back(static_cast<uint8_t>(value));
            wav.push_back(static_cast<uint8_t>(value >> 8));
        };
        const auto u32 = [&wav](uint32_t value) {
            for (unsigned shift = 0; shift < 32; shift += 8)
                wav.push_back(static_cast<uint8_t>(value >> shift));
        };
        const uint32_t dataBytes = 4u * sizeof(int16_t);
        wav.insert(wav.end(), {'R','I','F','F'});
        u32(36u + dataBytes);
        wav.insert(wav.end(), {'W','A','V','E','f','m','t',' '});
        u32(16); u16(1); u16(1); u32(48'000); u32(96'000); u16(2); u16(16);
        wav.insert(wav.end(), {'d','a','t','a'}); u32(dataBytes);
        for (int16_t sample : {1000, 2000, 1500, 2500}) u16(static_cast<uint16_t>(sample));
        std::ofstream output(peakWav, std::ios::binary | std::ios::trunc);
        output.write(reinterpret_cast<const char*>(wav.data()), static_cast<std::streamsize>(wav.size()));
    }
    auto& audioPool = Aura::Core::AudioPool::getInstance();
    if (audioPool.addSource((peakWav.string() + ".missing")) != nullptr) return 50;
    const auto peakSource = audioPool.addSource(peakWav.string());
    if (!peakSource) return 48;
    const auto peakDeadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    std::shared_ptr<Aura::Core::PeakData> peaks;
    while (std::chrono::steady_clock::now() < peakDeadline) {
        peaks = audioPool.getPeaks(peakWav.string());
        if (peaks && peaks->isReady.load(std::memory_order_acquire)) break;
        std::this_thread::yield();
    }
    if (!peaks || !peaks->isReady.load(std::memory_order_acquire) || peaks->levels.empty() ||
        peaks->levels.front().mins.front() <= 0.0f || peaks->levels.front().maxs.front() <= 0.0f) return 49;
    audioPool.purgeUnused();
    std::error_code peakCleanup;
    std::filesystem::remove(peakWav, peakCleanup);
    // These hosts embed fixed-size realtime mailboxes. Keep them off the
    // test's main stack so the contract remains runnable on macOS's default
    // 8 MiB thread stack even when the sandbox protocol grows.
    auto vst3 = std::make_unique<Aura::Core::Plugins::VST3HostProcessor>();
    auto clap = std::make_unique<Aura::Core::Plugins::CLAPHostProcessor>();
    // The built-in compressor is part of the production plugin contract, not
    // merely a header-only type. Exercise prepare/process/parameter/state so
    // a future refactor cannot leave the browser-visible plugin unlinked.
    {
        Aura::DSP::Effects::DynamicCompressor compressor;
        compressor.prepareToPlay(48'000.0, 128);
        compressor.setParameter(0, 0.7f);
        compressor.setParameter(1, 0.25f);
        Aura::Core::AudioBuffer block(2, 128);
        for (uint32_t i = 0; i < block.getNumSamples(); ++i) {
            block.getWritePointer(0)[i] = 0.9f;
            block.getWritePointer(1)[i] = -0.9f;
        }
        Aura::Core::MidiBuffer midi;
        Aura::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 128;
        compressor.process(block, midi, context);
        if (block.sanitizeNonFinite() != 0 || compressor.getNumParameters() != 7) return 67;
        const auto state = compressor.getState();
        Aura::DSP::Effects::DynamicCompressor restored;
        if (!restored.restoreStateChecked(state) ||
            restored.getParameter(0) != compressor.getParameter(0)) return 68;
    }
    // Procedural Foley must be deterministic for offline renders and must
    // fail closed when a UI/FFI caller supplies non-finite event controls.
    {
        Aura::Core::DSP::Effects::ProceduralFoleyKernel first;
        Aura::Core::DSP::Effects::ProceduralFoleyKernel second;
        first.triggerEvent(std::numeric_limits<float>::quiet_NaN(), 0.65f);
        second.triggerEvent(0.5f, 0.65f);
        std::array<float, 512> firstBlock{};
        std::array<float, 512> secondBlock{};
        first.process(firstBlock.data(), static_cast<uint32_t>(firstBlock.size()));
        second.process(secondBlock.data(), static_cast<uint32_t>(secondBlock.size()));
        for (size_t sample = 0; sample < firstBlock.size(); ++sample) {
            if (!std::isfinite(firstBlock[sample]) || !std::isfinite(secondBlock[sample]) ||
                std::abs(firstBlock[sample] - secondBlock[sample]) > 1.0e-6f) return 76;
        }
    }
    {
        auto& assetLibrary = Aura::IO::Assets::AudioAssetLibrary::getInstance();
        assetLibrary.registerPatch("Contract Bass", "Bass", "/tmp/aura-contract-bass.aura");
        assetLibrary.registerPatch("Contract Drums", "Drums", "/tmp/aura-contract-drums.aura");
        assetLibrary.registerPatch("Contract Keys", "Keys", "/tmp/aura-contract-keys.aura");
        const auto categories = assetLibrary.categories();
        if (categories.size() < 3 || !std::is_sorted(categories.begin(), categories.end()) ||
            std::find(categories.begin(), categories.end(), "Bass") == categories.end() ||
            std::find(categories.begin(), categories.end(), "Drums") == categories.end() ||
            std::find(categories.begin(), categories.end(), "Keys") == categories.end()) return 77;
    }
    // The sampler must consume both legacy MIDI and MIDI 2.0 UMP note events,
    // including sustain pedal state, through the same realtime dispatcher.
    {
        static std::array<float, 4096> sample{};
        sample.fill(0.25f);
        auto sampler = std::make_unique<Aura::DSP::Synthesis::SamplerEngine>(48'000.0);
        sampler->addZone({60, 0, 127, 1, 127, {sample.data(), sample.size()}});
        auto block = std::make_unique<Aura::Core::AudioBuffer>(2, 32);
        auto midi = std::make_unique<Aura::Core::MidiBuffer>();
        const uint8_t umpOn[] = {0x40, 0x90, 60, 0, 0x7f, 0, 0, 0,
                                 0, 0, 0, 0, 0, 0, 0, 0};
        midi->addEvent(0, umpOn, sizeof(umpOn));
        Aura::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 32;
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 69;
        const uint8_t sustainDown[] = {0xB0, 64, 127};
        const uint8_t noteOff[] = {0x80, 60, 0};
        midi->clear();
        midi->addEvent(0, sustainDown, sizeof(sustainDown));
        midi->addEvent(1, noteOff, sizeof(noteOff));
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 70;
        const uint8_t sustainUp[] = {0xB0, 64, 0};
        midi->clear();
        midi->addEvent(0, sustainUp, sizeof(sustainUp));
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 71;
    }
    // A looped zone must continue producing audio after the source reaches
    // its end, while keeping the output finite.
    {
        static std::array<float, 8> loopSample{};
        for (size_t i = 0; i < loopSample.size(); ++i) loopSample[i] = 0.1f + 0.1f * static_cast<float>(i);
        auto sampler = std::make_unique<Aura::DSP::Synthesis::SamplerEngine>(48'000.0);
        sampler->addZone({60, 60, 60, 1, 127, {loopSample.data(), loopSample.size()}, 2, 6, true});
        auto block = std::make_unique<Aura::Core::AudioBuffer>(1, 32);
        auto midi = std::make_unique<Aura::Core::MidiBuffer>();
        const uint8_t noteOn[] = {0x90, 60, 127};
        midi->addEvent(0, noteOn, sizeof(noteOn));
        Aura::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 32;
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 74;
        bool hasTail = false;
        for (uint32_t i = 8; i < block->getNumSamples(); ++i)
            hasTail = hasTail || std::abs(block->getReadPointer(0)[i]) > 1.0e-5f;
        if (!hasTail) return 75;
    }
    {
        Aura::Synthesis::SamplerMap map;
        map.addZone(-1, 60, 1, 127, "invalid.wav");
        map.addZone(0, 127, 0, 127, "layer-a.wav");
        map.addZone(0, 127, 0, 127, "layer-b.wav");
        if (map.resolveSample(60, 100).empty() || map.resolveSample(60, 100).empty()) return 72;
        Aura::Synthesis::SamplerMap independent;
        independent.addZone(0, 127, 0, 127, "layer-a.wav");
        if (independent.resolveSample(60, 100) != "layer-a.wav") return 73;
    }
    Aura::Core::PluginHost::ClapHostInterface legacyClap("builtin://compile-contract");
    Aura::Core::Engine::MIDIOrchestrator midiOrchestrator;
    // The sandbox host owns bounded shared-memory staging arrays sized for
    // the maximum plugin protocol block. Allocate it on the heap to keep
    // this broad native contract below the platform thread-stack limit.
    auto sandbox = std::make_unique<Aura::Core::Plugins::PluginSandboxHost>("builtin://compile-contract");
    Aura::Core::EffectChain chain;
    Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode recovery =
        sandbox->isAlive()
            ? Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::ClearBlock
            : Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::Quarantined;
    static_assert(Aura::Core::Plugins::SandboxProtocol::kStatusHeaderV9 == 0x41555209u);
    static_assert(Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion == 1u);
    static_assert(static_cast<unsigned>(Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::ClearBlock) == 0u);
    static_assert(static_cast<unsigned>(Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::Quarantined) == 1u);
    if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(
            Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion) ||
        Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(0u) ||
        Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(
            Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion + 1u)) return 64;
    (void)vst3;
    (void)clap;
    (void)legacyClap;
    (void)midiOrchestrator;
    (void)sandbox;
    (void)chain;
    (void)recovery;

    // The legacy structural wrapper must remain ABI-compatible with the
    // current processor interface while refusing to masquerade as a loaded
    // third-party plugin. Real processing is covered by the format adapters.
    Aura::Core::PluginHost::ExternalPluginWrapper legacyExternal(
        Aura::Core::PluginHost::ExternalPluginWrapper::Format::VST3,
        "builtin://unsupported-compat-wrapper");
    if (legacyExternal.supportsProcessing()) return 65;

    auto& undo = Aura::Core::Undo::UndoManager::getInstance();
    undo.clear();
    int transactionValue = 0;
    undo.beginTransaction("partial-operation");
    undo.perform(std::make_unique<UndoContractCommand>(transactionValue, 2, "first"));
    undo.perform(std::make_unique<UndoContractCommand>(transactionValue, 3, "second"));
    if (transactionValue != 5 || !undo.transactionActive()) return 16;
    undo.abortTransaction();
    if (transactionValue != 0 || undo.transactionActive() || undo.getUndoCount() != 0) return 17;

    auto& engineUndo = Aura::Core::Engine::UndoTransactionManager::getInstance();
    engineUndo.clear();

    Aura::Core::AudioBuffer finiteContract(1, 3);
    finiteContract.getWritePointer(0)[0] = 1.0f;
    finiteContract.getWritePointer(0)[1] = std::numeric_limits<float>::quiet_NaN();
    finiteContract.getWritePointer(0)[2] = std::numeric_limits<float>::infinity();
    if (finiteContract.sanitizeNonFinite() != 2 ||
        !std::isfinite(finiteContract.getReadPointer(0)[1]) ||
        !std::isfinite(finiteContract.getReadPointer(0)[2])) return 24;

    // The graph must contain third-party faults: non-finite samples are
    // sanitized without allocation and remain finite on subsequent blocks.
    Aura::Core::AudioProcessorGraph graphContract;
    graphContract.addNode(std::make_shared<NonFiniteProcessor>());
    graphContract.prepare(48'000.0, 8);
    Aura::Core::AudioBuffer graphBuffer(1, 8);
    Aura::Core::MidiBuffer graphMidi;
    Aura::DSP::ProcessContext graphContext{};
    graphContext.sampleRate = 48'000.0;
    graphContext.blockSize = 8;
    graphContext.numOutputChannels = 1;
    graphContract.process(graphBuffer, graphMidi, graphContext);
    if (graphContract.getSanitizedSampleCount() != 8 ||
        graphContract.getProcessorFaultCount() != 0 ||
        graphContract.getFaultedNodeCount() != 0) return 31;
    for (uint32_t index = 0; index < graphBuffer.getNumSamples(); ++index) {
        if (!std::isfinite(graphBuffer.getReadPointer(0)[index])) return 32;
    }
    graphContract.process(graphBuffer, graphMidi, graphContext);
    if (graphContract.getSanitizedSampleCount() != 16 ||
        graphContract.getProcessorFaultCount() != 0 ||
        graphContract.getFaultedNodeCount() != 0) return 33;
    int engineTransactionValue = 0;
    engineUndo.beginTransaction("engine-partial-operation");
    engineUndo.performAction("first", [&] { engineTransactionValue -= 2; },
                             [&] { engineTransactionValue += 2; });
    engineUndo.performAction("second", [&] { engineTransactionValue -= 3; },
                             [&] { engineTransactionValue += 3; });
    if (engineTransactionValue != 5 || !engineUndo.transactionActive()) return 18;
    if (!engineUndo.abortTransaction() || engineTransactionValue != 0 ||
        engineUndo.transactionActive() || engineUndo.getUndoCount() != 0) return 19;
    engineUndo.beginTransaction("engine-commit");
    engineUndo.performAction("first", [&] { engineTransactionValue -= 2; },
                             [&] { engineTransactionValue += 2; });
    engineUndo.performAction("second", [&] { engineTransactionValue -= 3; },
                             [&] { engineTransactionValue += 3; });
    if (!engineUndo.endTransaction() || engineUndo.getUndoCount() != 1) return 20;
    engineUndo.undo();
    if (engineTransactionValue != 0 || engineUndo.getRedoCount() != 1) return 21;
    engineUndo.redo();
    if (engineTransactionValue != 5) return 22;
    engineUndo.clear();

    const auto autosavePath = std::filesystem::temp_directory_path() /
        ("aura-autosave-contract-" + std::to_string(
#if !defined(_WIN32)
            static_cast<unsigned long long>(::getpid())
#else
            0ULL
#endif
        ) + ".json");
    auto& autosave = Aura::IO::Persistence::AutoSaveEngine::getInstance();
    autosave.start(autosavePath.string(), 60, [] { return std::string("{\"ok\":true}"); });
    autosave.markModified();
    if (!autosave.flushNow() || autosave.isDirty()) return 23;
    autosave.stop();
    std::error_code autosaveCleanup;
    std::filesystem::remove(autosavePath.string() + ".autosave", autosaveCleanup);

    // A failed DB publication must not remove the last destination. Using a
    // directory as the destination forces rename to fail while making the
    // preservation invariant directly observable.
    auto& projectDb = Aura::Core::Database::ProjectDB::i();
    projectDb.recordForensic("contract", "publication", "preserve-last-good");
    const auto dbDestination = std::filesystem::temp_directory_path() / "aura-project-db-contract-destination";
    std::error_code dbCleanup;
    std::filesystem::remove_all(dbDestination, dbCleanup);
    std::filesystem::create_directory(dbDestination, dbCleanup);
    if (dbCleanup || projectDb.flushToDisk(dbDestination.string()) ||
        !std::filesystem::is_directory(dbDestination)) return 65;
    std::filesystem::remove_all(dbDestination, dbCleanup);

    // A newer asset batch must prevent an older worker from publishing stale
    // results or failure diagnostics into the current project.
    auto& assets = Aura::IO::ParallelAssetManager::getInstance();
    assets.loadAssets({"/definitely/missing/aura-old.wav"});
    if (assets.failedPaths().empty()) return 25;

    // Native project collection must preserve distinct same-basename assets
    // instead of silently reusing the first destination.
    const auto collectorRoot = std::filesystem::temp_directory_path() /
                               "aura-project-collector-contract";
    std::error_code collectorCleanup;
    std::filesystem::remove_all(collectorRoot, collectorCleanup);
    std::filesystem::create_directories(collectorRoot / "one", collectorCleanup);
    std::filesystem::create_directories(collectorRoot / "two", collectorCleanup);
    if (collectorCleanup) return 66;
    const auto collectorFirst = collectorRoot / "one" / "same.wav";
    const auto collectorSecond = collectorRoot / "two" / "same.wav";
    std::ofstream(collectorFirst) << "first";
    std::ofstream(collectorSecond) << "second";
    auto& collector = Aura::IO::Persistence::ProjectCollector::getInstance();
    if (!collector.collect(collectorRoot.string(),
                           {collectorFirst.string(), collectorSecond.string()})) return 67;
    size_t collectedCount = 0;
    for (const auto& entry : std::filesystem::directory_iterator(collectorRoot / "Assets")) {
        if (entry.is_regular_file()) ++collectedCount;
    }
    std::filesystem::remove_all(collectorRoot, collectorCleanup);
    if (collectedCount != 2) return 68;

    auto& telemetryBridge = Aura::Network::SharedMemoryBridge::getInstance();
    if (!telemetryBridge.start()) return 26;
    const std::string telemetryName = telemetryBridge.segmentName();
    if (telemetryName.find("/aura_sovereign_bridge_") != 0) return 27;
    std::array<uint8_t, 300> sharedMidiPayload{};
    sharedMidiPayload.front() = 0xf0;
    sharedMidiPayload.back() = 0xf7;
    if (!telemetryBridge.pushExtendedMidi(24, 3, sharedMidiPayload.data(), sharedMidiPayload.size()) ||
        telemetryBridge.pendingExtendedMidi() != 1) return 59;
    Aura::Core::Plugins::MidiExtendedMessageRing::Message sharedMidiMessage;
    if (!telemetryBridge.popExtendedMidi(sharedMidiMessage) ||
        sharedMidiMessage.size != sharedMidiPayload.size() ||
        sharedMidiMessage.sampleOffset != 24 || sharedMidiMessage.articulationId != 3 ||
        sharedMidiMessage.data.front() != 0xf0 || sharedMidiMessage.data[299] != 0xf7) return 60;
    for (unsigned slot = 0; slot < Aura::Core::Plugins::MidiExtendedMessageRing::kCapacity; ++slot) {
        if (!telemetryBridge.pushExtendedMidi(slot, 0, sharedMidiPayload.data(), sharedMidiPayload.size())) return 61;
    }
    if (telemetryBridge.pushExtendedMidi(99, 0, sharedMidiPayload.data(), sharedMidiPayload.size()) ||
        telemetryBridge.pendingExtendedMidi() != Aura::Core::Plugins::MidiExtendedMessageRing::kCapacity) return 62;
    Aura::Core::Plugins::MidiExtendedMessageRing::Message drainedMessage;
    while (telemetryBridge.popExtendedMidi(drainedMessage)) {}
    if (telemetryBridge.pendingExtendedMidi() != 0) return 63;
    telemetryBridge.stop();
    if (!telemetryBridge.segmentName().empty()) return 28;
    if (!telemetryBridge.start()) return 45;
    const std::string secondTelemetryName = telemetryBridge.segmentName();
    if (secondTelemetryName.empty() || secondTelemetryName == telemetryName) return 46;
    telemetryBridge.stop();

    // Payloads beyond the bounded SysEx/MIDI2 envelope must be observable at
    // ingress rather than silently becoming a truncated channel-voice event.
    Aura::Core::MidiBuffer midiIngress;
    const uint8_t oversizedMidi[257] = {};
    midiIngress.addEvent(0, oversizedMidi, sizeof(oversizedMidi));
    if (!midiIngress.overflowed() || midiIngress.takeOversizeEvents() != 1 ||
        midiIngress.takeExtendedEvents() != 1 || midiIngress.size() != 0) return 15;

    const auto temp = std::filesystem::temp_directory_path() / "aura-plugin-cache-contract.bin";
    {
        std::ofstream output(temp, std::ios::binary | std::ios::trunc);
        output << "version-one";
    }
    const auto first = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(temp);
    if (!first || *first == 0) return 2;
    {
        std::ofstream output(temp, std::ios::binary | std::ios::trunc);
        output << "version-two-with-a-different-size";
    }
    const auto second = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(temp);
    std::error_code cleanup;
    std::filesystem::remove(temp, cleanup);
    if (!second || *second == *first) return 3;
    const auto directoryFingerprint = std::filesystem::temp_directory_path() / "aura-plugin-fingerprint-directory";
    std::filesystem::remove_all(directoryFingerprint, cleanup);
    std::filesystem::create_directories(directoryFingerprint / "nested", cleanup);
    {
        std::ofstream(directoryFingerprint / "z.bin", std::ios::binary) << "z";
        std::ofstream(directoryFingerprint / "nested" / "a.bin", std::ios::binary) << "a";
    }
    const auto directoryFirst = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(directoryFingerprint);
    std::filesystem::remove_all(directoryFingerprint, cleanup);
    std::filesystem::create_directories(directoryFingerprint / "nested", cleanup);
    {
        // Create in the opposite order: the fingerprint must not depend on
        // filesystem iterator ordering.
        std::ofstream(directoryFingerprint / "nested" / "a.bin", std::ios::binary) << "a";
        std::ofstream(directoryFingerprint / "z.bin", std::ios::binary) << "z";
    }
    const auto directorySecond = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(directoryFingerprint);
    std::filesystem::remove_all(directoryFingerprint, cleanup);
    if (!directoryFirst || !directorySecond || *directoryFirst != *directorySecond) return 4;
    const auto admittedPath = std::filesystem::temp_directory_path() / "aura-admission-contract.clap";
    {
        std::ofstream output(admittedPath, std::ios::binary | std::ios::trunc);
        output << "fixture-v1";
    }
    Aura::Core::Plugins::PluginDescriptor admitted;
    admitted.uuid = "contract-admission-plugin";
    admitted.name = "Admission Contract";
    admitted.format = "CLAP";
    admitted.binaryPath = admittedPath.string();
    auto& infrastructure = Aura::Core::Plugins::PluginHostInfrastructure::getInstance();
    if (!infrastructure.registerPlugin(admitted)) return 28;
    std::string admissionError;
    admissionError = "stale diagnostic";
    if (!infrastructure.validatePlugin(admitted.uuid, &admissionError)) return 29;
    if (!admissionError.empty()) return 33;
    {
        std::ofstream output(admittedPath, std::ios::binary | std::ios::trunc);
        output << "fixture-v2-with-a-new-binary-fingerprint";
    }
    if (infrastructure.validatePlugin(admitted.uuid, &admissionError) ||
        admissionError.find("rescan required") == std::string::npos) return 30;
    if (infrastructure.createPlugin(admitted.uuid, &admissionError)) return 36;
    if (admissionError.find("rescan required") == std::string::npos) return 37;
    const auto collisionPath = std::filesystem::temp_directory_path() /
                               "aura-admission-collision.clap";
    {
        std::ofstream output(collisionPath, std::ios::binary | std::ios::trunc);
        output << "collision";
    }
    Aura::Core::Plugins::PluginDescriptor collision = admitted;
    collision.binaryPath = collisionPath.string();
    collision.binaryFingerprint = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(
        collisionPath).value_or(0);
    if (infrastructure.registerPlugin(collision)) return 35;
    std::filesystem::remove(collisionPath, cleanup);
    std::filesystem::remove(admittedPath, cleanup);
#if !defined(_WIN32)
    const auto symlinkPath = std::filesystem::temp_directory_path() /
                             "aura-admission-contract-symlink.clap";
    const auto symlinkTarget = std::filesystem::temp_directory_path() /
                               "aura-admission-contract-target.clap";
    {
        std::ofstream output(symlinkTarget, std::ios::binary | std::ios::trunc);
        output << "target";
    }
    std::filesystem::create_symlink(symlinkTarget, symlinkPath, cleanup);
    if (Aura::Core::Plugins::PluginAdmission::isSafeCandidate(symlinkPath, "CLAP")) return 31;
    std::filesystem::remove(symlinkPath, cleanup);
    std::filesystem::remove(symlinkTarget, cleanup);
#endif
