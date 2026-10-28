int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--capabilities") == 0) {
        const char* capabilities =
#if defined(HIRARI_ENABLE_VST3_SDK)
            "{\"clap\":true,\"au\":true,\"vst3\":true}\n";
#elif defined(__APPLE__)
            "{\"clap\":true,\"au\":true,\"vst3\":false}\n";
#else
            "{\"clap\":true,\"au\":false,\"vst3\":false}\n";
#endif
        (void)::write(STDOUT_FILENO, capabilities, std::strlen(capabilities));
        return 0;
    }
    HirariPluginWorkerLaunchOptions options{};
    uint32_t parseError = 0;
    if (!hirari_plugin_worker_parse_args(argc,
            const_cast<const char* const*>(argv), &options, &parseError)) {
        debugLog(hirari_plugin_worker_parse_error_text(parseError));
        return 2;
    }
    const int controlFd = options.controlFd;
    const int statusFd = options.statusFd;
    int sharedFd = options.sharedFd;
    const char* pluginPath = options.pluginPath;
    const char* sharedName = options.sharedName;
    double sampleRate = options.sampleRate;
    uint32_t minFrames = options.minFrames;
    uint32_t maxFrames = options.maxFrames;
    uint32_t channels = options.channels;

    if (!hirari_plugin_worker_apply_limits()) {
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorSecurity;
        (void)::write(statusFd, &error, sizeof(error));
        return 11;
    }

    uint32_t mappingError = 0;
    auto* shared = static_cast<Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock*>(
        hirari_plugin_worker_map_shared(
            sharedFd, sharedName, sizeof(Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock),
            &mappingError));
    if (!shared) {
        if (mappingError == 2) {
            debugLog("shared audio block mapping failed");
            return 5;
        }
        debugLog("shared audio block descriptor and name are unavailable");
        return 4;
    }
    if (shared->protocolVersion.load(std::memory_order_acquire) !=
        Hirari::Core::Plugins::SandboxProtocol::kAudioBlockProtocolVersion) {
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorAbi;
        (void)::write(statusFd, &error, sizeof(error));
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 12;
    }
    shared->activeSampleRate.store(static_cast<uint32_t>(sampleRate), std::memory_order_release);
    shared->activeChannels.store(channels, std::memory_order_release);

    void* pluginHandle = nullptr;
    void* clapInstance = nullptr;
    const void* clapPlugin = nullptr;
    bool midiInstrument = false;
    bool clapStarted = false;
#if defined(__APPLE__)
    Hirari::Core::Plugins::SandboxAU::Runtime auRuntime;
    bool auStarted = false;
#endif
#if defined(HIRARI_ENABLE_VST3_SDK)
    Hirari::Core::Plugins::SandboxVST3::Runtime vst3Runtime;
    bool vst3Started = false;
#endif
    const void* clapState = nullptr;
    const uint64_t crashAfterBlocks = options.crashAfterBlocks;
    const uint32_t delayMilliseconds = options.delayMilliseconds;
#if !defined(_WIN32)
    const bool builtinPassthrough = options.builtinPassthrough != 0;
    const bool pathIsClap = options.pathIsClap != 0;
    const bool pathIsVst3 = options.pathIsVst3 != 0;
    const bool pathIsAudioUnit = options.pathIsAudioUnit != 0;
    const bool isClap = (options.requestedFormat == 2 || options.requestedFormat == 0) && pathIsClap;
    const bool isVst3 = (options.requestedFormat == 3 || options.requestedFormat == 0) && pathIsVst3;
    const bool isAudioUnit = (options.requestedFormat == 4 || options.requestedFormat == 0) && pathIsAudioUnit;
    const bool requestedBuiltin = options.requestedFormat == 1;
    if ((options.requestedFormat == 2 && !pathIsClap) ||
        (options.requestedFormat == 3 && !pathIsVst3) ||
        (options.requestedFormat == 4 && !pathIsAudioUnit) ||
        (requestedBuiltin && !builtinPassthrough)) {
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 12;
    }
    if (isVst3) {
#if defined(HIRARI_ENABLE_VST3_SDK)
        if (!vst3Runtime.load(pluginPath, sampleRate, maxFrames)) {
            debugLog(vst3Runtime.error());
            const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
            return 10;
        }
        vst3Started = true;
        midiInstrument = vst3Runtime.isMidiInstrument();
#else
        // VST3 requires the official ABI interfaces. Do not acknowledge
        // readiness until an SDK adapter is present.
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 12;
#endif
    }
    if (isAudioUnit) {
#if defined(__APPLE__)
        if (!auRuntime.load(pluginPath, sampleRate, maxFrames, channels)) {
            const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
            return 10;
        }
        auStarted = true;
        midiInstrument = auRuntime.isMidiInstrument();
#else
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 12;
#endif
    }
    const bool vst3SdkLoaded = isVst3 &&
