int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--capabilities") == 0) {
        const char* capabilities =
#if defined(AURA_ENABLE_VST3_SDK)
            "{\"clap\":true,\"au\":true,\"vst3\":true}\n";
#elif defined(__APPLE__)
            "{\"clap\":true,\"au\":true,\"vst3\":false}\n";
#else
            "{\"clap\":true,\"au\":false,\"vst3\":false}\n";
#endif
        (void)::write(STDOUT_FILENO, capabilities, std::strlen(capabilities));
        return 0;
    }
    int controlFd = -1;
    int statusFd = -1;
    int sharedFd = -1;
    const char* pluginPath = nullptr;
    const char* sharedName = nullptr;
    double sampleRate = 44100.0;
    uint32_t minFrames = 1;
    uint32_t maxFrames = Aura::Core::Plugins::SandboxProtocol::kMaxFrames;
    uint32_t channels = Aura::Core::Plugins::SandboxProtocol::kMaxChannels;
    const char* requestedFormat = "auto";
    for (int i = 1; i < argc; ++i) {
        const char* option = argv[i];
        if (i + 1 >= argc) {
            debugLog("missing command-line option value");
            return 2;
        }
        const char* value = argv[++i];
        if (std::strcmp(option, "--control-fd") == 0) {
            if (!parseInt(value, controlFd)) return 2;
        } else if (std::strcmp(option, "--status-fd") == 0) {
            if (!parseInt(value, statusFd)) return 2;
        } else if (std::strcmp(option, "--shared-fd") == 0) {
            if (!parseInt(value, sharedFd)) return 2;
        } else if (std::strcmp(option, "--shared-name") == 0) {
            sharedName = value;
        } else if (std::strcmp(option, "--plugin") == 0) {
            pluginPath = value;
        } else if (std::strcmp(option, "--format") == 0) {
            requestedFormat = value;
        } else if (std::strcmp(option, "--sample-rate") == 0) {
            if (!parseDouble(value, sampleRate)) return 2;
        } else if (std::strcmp(option, "--min-frames") == 0) {
            if (!parseUint32(value, minFrames)) return 2;
        } else if (std::strcmp(option, "--max-frames") == 0) {
            if (!parseUint32(value, maxFrames)) return 2;
        } else if (std::strcmp(option, "--channels") == 0) {
            if (!parseUint32(value, channels)) return 2;
        } else {
            debugLog("unknown command-line option");
            return 2;
        }
    }
    if (controlFd < 0 || statusFd < 0 || (sharedFd < 0 && (!sharedName || *sharedName == '\0')) ||
        pluginPath == nullptr || *pluginPath == '\0') {
        debugLog("required worker argument is invalid");
        return 2;
    }
    if (!requestedFormat ||
        (std::strcmp(requestedFormat, "auto") != 0 &&
         std::strcmp(requestedFormat, "builtin") != 0 &&
         std::strcmp(requestedFormat, "clap") != 0 &&
         std::strcmp(requestedFormat, "vst3") != 0 &&
         std::strcmp(requestedFormat, "au") != 0)) {
        debugLog("requested plugin format is invalid");
        return 2;
    }
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || sampleRate > 384000.0 ||
        minFrames == 0 || minFrames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames ||
        maxFrames < minFrames || maxFrames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames ||
        channels == 0 || channels > Aura::Core::Plugins::SandboxProtocol::kMaxChannels) {
        debugLog("requested audio configuration is invalid");
        return 2;
    }

    if (!applyWorkerLimits()) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorSecurity;
        (void)::write(statusFd, &error, sizeof(error));
        return 11;
    }

    struct stat sharedStat{};
    bool sharedValid = ::fstat(sharedFd, &sharedStat) == 0 &&
        static_cast<size_t>(sharedStat.st_size) >= sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock);
    if (!sharedValid && sharedName) {
        const int namedFd = ::shm_open(sharedName, O_RDWR, 0);
        if (namedFd >= 0) {
            sharedFd = namedFd;
            sharedValid = ::fstat(sharedFd, &sharedStat) == 0 &&
                static_cast<size_t>(sharedStat.st_size) >= sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock);
        }
    }
    if (!sharedValid ||
        static_cast<size_t>(sharedStat.st_size) < sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock)) {
        debugLog("shared audio block descriptor and name are unavailable");
        return 4;
    }
    auto* shared = static_cast<Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock*>(
        ::mmap(nullptr, sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock),
               PROT_READ | PROT_WRITE, MAP_SHARED, sharedFd, 0));
    if (shared == MAP_FAILED) {
        debugLog("shared audio block mapping failed");
        return 5;
    }
    if (shared->protocolVersion.load(std::memory_order_acquire) !=
        Aura::Core::Plugins::SandboxProtocol::kAudioBlockProtocolVersion) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorAbi;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock));
        return 12;
    }
    shared->activeSampleRate.store(static_cast<uint32_t>(sampleRate), std::memory_order_release);
    shared->activeChannels.store(channels, std::memory_order_release);

    void* pluginHandle = nullptr;
    const Aura::Core::Plugins::ClapAbi::Entry* clapEntry = nullptr;
    const Aura::Core::Plugins::ClapAbi::Plugin* clapPlugin = nullptr;
    bool clapStarted = false;
