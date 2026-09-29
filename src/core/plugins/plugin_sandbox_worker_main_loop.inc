    for (;;) {
        struct pollfd descriptor{controlFd, POLLIN | POLLHUP | POLLERR, 0};
        const int result = ::poll(&descriptor, 1, 1);
        if (result < 0 && errno == EINTR) continue;
        if (result > 0 && (descriptor.revents & (POLLHUP | POLLERR))) break;
        if (result > 0 && (descriptor.revents & POLLIN)) {
            uint8_t command = 0;
            const ssize_t bytes = ::read(controlFd, &command, sizeof(command));
            if (bytes != 1 || command == Hirari::Core::Plugins::SandboxProtocol::kShutdown) break;
            if (command == Hirari::Core::Plugins::SandboxProtocol::kReset) {
                bool resetOk = true;
                if (clapStarted && clapPlugin) {
                    clapPlugin->stop_processing(clapPlugin);
                    clapPlugin->deactivate(clapPlugin);
                    resetOk = clapPlugin->activate(clapPlugin, sampleRate, minFrames, maxFrames);
                    resetOk = resetOk && clapPlugin->start_processing(clapPlugin);
                }
#if defined(HIRARI_ENABLE_VST3_SDK)
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
            uint8_t stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
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
                context.capacity = Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes;
                if (mode == 1 && clapState->save) {
                    context.writing = true;
                    const Hirari::Core::Plugins::ClapAbi::OStream stream{&context, &writeState};
                    stateOk = clapState->save(clapPlugin, &stream);
                    if (stateOk) {
                        if (context.size > Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                            stateOk = false;
                            stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                        } else {
                            shared->stateSize.store(context.size, std::memory_order_release);
                            shared->stateVersion.store(
                                Hirari::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                                std::memory_order_release);
                            shared->stateChecksum.store(
                                hirari_plugin_state_checksum(shared->state, context.size),
                                std::memory_order_release);
                            stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone;
                        }
                    }
                } else if (mode == 2 && clapState->load) {
                    context.writing = false;
                    context.size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (context.size > Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else {
                        stateError = hirari_plugin_state_validate(
                            shared->state, context.size, version,
                            shared->stateChecksum.load(std::memory_order_acquire));
                        if (stateError == Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone) {
                            const Hirari::Core::Plugins::ClapAbi::IStream stream{&context, &readState};
                            stateOk = clapState->load(clapPlugin, &stream);
                            stateError = stateOk ? Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone
                                                 : Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                        }
                    }
                }
            }
#if defined(HIRARI_ENABLE_VST3_SDK)
            else if (vst3Started) {
                if (mode == 1) {
                    std::vector<uint8_t> state;
                    stateOk = vst3Runtime.saveState(state);
                    if (stateOk) {
                        if (state.size() > Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                            stateOk = false;
                            stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                        } else {
                            std::memcpy(shared->state, state.data(), state.size());
                            shared->stateSize.store(static_cast<uint32_t>(state.size()), std::memory_order_release);
                            shared->stateVersion.store(
                                Hirari::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                                std::memory_order_release);
                            shared->stateChecksum.store(
                                hirari_plugin_state_checksum(shared->state, state.size()),
                                std::memory_order_release);
                            stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone;
                        }
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (size > Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else {
                        stateError = hirari_plugin_state_validate(
                            shared->state, size, version,
                            shared->stateChecksum.load(std::memory_order_acquire));
                        if (stateError == Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone) {
                            stateOk = vst3Runtime.loadState(shared->state, size);
                            stateError = stateOk ? Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone
                                                 : Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                        }
                    }
                }
            }
#endif
#if defined(__APPLE__)
            else if (auStarted) {
                if (mode == 1) {
                    uint32_t size = 0;
                    stateOk = auRuntime.saveState(
                        shared->state, Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes, size);
                    if (stateOk) {
                        shared->stateSize.store(size, std::memory_order_release);
                        shared->stateVersion.store(
                            Hirari::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                            std::memory_order_release);
                        shared->stateChecksum.store(
                            hirari_plugin_state_checksum(shared->state, size),
                            std::memory_order_release);
                        stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone;
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (size > Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else {
                        stateError = hirari_plugin_state_validate(
                            shared->state, size, version,
                            shared->stateChecksum.load(std::memory_order_acquire));
                        if (stateError == Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone) {
                            stateOk = auRuntime.loadState(shared->state, size);
                            stateError = stateOk ? Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone
                                                 : Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                        }
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
                    stateError = Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
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
            if (channels == 0 || channels > Hirari::Core::Plugins::SandboxProtocol::kMaxChannels ||
                frames == 0 || frames > Hirari::Core::Plugins::SandboxProtocol::kMaxFrames) break;
            for (uint32_t channel = 0; channel < channels; ++channel) {
                std::memcpy(shared->output[channel], shared->input[channel],
                            static_cast<size_t>(frames) * sizeof(float));
            }
            if (clapStarted) {
                float* inputChannels[Hirari::Core::Plugins::SandboxProtocol::kMaxChannels]{};
                float* outputChannels[Hirari::Core::Plugins::SandboxProtocol::kMaxChannels]{};
                for (uint32_t channel = 0; channel < channels; ++channel) {
                    inputChannels[channel] = shared->input[channel];
                    outputChannels[channel] = shared->output[channel];
                }
                Hirari::Core::Plugins::ClapAbi::AudioBuffer input{
                    inputChannels, nullptr, channels, 0, 0};
                Hirari::Core::Plugins::ClapAbi::AudioBuffer output{
                    outputChannels, nullptr, channels, 0, 0};
                const uint32_t midiEvents = std::min<uint32_t>(
                    shared->midiEvents.load(std::memory_order_acquire),
                    Hirari::Core::Plugins::SandboxProtocol::kMaxMidiEvents);
                MidiInputContext midiContext{shared->midi, midiEvents};
                while (midiContext.extendedCount < midiContext.extended.size() &&
                       shared->extendedMidi.pop(midiContext.extended[midiContext.extendedCount])) {
                    ++midiContext.extendedCount;
                }
                midiContext.parameterCount = std::min<uint32_t>(
                    shared->parameterChanges.load(std::memory_order_acquire),
                    Hirari::Core::Plugins::SandboxProtocol::kMaxParameterChanges);
                for (uint32_t parameterIndex = 0;
                     parameterIndex < midiContext.parameterCount; ++parameterIndex) {
                    const auto& source = shared->parameterChange[parameterIndex];
                    auto& destination = midiContext.parameters[parameterIndex];
                    destination.header.size = sizeof(destination);
                    destination.header.time = std::min<uint32_t>(source.sampleOffset, frames);
                    destination.header.space_id = Hirari::Core::Plugins::ClapAbi::kCoreEventSpaceId;
                    destination.header.type = Hirari::Core::Plugins::ClapAbi::kEventParamValue;
                    destination.header.flags = 0;
                    destination.param_id = source.parameterId;
                    destination.cookie = nullptr;
                    destination.value = source.value;
                    destination.note_id = -1;
                    destination.port_index = 0;
                    destination.channel = -1;
                    destination.key = -1;
                }
                auto addInputEvent = [&midiContext](
                    MidiInputContext::EventKind kind, uint32_t index, uint32_t sampleOffset) {
                    if (midiContext.orderedCount < midiContext.ordered.size()) {
                        midiContext.ordered[midiContext.orderedCount++] = {
                            kind, index, sampleOffset};
                    }
                };
                for (uint32_t index = 0; index < midiContext.count; ++index) {
                    addInputEvent(MidiInputContext::EventKind::Midi, index,
                        static_cast<uint32_t>(std::min<uint64_t>(
                            shared->midi[index].sampleOffset, UINT32_MAX)));
                }
                for (uint32_t index = 0; index < midiContext.extendedCount; ++index) {
                    addInputEvent(MidiInputContext::EventKind::Extended, index,
                        static_cast<uint32_t>(std::min<uint64_t>(
                            midiContext.extended[index].sampleOffset, UINT32_MAX)));
                }
                for (uint32_t index = 0; index < midiContext.parameterCount; ++index) {
                    addInputEvent(MidiInputContext::EventKind::Parameter, index,
                        midiContext.parameters[index].header.time);
                }
                // CLAP requires events in nondecreasing sample order. Merge
                // MIDI, SysEx/UMP, and automation without allocating.
                std::sort(midiContext.ordered.begin(),
                          midiContext.ordered.begin() + midiContext.orderedCount,
                          [](const auto& left, const auto& right) {
                              if (left.sampleOffset != right.sampleOffset)
                                  return left.sampleOffset < right.sampleOffset;
                              if (left.kind != right.kind)
                                  return left.kind < right.kind;
                              return left.index < right.index;
                          });
                const Hirari::Core::Plugins::ClapAbi::InputEvents inputEvents{
                    &midiContext, &midiInputSize, &midiInputGet};
                MidiOutputContext midiOutputContext{shared, frames};
                const Hirari::Core::Plugins::ClapAbi::OutputEvents outputEvents{
                    &midiOutputContext, &midiOutputTryPush};
                Hirari::Core::Plugins::ClapAbi::Process process{
                    0, frames, nullptr, &input, 1, &output, 1, &inputEvents,
                    const_cast<Hirari::Core::Plugins::ClapAbi::OutputEvents*>(&outputEvents)};
                const auto processStatus = clapPlugin->process(clapPlugin, &process);
                if (processStatus == Hirari::Core::Plugins::ClapAbi::Error) {
                    std::memset(shared->output, 0, sizeof(shared->output));
                    shared->processErrors.fetch_add(1, std::memory_order_release);
                }
#if defined(__APPLE__)
            } else if (auStarted && !auRuntime.process(*shared, channels, frames)) {
                std::memset(shared->output, 0, sizeof(shared->output));
                shared->processErrors.fetch_add(1, std::memory_order_release);
#endif
#if defined(HIRARI_ENABLE_VST3_SDK)
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
