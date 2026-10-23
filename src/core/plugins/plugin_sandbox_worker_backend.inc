    struct WorkerBackendContext {
        Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock* shared;
        void* clapInstance;
        const void*& clapPlugin;
        const void*& clapState;
        bool& clapStarted;
        void* clapInputContext;
        double& sampleRate;
        uint32_t& minFrames;
        uint32_t& maxFrames;
#if defined(__APPLE__)
        Hirari::Core::Plugins::SandboxAU::Runtime& auRuntime;
        bool& auStarted;
#endif
#if defined(HIRARI_ENABLE_VST3_SDK)
        Hirari::Core::Plugins::SandboxVST3::Runtime& vst3Runtime;
        bool& vst3Started;
#endif
    } context{shared, clapInstance, clapPlugin, clapState, clapStarted, clapInputContext,
        sampleRate, minFrames, maxFrames
#if defined(__APPLE__)
        , auRuntime, auStarted
#endif
#if defined(HIRARI_ENABLE_VST3_SDK)
        , vst3Runtime, vst3Started
#endif
    };
    const HirariPluginWorkerBackend backend{
        +[](void* opaque) -> bool {
            auto& context = *static_cast<WorkerBackendContext*>(opaque);
            auto*& clapPlugin = context.clapPlugin;
            auto& clapStarted = context.clapStarted;
            bool resetOk = true;
            if (clapStarted && clapPlugin) {
                resetOk = hirari_clap_instance_reset(
                    context.clapInstance, context.sampleRate,
                    context.minFrames, context.maxFrames);
            }
#if defined(HIRARI_ENABLE_VST3_SDK)
            if (context.vst3Started) resetOk = context.vst3Runtime.reset() && resetOk;
#endif
#if defined(__APPLE__)
            if (context.auStarted) resetOk = context.auRuntime.reset() && resetOk;
#endif
            return resetOk;
        },
        +[](void* opaque, uint8_t mode) -> uint16_t {
            auto& context = *static_cast<WorkerBackendContext*>(opaque);
            auto* shared = context.shared;
            if (context.clapState && context.clapPlugin) {
                return hirari_clap_state_block(
                    context.clapPlugin, context.clapState, context.clapInputContext,
                    mode, shared->state, &shared->stateSize, &shared->stateVersion,
                    &shared->stateChecksum, context.sampleRate,
                    context.minFrames, context.maxFrames);
            }
            bool stateOk = false;
            uint8_t stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
            [[maybe_unused]] bool backendStateHandled = false;
#if defined(HIRARI_ENABLE_VST3_SDK)
            if (context.vst3Started) {
                backendStateHandled = true;
                if (mode == 1) {
                    std::vector<uint8_t> state;
                    stateOk = context.vst3Runtime.saveState(state);
                    if (stateOk && state.size() <= Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        std::memcpy(shared->state, state.data(), state.size());
                        stateError = hirari_plugin_worker_publish_state(
                            shared->state, static_cast<uint32_t>(state.size()),
                            &shared->stateSize, &shared->stateVersion, &shared->stateChecksum);
                        if (stateError != Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone)
                            stateOk = false;
                    } else if (stateOk) {
                        stateOk = false;
                        stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    stateError = hirari_plugin_worker_validate_state(
                        shared->state, &shared->stateSize, &shared->stateVersion,
                        &shared->stateChecksum);
                    if (stateError == Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone) {
                        stateOk = context.vst3Runtime.loadState(shared->state, size);
                        if (!stateOk)
                            stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#endif
#if defined(__APPLE__)
            if (!backendStateHandled && context.auStarted) {
                if (mode == 1) {
                    uint32_t size = 0;
                    stateOk = context.auRuntime.saveState(
                        shared->state, Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes, size);
                    if (stateOk) {
                        shared->stateSize.store(size, std::memory_order_release);
                        stateError = hirari_plugin_worker_publish_state(
                            shared->state, size, &shared->stateSize, &shared->stateVersion,
                            &shared->stateChecksum);
                        if (stateError != Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone)
                            stateOk = false;
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    stateError = hirari_plugin_worker_validate_state(
                        shared->state, &shared->stateSize, &shared->stateVersion,
                        &shared->stateChecksum);
                    if (stateError == Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone) {
                        stateOk = context.auRuntime.loadState(shared->state, size);
                        if (!stateOk)
                            stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#endif
            return static_cast<uint16_t>((static_cast<uint16_t>(stateError) << 8) |
                                         static_cast<uint16_t>(stateOk));
        },
        +[](void* opaque, void* sharedOpaque, uint32_t channels, uint32_t frames) -> bool {
            auto& context = *static_cast<WorkerBackendContext*>(opaque);
            auto*& clapPlugin = context.clapPlugin;
            auto& clapStarted = context.clapStarted;
            auto* block = static_cast<Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock*>(sharedOpaque);
            auto* shared = block;
            for (uint32_t channel = 0; channel < channels; ++channel) {
                std::memcpy(shared->output[channel], shared->input[channel],
                            static_cast<size_t>(frames) * sizeof(float));
            }
            if (clapStarted) {
                const uint32_t midiCount = shared->midiEvents.load(std::memory_order_acquire);
                const uint32_t parameterCount = shared->parameterChanges.load(std::memory_order_acquire);
                return hirari_clap_process_block(
                    clapPlugin, context.clapInputContext,
                    &shared->input[0][0], &shared->output[0][0], channels, frames,
                    shared->midi, midiCount, &shared->extendedMidi,
                    shared->parameterChange, parameterCount);
#if defined(__APPLE__)
            } else if (context.auStarted && !context.auRuntime.process(*shared, channels, frames)) {
                std::memset(shared->output, 0, sizeof(shared->output));
                return false;
#endif
#if defined(HIRARI_ENABLE_VST3_SDK)
            } else if (context.vst3Started && !context.vst3Runtime.process(*shared, channels, frames)) {
                std::memset(shared->output, 0, sizeof(shared->output));
                return false;
#endif
            }
            return true;
        }};
    const HirariPluginWorkerFaultConfig faults{crashAfterBlocks, delayMilliseconds};
    const int32_t workerResult = hirari_plugin_worker_run(
        controlFd, shared, &context, &backend, &faults);
    (void)workerResult;
    hirari_clap_input_context_destroy(clapInputContext);