#if defined(__APPLE__)
    Aura::Core::Plugins::SandboxAU::Runtime auRuntime;
    bool auStarted = false;
#endif
#if defined(AURA_ENABLE_VST3_SDK)
    Aura::Core::Plugins::SandboxVST3::Runtime vst3Runtime;
    bool vst3Started = false;
#endif
    const Aura::Core::Plugins::ClapAbi::StateExtension* clapState = nullptr;
    const bool testFaultsEnabled = std::getenv("AURA_PLUGIN_TEST_FAULTS") != nullptr;
    uint64_t processedBlocks = 0;
    uint64_t crashAfterBlocks = 0;
    uint32_t delayMilliseconds = 0;
    if (testFaultsEnabled) {
        if (const char* value = std::getenv("AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS")) {
            uint64_t parsed = 0;
            if (parseUint64(value, parsed)) crashAfterBlocks = parsed;
        }
        if (const char* value = std::getenv("AURA_PLUGIN_WORKER_DELAY_MS")) {
            uint32_t parsed = 0;
            if (parseUint32(value, parsed)) delayMilliseconds = parsed;
        }
    }
#if !defined(_WIN32)
    const bool builtinPassthrough = std::strncmp(pluginPath, "builtin://", 10) == 0;
    const bool pathIsClap = hasPluginExtension(pluginPath, ".clap");
    const bool pathIsVst3 = hasPluginExtension(pluginPath, ".vst3");
    const bool pathIsAudioUnit = hasPluginExtension(pluginPath, ".component");
    const bool isClap = std::strcmp(requestedFormat, "clap") == 0 ? pathIsClap :
                        (std::strcmp(requestedFormat, "auto") == 0 && pathIsClap);
    const bool isVst3 = std::strcmp(requestedFormat, "vst3") == 0 ? pathIsVst3 :
                        (std::strcmp(requestedFormat, "auto") == 0 && pathIsVst3);
    const bool isAudioUnit = std::strcmp(requestedFormat, "au") == 0 ? pathIsAudioUnit :
                             (std::strcmp(requestedFormat, "auto") == 0 && pathIsAudioUnit);
    const bool requestedBuiltin = std::strcmp(requestedFormat, "builtin") == 0;
    if ((requestedFormat[0] == 'c' && !pathIsClap) ||
        (requestedFormat[0] == 'v' && !pathIsVst3) ||
        (requestedFormat[0] == 'a' && !pathIsAudioUnit) ||
        (requestedBuiltin && !builtinPassthrough)) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 12;
    }
    if (isVst3) {
#if defined(AURA_ENABLE_VST3_SDK)
        if (!vst3Runtime.load(pluginPath, sampleRate, maxFrames)) {
            debugLog(vst3Runtime.error());
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            ::munmap(shared, sizeof(*shared));
            return 10;
        }
        vst3Started = true;
#else
        // VST3 requires the official ABI interfaces. Do not acknowledge
        // readiness until an SDK adapter is present.
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 12;
#endif
    }
    if (isAudioUnit) {
#if defined(__APPLE__)
        if (!auRuntime.load(pluginPath, sampleRate, maxFrames, channels)) {
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            ::munmap(shared, sizeof(*shared));
            return 10;
        }
        auStarted = true;
#else
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 12;
#endif
    }
    const bool vst3SdkLoaded = isVst3 &&
#if defined(AURA_ENABLE_VST3_SDK)
        true;
#else
        false;
