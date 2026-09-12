    for (;;) {
        struct pollfd descriptor{controlFd, POLLIN | POLLHUP | POLLERR, 0};
        const int result = ::poll(&descriptor, 1, 1);
        if (result < 0 && errno == EINTR) continue;
        if (result > 0 && (descriptor.revents & (POLLHUP | POLLERR))) break;
        if (result > 0 && (descriptor.revents & POLLIN)) {
            uint8_t command = 0;
            const ssize_t bytes = ::read(controlFd, &command, sizeof(command));
            if (bytes != 1 || command == Aura::Core::Plugins::SandboxProtocol::kShutdown) break;
            if (command == Aura::Core::Plugins::SandboxProtocol::kReset) {
                bool resetOk = true;
                if (clapStarted && clapPlugin) {
                    clapPlugin->stop_processing(clapPlugin);
                    clapPlugin->deactivate(clapPlugin);
                    resetOk = clapPlugin->activate(clapPlugin, sampleRate, minFrames, maxFrames);
                    resetOk = resetOk && clapPlugin->start_processing(clapPlugin);
                }
#if defined(AURA_ENABLE_VST3_SDK)
                if (vst3Started) resetOk = vst3Runtime.reset() && resetOk;
#endif
#if defined(__APPLE__)
                if (auStarted) resetOk = auRuntime.reset() && resetOk;
#endif
                if (!resetOk) shared->processErrors.fetch_add(1, std::memory_order_release);
            }
        }
        shared->heartbeat.fetch_add(1, std::memory_order_relaxed);
        const uint64_t stateRequest = shared->stateRequestSequence.load(std::memory_order_acquire);
        const uint64_t stateCompleted = shared->stateCompletedSequence.load(std::memory_order_relaxed);
        if (stateRequest != stateCompleted) {
            const uint8_t mode = shared->stateMode.load(std::memory_order_relaxed);
            bool stateOk = false;
            uint8_t stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
            bool clapSuspendedForState = false;
            if (clapState && clapPlugin) {
                // CLAP state is a main-thread/control-plane operation.  A
                // number of production plugins assume the instance is
                // inactive while loading state and can deadlock when load is
                // called concurrently with process().  Keep save active
                // (some instruments only expose their current state while
                // active), but suspend the instance for load and restore the
                // lifecycle before acknowledging the request.
                if (mode == 2) {
                    clapPlugin->stop_processing(clapPlugin);
                    clapPlugin->deactivate(clapPlugin);
                    clapSuspendedForState = true;
                }
                StateStreamContext context;
                context.data = shared->state;
                context.capacity = Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes;
                if (mode == 1 && clapState->save) {
                    context.writing = true;
                    const Aura::Core::Plugins::ClapAbi::OStream stream{&context, &writeState};
                    stateOk = clapState->save(clapPlugin, &stream);
                    if (stateOk) {
                        if (context.size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                            stateOk = false;
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                        } else {
                            shared->stateSize.store(context.size, std::memory_order_release);
                            shared->stateVersion.store(
                                Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                                std::memory_order_release);
                            shared->stateChecksum.store(
                                Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, context.size),
                                std::memory_order_release);
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorNone;
                        }
                    }
                } else if (mode == 2 && clapState->load) {
                    context.writing = false;
                    context.size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (context.size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(version)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorVersion;
                    } else if (shared->stateChecksum.load(std::memory_order_acquire) !=
                               Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, context.size)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorChecksum;
                    } else {
                        const Aura::Core::Plugins::ClapAbi::IStream stream{&context, &readState};
                        stateOk = clapState->load(clapPlugin, &stream);
                        stateError = stateOk ? Aura::Core::Plugins::SandboxProtocol::kStateErrorNone
                                             : Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#if defined(AURA_ENABLE_VST3_SDK)
            else if (vst3Started) {
                if (mode == 1) {
                    std::vector<uint8_t> state;
                    stateOk = vst3Runtime.saveState(state);
                    if (stateOk) {
                        if (state.size() > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                            stateOk = false;
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                        } else {
                            std::memcpy(shared->state, state.data(), state.size());
                            shared->stateSize.store(static_cast<uint32_t>(state.size()), std::memory_order_release);
                            shared->stateVersion.store(
                                Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                                std::memory_order_release);
                            shared->stateChecksum.store(
                                Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, state.size()),
                                std::memory_order_release);
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorNone;
                        }
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(version)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorVersion;
                    } else if (shared->stateChecksum.load(std::memory_order_acquire) !=
                               Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, size)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorChecksum;
                    } else {
                        stateOk = vst3Runtime.loadState(shared->state, size);
                        stateError = stateOk ? Aura::Core::Plugins::SandboxProtocol::kStateErrorNone
                                             : Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#endif
#if defined(__APPLE__)
            else if (auStarted) {
                if (mode == 1) {
                    uint32_t size = 0;
                    stateOk = auRuntime.saveState(
                        shared->state, Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes, size);
                    if (stateOk) {
                        shared->stateSize.store(size, std::memory_order_release);
                        shared->stateVersion.store(
                            Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                            std::memory_order_release);
                        shared->stateChecksum.store(
                            Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, size),
                            std::memory_order_release);
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorNone;
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(version)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorVersion;
                    } else if (shared->stateChecksum.load(std::memory_order_acquire) !=
                               Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, size)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorChecksum;
                    } else {
                        stateOk = auRuntime.loadState(shared->state, size);
                        stateError = stateOk ? Aura::Core::Plugins::SandboxProtocol::kStateErrorNone
                                             : Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#endif
            if (clapSuspendedForState) {
                const bool reactivated = clapPlugin->activate(
                    clapPlugin, sampleRate, minFrames, maxFrames);
                const bool restartedProcessing = reactivated &&
                    clapPlugin->start_processing(clapPlugin);
                if (!reactivated || !restartedProcessing) {
                    stateOk = false;
                    stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                }
            }
            if (!stateOk && mode == 1) {
                shared->stateSize.store(0, std::memory_order_release);
                shared->stateVersion.store(0, std::memory_order_release);
                shared->stateChecksum.store(0, std::memory_order_release);
            }
            shared->stateError.store(stateError, std::memory_order_release);
            shared->stateResult.store(stateOk ? 1 : 0, std::memory_order_release);
            shared->stateCompletedSequence.store(stateRequest, std::memory_order_release);
        }
        const uint64_t request = shared->requestSequence.load(std::memory_order_acquire);
        const uint64_t completed = shared->completedSequence.load(std::memory_order_relaxed);
        if (request != 0 && request != completed) {
            ++processedBlocks;
            if (crashAfterBlocks != 0 && processedBlocks >= crashAfterBlocks) {
                // Test-only fault injection. The explicit opt-in prevents a
                // project or plugin environment variable from crashing a
                // production worker accidentally.
                ::_exit(90);
            }
            if (delayMilliseconds != 0)
                std::this_thread::sleep_for(std::chrono::milliseconds(delayMilliseconds));
            const uint32_t channels = shared->channels.load(std::memory_order_relaxed);
            const uint32_t frames = shared->frames.load(std::memory_order_relaxed);
            if (channels == 0 || channels > Aura::Core::Plugins::SandboxProtocol::kMaxChannels ||
                frames == 0 || frames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames) break;
            for (uint32_t channel = 0; channel < channels; ++channel) {
                std::memcpy(shared->output[channel], shared->input[channel],
                            static_cast<size_t>(frames) * sizeof(float));
            }
            if (clapStarted) {
                float* inputChannels[Aura::Core::Plugins::SandboxProtocol::kMaxChannels]{};
                float* outputChannels[Aura::Core::Plugins::SandboxProtocol::kMaxChannels]{};
                for (uint32_t channel = 0; channel < channels; ++channel) {
                    inputChannels[channel] = shared->input[channel];
                    outputChannels[channel] = shared->output[channel];
                }
                Aura::Core::Plugins::ClapAbi::AudioBuffer input{
                    inputChannels, nullptr, channels, 0, 0};
                Aura::Core::Plugins::ClapAbi::AudioBuffer output{
                    outputChannels, nullptr, channels, 0, 0};
                const uint32_t midiEvents = std::min<uint32_t>(
                    shared->midiEvents.load(std::memory_order_acquire),
                    Aura::Core::Plugins::SandboxProtocol::kMaxMidiEvents);
                MidiInputContext midiContext{shared->midi, midiEvents};
                while (midiContext.extendedCount < midiContext.extended.size() &&
                       shared->extendedMidi.pop(midiContext.extended[midiContext.extendedCount])) {
                    ++midiContext.extendedCount;
                }
                midiContext.parameterCount = std::min<uint32_t>(
                    shared->parameterChanges.load(std::memory_order_acquire),
                    Aura::Core::Plugins::SandboxProtocol::kMaxParameterChanges);
                for (uint32_t parameterIndex = 0;
                     parameterIndex < midiContext.parameterCount; ++parameterIndex) {
                    const auto& source = shared->parameterChange[parameterIndex];
                    auto& destination = midiContext.parameters[parameterIndex];
                    destination.header.size = sizeof(destination);
                    destination.header.time = std::min<uint32_t>(source.sampleOffset, frames);
                    destination.header.space_id = Aura::Core::Plugins::ClapAbi::kCoreEventSpaceId;
                    destination.header.type = Aura::Core::Plugins::ClapAbi::kEventParamValue;
                    destination.header.flags = 0;
                    destination.param_id = source.parameterId;
                    destination.cookie = nullptr;
                    destination.value = source.value;
                    destination.note_id = -1;
                    destination.port_index = 0;
                    destination.channel = -1;
                    destination.key = -1;
                }
                const Aura::Core::Plugins::ClapAbi::InputEvents inputEvents{
                    &midiContext, &midiInputSize, &midiInputGet};
                MidiOutputContext midiOutputContext{shared, frames};
                const Aura::Core::Plugins::ClapAbi::OutputEvents outputEvents{
                    &midiOutputContext, &midiOutputTryPush};
                Aura::Core::Plugins::ClapAbi::Process process{
                    0, frames, nullptr, &input, 1, &output, 1, &inputEvents,
                    const_cast<Aura::Core::Plugins::ClapAbi::OutputEvents*>(&outputEvents)};
                const auto processStatus = clapPlugin->process(clapPlugin, &process);
                if (processStatus == Aura::Core::Plugins::ClapAbi::Error) {
                    std::memset(shared->output, 0, sizeof(shared->output));
                    shared->processErrors.fetch_add(1, std::memory_order_release);
                }
#if defined(__APPLE__)
            } else if (auStarted && !auRuntime.process(*shared, channels, frames)) {
                std::memset(shared->output, 0, sizeof(shared->output));
                shared->processErrors.fetch_add(1, std::memory_order_release);
#endif
#if defined(AURA_ENABLE_VST3_SDK)
            } else if (vst3Started) {
                const bool processed = vst3Runtime.process(*shared, channels, frames);
                if (!processed) {
                    std::memset(shared->output, 0, sizeof(shared->output));
                    shared->processErrors.fetch_add(1, std::memory_order_release);
                }
#endif
            }
            // Parameter changes belong to this exact mailbox block. Clear
            // them only after the selected backend consumed the block, so a
            // slow worker cannot reapply an old change on the next request.
            shared->parameterChanges.store(0, std::memory_order_release);
            shared->completedSequence.store(request, std::memory_order_release);
        }
    }
#if !defined(_WIN32)
    if (clapStarted) { clapPlugin->stop_processing(clapPlugin); clapPlugin->deactivate(clapPlugin); clapPlugin->destroy(clapPlugin); clapEntry->deinit(); }
    if (pluginHandle != reinterpret_cast<void*>(1)) ::dlclose(pluginHandle);
#endif
    ::munmap(shared, sizeof(*shared));
    return 0;
}
