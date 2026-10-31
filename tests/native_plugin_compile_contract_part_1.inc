int main() {
    {
        auto& jack = Hirari::Core::External::JackBridgeDeep::getInstance();
        jack.shutdown();
#if !defined(HIRARI_ENABLE_JACK)
        if (jack.tryInitialize("hirari-contract") || jack.isRunning() ||
            jack.lastError().find("HIRARI_ENABLE_JACK") == std::string::npos) return 88;
#endif
    }
    {
        SDKContractFactory factory;
        const auto descriptor = factory.getDescriptor();
        if (descriptor.sdkVersion != Hirari::SDK::kSDKVersion ||
            descriptor.identifier != "com.hirari.sdk.contract" ||
            descriptor.parameters.size() != 1 || !descriptor.supportsSidechain ||
            descriptor.stateSchemaVersion == 0 || descriptor.guiStateSchemaVersion == 0 ||
            !descriptor.isValid()) return 65;
        auto invalid = descriptor;
        invalid.parameters[0].defaultValue = 2.0f;
        if (invalid.isValid()) return 69;
        const auto cache_key = descriptor.stateCacheKey();
        if (cache_key.find("com.hirari.sdk.contract@1.0.0") == std::string::npos) return 70;
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
        Hirari::SDK::ParameterAutomation automation;
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
    // This translation unit intentionally exercises the production native
    // plugin boundary without loading a third-party binary.
    auto& pool = Hirari::Core::Concurrency::ThreadPool::getInstance();
    auto poolResult = pool.enqueue([] { return 42; });
    if (poolResult.get() != 42) return 47;
    // The built-in compressor is part of the production plugin contract, not
    // merely a header-only type. Exercise prepare/process/parameter/state so
    // a future refactor cannot leave the browser-visible plugin unlinked.
    {
        const std::string compressorPath = "Hirari/Compressor";
        const int32_t compressorKind = hirari_native_processor_kind_for_path(
            reinterpret_cast<const uint8_t*>(compressorPath.data()), compressorPath.size());
        if (compressorKind < 0) return 69;
        Hirari::Core::Plugin::NativeProcessor compressor(compressorKind);
        compressor.prepareToPlay(48'000.0, 128);
        compressor.setParameter(0, 0.7f);
        compressor.setParameter(1, 0.25f);
        Hirari::Core::AudioBuffer block(2, 128);
        for (uint32_t i = 0; i < block.getNumSamples(); ++i) {
            block.getWritePointer(0)[i] = 0.9f;
            block.getWritePointer(1)[i] = -0.9f;
        }
        Hirari::Core::MidiBuffer midi;
        Hirari::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 128;
        compressor.process(block, midi, context);
        if (block.sanitizeNonFinite() != 0 || compressor.getNumParameters() != 7) return 67;
        const auto state = compressor.getState();
        Hirari::Core::Plugin::NativeProcessor restored(compressorKind);
        if (!restored.restoreStateChecked(state) ||
            restored.getParameter(0) != compressor.getParameter(0)) return 68;
    }
    // The sandbox host owns bounded shared-memory staging arrays sized for
    // the maximum plugin protocol block. Allocate it on the heap to keep
    // this broad native contract below the platform thread-stack limit.
    auto sandbox = std::make_unique<Hirari::Core::Plugins::PluginSandboxHost>("builtin://compile-contract");
    Hirari::Core::EffectChain chain;
    Hirari::Core::Plugins::ProcessSandboxProcessor::RecoveryMode recovery =
        sandbox->isAlive()
            ? Hirari::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::ClearBlock
            : Hirari::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::Quarantined;
    static_assert(Hirari::Core::Plugins::SandboxProtocol::kStatusHeaderV9 == 0x41555209u);
    static_assert(Hirari::Core::Plugins::SandboxProtocol::kStateProtocolVersion == 1u);
    static_assert(static_cast<unsigned>(Hirari::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::ClearBlock) == 0u);
    static_assert(static_cast<unsigned>(Hirari::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::Quarantined) == 1u);
    if (!Hirari::Core::Plugins::SandboxProtocol::isSupportedStateVersion(
            Hirari::Core::Plugins::SandboxProtocol::kStateProtocolVersion) ||
        Hirari::Core::Plugins::SandboxProtocol::isSupportedStateVersion(0u) ||
        Hirari::Core::Plugins::SandboxProtocol::isSupportedStateVersion(
            Hirari::Core::Plugins::SandboxProtocol::kStateProtocolVersion + 1u)) return 64;
    (void)sandbox;
    (void)chain;
    (void)recovery;


    // Transactional undo is Rust-owned (hirari-core-bridge/src/undo_transaction.rs).
    // The contract drives the raw FFI so native callbacks and abort rollback
    // stay covered without any C++ history wrapper.
    void* engineUndo = hirari_undo_manager_create(128, 300);
    if (!engineUndo) return 18;
    int engineTransactionValue = 0;
    struct UndoDelta {
        int* value;
        int delta;
    };
    const auto registerDelta = [](void* state, int* value, int delta) -> uint64_t {
        auto* context = new UndoDelta{value, delta};
        const uint64_t id = hirari_undo_manager_register_native_callback(
            state, context,
            [](void* raw) { auto* d = static_cast<UndoDelta*>(raw); *d->value += d->delta; },
            [](void* raw) { delete static_cast<UndoDelta*>(raw); });
        if (!id) delete context;
        return id;
    };
    const auto perform = [&](const char* name, int delta) {
        const std::string label(name);
        return hirari_undo_manager_commit(
            engineUndo, reinterpret_cast<const uint8_t*>(label.data()), label.size(),
            registerDelta(engineUndo, &engineTransactionValue, -delta),
            registerDelta(engineUndo, &engineTransactionValue, delta), true, true);
    };

    Hirari::Core::AudioBuffer finiteContract(1, 3);
    finiteContract.getWritePointer(0)[0] = 1.0f;
    finiteContract.getWritePointer(0)[1] = std::numeric_limits<float>::quiet_NaN();
    finiteContract.getWritePointer(0)[2] = std::numeric_limits<float>::infinity();
    if (finiteContract.sanitizeNonFinite() != 2 ||
        !std::isfinite(finiteContract.getReadPointer(0)[1]) ||
        !std::isfinite(finiteContract.getReadPointer(0)[2])) return 24;

    const char* partial = "engine-partial-operation";
    (void)hirari_undo_manager_begin_transaction(
        engineUndo, reinterpret_cast<const uint8_t*>(partial), std::string(partial).size());
    (void)perform("first", 2);
    (void)perform("second", 3);
    if (engineTransactionValue != 5 || !hirari_undo_manager_transaction_active(engineUndo))
        return 18;
    if (!hirari_undo_manager_abort_and_invoke(engineUndo) || engineTransactionValue != 0 ||
        hirari_undo_manager_transaction_active(engineUndo) ||
        hirari_undo_manager_undo_count(engineUndo) != 0) return 19;
    const char* commit = "engine-commit";
    (void)hirari_undo_manager_begin_transaction(
        engineUndo, reinterpret_cast<const uint8_t*>(commit), std::string(commit).size());
    (void)perform("first", 2);
    (void)perform("second", 3);
    if (!hirari_undo_manager_end_transaction(engineUndo) ||
        hirari_undo_manager_undo_count(engineUndo) != 1) return 20;
    (void)hirari_undo_manager_apply(engineUndo, false);
    if (engineTransactionValue != 0 || hirari_undo_manager_redo_count(engineUndo) != 1)
        return 21;
    (void)hirari_undo_manager_apply(engineUndo, true);
    if (engineTransactionValue != 5) return 22;
    hirari_undo_manager_clear(engineUndo);
    hirari_undo_manager_destroy(engineUndo);

    // A failed DB publication must not remove the last destination. Using a
    // directory as the destination forces rename to fail while making the
    // preservation invariant directly observable.
    void* projectDb = hirari_project_db_create();
    const auto* dbCategory = reinterpret_cast<const uint8_t*>("contract");
    const auto* dbKey = reinterpret_cast<const uint8_t*>("publication");
    const auto* dbValue = reinterpret_cast<const uint8_t*>("preserve-last-good");
    (void)hirari_project_db_record(
        projectDb, static_cast<uint64_t>(std::chrono::system_clock::now().time_since_epoch().count()),
        dbCategory, 8, dbKey, 11, dbValue, 18);
    const auto dbDestination = std::filesystem::temp_directory_path() / "hirari-project-db-contract-destination";
    std::error_code dbCleanup;
    std::filesystem::remove_all(dbDestination, dbCleanup);
    std::filesystem::create_directory(dbDestination, dbCleanup);
    const std::string dbDestinationText = dbDestination.string();
    const bool dbFlushed = hirari_project_db_flush(
        projectDb, reinterpret_cast<const uint8_t*>(dbDestinationText.data()),
        dbDestinationText.size());
    hirari_project_db_destroy(projectDb);
    if (dbCleanup || dbFlushed ||
        !std::filesystem::is_directory(dbDestination)) return 65;
    std::filesystem::remove_all(dbDestination, dbCleanup);



    // Payloads beyond the bounded SysEx/MIDI2 envelope must be observable at
    // ingress rather than silently becoming a truncated channel-voice event.
    Hirari::Core::MidiBuffer midiIngress;
    const uint8_t oversizedMidi[257] = {};
    midiIngress.addEvent(0, oversizedMidi, sizeof(oversizedMidi));
    if (!midiIngress.overflowed() || midiIngress.takeOversizeEvents() != 1 ||
        midiIngress.takeExtendedEvents() != 1 || midiIngress.size() != 0) return 15;