#endif
    const std::string pluginImage = builtinPassthrough || isAudioUnit || vst3SdkLoaded
        ? std::string{} : resolveMacPluginImage(pluginPath);
    debugLog(pluginImage.empty() ? "plugin image empty" : pluginImage.c_str());
    debugLog("resolving plugin image");
    pluginHandle = builtinPassthrough || isAudioUnit || vst3SdkLoaded ? reinterpret_cast<void*>(1) :
        (pluginImage.empty() ? nullptr : ::dlopen(pluginImage.c_str(), RTLD_NOW | RTLD_LOCAL));
    if (pluginHandle == nullptr) {
        debugLog("dlopen failed");
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorLoad;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 6;
    }
    debugLog("plugin image ready");
    const char* symbol = isClap ? "clap_entry" : (isVst3 ? "GetPluginFactory" : nullptr);
    if (!builtinPassthrough && !isAudioUnit && !vst3SdkLoaded &&
        (symbol == nullptr || ::dlsym(pluginHandle, symbol) == nullptr)) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorAbi;
        (void)::write(statusFd, &error, sizeof(error));
        if (!builtinPassthrough && !isAudioUnit) ::dlclose(pluginHandle);
        ::munmap(shared, sizeof(*shared));
        return 7;
    }
    if (isClap && !builtinPassthrough) {
        debugLog("initializing CLAP entry");
        // CLAP exports `clap_entry` as a const entry object, not a function.
        // Calling the symbol as a function is undefined behaviour.
        clapEntry = reinterpret_cast<const Aura::Core::Plugins::ClapAbi::Entry*>(
            ::dlsym(pluginHandle, "clap_entry"));
        if (!clapEntry || !clapEntry->init || !clapEntry->deinit || !clapEntry->get_factory ||
            clapEntry->clap_version.major != 1 || !clapEntry->init(pluginPath)) {
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorAbi;
            (void)::write(statusFd, &error, sizeof(error));
            ::dlclose(pluginHandle);
            ::munmap(shared, sizeof(*shared));
            return 8;
        }
        const auto* factory = static_cast<const Aura::Core::Plugins::ClapAbi::Factory*>(
            clapEntry->get_factory(Aura::Core::Plugins::ClapAbi::kPluginFactoryId));
        debugLog("factory acquired");
        if (!factory || !factory->get_plugin_count || !factory->get_plugin_descriptor ||
            !factory->create_plugin || factory->get_plugin_count(factory) == 0) {
            clapEntry->deinit();
            ::dlclose(pluginHandle);
            ::munmap(shared, sizeof(*shared));
            return 9;
        }
        uint32_t descriptorIndex = 0;
        const char* requestedFeature = std::getenv("AURA_PLUGIN_KIND");
        if (requestedFeature && *requestedFeature) {
            const uint32_t count = factory->get_plugin_count(factory);
            bool found = false;
            for (uint32_t candidate = 0; candidate < count; ++candidate) {
                const auto* candidateDescriptor =
                    factory->get_plugin_descriptor(factory, candidate);
                if (clapDescriptorHasFeature(candidateDescriptor, requestedFeature)) {
                    descriptorIndex = candidate;
                    found = true;
                    break;
                }
            }
            if (!found) {
                debugLog("requested CLAP feature was not found");
                clapEntry->deinit();
                ::dlclose(pluginHandle);
                ::munmap(shared, sizeof(*shared));
                const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
                (void)::write(statusFd, &error, sizeof(error));
                return 10;
            }
        }
        const auto* descriptor = factory->get_plugin_descriptor(factory, descriptorIndex);
        Aura::Core::Plugins::ClapAbi::Host host{
            {1, 0, 0}, "aura-sandbox", "Aura", "Aura", "", "1",
            &noopRestart, &noopProcess, &noopCallback, &noExtension};
        clapPlugin = descriptor ? factory->create_plugin(factory, &host, descriptor->id) : nullptr;
        debugLog("plugin instance created");
        const bool validPlugin = clapPlugin && clapPlugin->init && clapPlugin->destroy &&
            clapPlugin->activate && clapPlugin->deactivate && clapPlugin->start_processing &&
            clapPlugin->stop_processing && clapPlugin->process;
        if (!validPlugin) debugLog("plugin vtable incomplete");
        const bool initialized = validPlugin && clapPlugin->init(clapPlugin);
        debugLog(initialized ? "plugin init ok" : "plugin init failed");
        const bool activated = initialized && clapPlugin->activate(
            clapPlugin, sampleRate, minFrames, maxFrames);
        debugLog(activated ? "plugin activate ok" : "plugin activate failed");
        const bool processing = activated && clapPlugin->start_processing(clapPlugin);
        debugLog(processing ? "plugin start_processing ok" : "plugin start_processing failed");
        if (!validPlugin || !initialized || !activated || !processing) {
            if (clapPlugin && clapPlugin->destroy) clapPlugin->destroy(clapPlugin);
            clapEntry->deinit();
            ::dlclose(pluginHandle);
            ::munmap(shared, sizeof(*shared));
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            return 10;
        }
        debugLog("plugin activated and processing started");
        clapStarted = true;
        clapState = static_cast<const Aura::Core::Plugins::ClapAbi::StateExtension*>(
            clapPlugin->get_extension ? clapPlugin->get_extension(clapPlugin, Aura::Core::Plugins::ClapAbi::kStateExtensionId) : nullptr);
    }
#else
    (void)pluginPath;
#endif

    const uint8_t ready = Aura::Core::Plugins::SandboxProtocol::kReady;
    debugLog("sending ready");
    if (::write(statusFd, &ready, sizeof(ready)) != 1) {
#if !defined(_WIN32)
        if (clapStarted) { clapPlugin->stop_processing(clapPlugin); clapPlugin->deactivate(clapPlugin); clapPlugin->destroy(clapPlugin); clapEntry->deinit(); }
        if (!builtinPassthrough && !isAudioUnit) ::dlclose(pluginHandle);
#endif
        ::munmap(shared, sizeof(*shared));
        return 3;
    }