#if defined(HIRARI_ENABLE_VST3_SDK)
        true;
#else
        false;
#endif
    const std::string pluginImage = builtinPassthrough || isAudioUnit || vst3SdkLoaded
        ? std::string{} : resolveMacPluginImage(pluginPath);
    debugLog(pluginImage.empty() ? "plugin image empty" : pluginImage.c_str());
    debugLog("resolving plugin image");
    pluginHandle = builtinPassthrough || isAudioUnit || vst3SdkLoaded || isClap
        ? reinterpret_cast<void*>(1)
        : (pluginImage.empty() ? nullptr : ::dlopen(pluginImage.c_str(), RTLD_NOW | RTLD_LOCAL));
    if (pluginHandle == nullptr) {
        debugLog("dlopen failed");
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorLoad;
        (void)::write(statusFd, &error, sizeof(error));
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 6;
    }
    debugLog("plugin image ready");
    const char* symbol = isVst3 ? "GetPluginFactory" : nullptr;
    if (!builtinPassthrough && !isAudioUnit && !vst3SdkLoaded && !isClap &&
        (symbol == nullptr || ::dlsym(pluginHandle, symbol) == nullptr)) {
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorAbi;
        (void)::write(statusFd, &error, sizeof(error));
        if (pluginHandle != reinterpret_cast<void*>(1)) ::dlclose(pluginHandle);
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 7;
    }
    if (isClap && !builtinPassthrough) {
        const int32_t loadResult = hirari_clap_instance_load(
            pluginImage.c_str(), pluginPath, sampleRate, minFrames, maxFrames,
            &clapInstance, &midiInstrument);
        if (loadResult != 0) {
            const uint8_t error = loadResult == 1
                ? Hirari::Core::Plugins::SandboxProtocol::kErrorLoad
                : (loadResult == 2
                    ? Hirari::Core::Plugins::SandboxProtocol::kErrorAbi
                    : Hirari::Core::Plugins::SandboxProtocol::kErrorInstance);
            (void)::write(statusFd, &error, sizeof(error));
            hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
            return loadResult == 1 ? 6 : (loadResult == 2 ? 8 : 10);
        }
        clapPlugin = hirari_clap_instance_plugin(clapInstance);
        clapState = hirari_clap_instance_state(clapInstance);
        clapStarted = clapPlugin != nullptr;
        if (!clapStarted) {
            hirari_clap_instance_destroy(clapInstance);
            hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
            return 10;
        }
    }
#else
    (void)pluginPath;
#endif

    void* clapInputContext = clapStarted ? hirari_clap_input_context_create(
        &shared->outputMidiEvents, &shared->outputMidiDropped, shared->outputMidi) : nullptr;
    if (clapStarted && !clapInputContext) {
        const uint8_t error = Hirari::Core::Plugins::SandboxProtocol::kErrorInstance;
        (void)::write(statusFd, &error, sizeof(error));
#if !defined(_WIN32)
        hirari_clap_instance_destroy(clapInstance);
#endif
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 10;
    }
    shared->midiInstrument.store(midiInstrument ? 1u : 0u, std::memory_order_release);
    const uint8_t ready = Hirari::Core::Plugins::SandboxProtocol::kReady;
    debugLog("sending ready");
    if (::write(statusFd, &ready, sizeof(ready)) != 1) {
#if !defined(_WIN32)
        if (clapInstance) hirari_clap_instance_destroy(clapInstance);
        if (pluginHandle != reinterpret_cast<void*>(1)) ::dlclose(pluginHandle);
#endif
        hirari_clap_input_context_destroy(clapInputContext);
        hirari_plugin_worker_unmap_shared(shared, sizeof(*shared));
        return 3;
    }
