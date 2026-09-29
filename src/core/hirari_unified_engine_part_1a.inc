inline int64_t hirari_sys(int /*op*/, int64_t /*a1*/, int64_t /*a2*/, int64_t /*a3*/) {
    static std::atomic<uint64_t> state{0x9E3779B97F4A7C15ull};
    uint64_t x = state.fetch_add(0x9E3779B97F4A7C15ull, std::memory_order_relaxed);
    x = (x ^ (x >> 30u)) * 0xBF58476D1CE4E5B9ull;
    x = (x ^ (x >> 27u)) * 0x94D049BB133111EBull;
    x ^= x >> 31u;
    return static_cast<int64_t>(x);
}

// A callback-safe thread identity.  std::hash<std::thread::id> may trigger
// runtime machinery on some standard libraries; this monotonically assigned
// token is allocation-free and is shared by realtime and offline callbacks.
inline uint64_t hirari_callback_thread_token() noexcept {
    static std::atomic<uint64_t> next{1};
    thread_local uint64_t token = 0;
    if (token == 0) token = next.fetch_add(1, std::memory_order_relaxed);
    return token;
}

// Marks the control worker that is intentionally driving an offline bounce.
// The token check below protects the realtime callback from seeing the
// temporary offline graph, while this explicit thread-local gate avoids
// relying on implementation-specific merging of inline-function TLS across
// translation units.
inline thread_local bool hirari_offline_render_thread = false;

using ScopedDenormalGuard = ::Hirari::Core::Concurrency::ScopedDenormalGuard;

// Singleton Implementation
HirariUnifiedEngine& HirariUnifiedEngine::getInstance() {
    static HirariUnifiedEngine instance;
    return instance;
}

HirariUnifiedEngine::~HirariUnifiedEngine() {
    shutdown();
}

void HirariUnifiedEngine::publish_input_monitor_block(const float* const* inputs,
                                                    uint32_t numChannels,
                                                    uint32_t numSamples) noexcept {
    const uint32_t monitorTrack = m_inputMonitorTrackId.load(std::memory_order_acquire);
    const uint64_t channelPair = m_inputMonitorChannelPair.load(std::memory_order_acquire);
    const uint32_t leftChannel = static_cast<uint32_t>(channelPair);
    const uint32_t rightChannel = static_cast<uint32_t>(channelPair >> 32);
    if (!inputs || leftChannel >= numChannels || rightChannel >= numChannels ||
        !inputs[leftChannel] || !inputs[rightChannel] ||
        monitorTrack == kNoInputMonitorTrack || numSamples == 0 ||
        numSamples > kMaxAudioBlockSize) {
        m_inputMonitorBlockPublished.store(false, std::memory_order_release);
        m_inputMonitorBlockFrames.store(0, std::memory_order_release);
        return;
    }
    if (!hirari_audio_copy_monitor_input(
            inputs, numChannels, leftChannel, rightChannel, numSamples,
            m_inputMonitorLeft.data(), m_inputMonitorRight.data())) {
        m_inputMonitorBlockPublished.store(false, std::memory_order_release);
        m_inputMonitorBlockFrames.store(0, std::memory_order_release);
        return;
    }
    m_inputMonitorBlockFrames.store(numSamples, std::memory_order_release);
    m_inputMonitorBlockPublished.store(true, std::memory_order_release);
}

void HirariUnifiedEngine::publish_talkback_input_block(const float* const* inputs,
                                                       uint32_t numChannels,
                                                       uint32_t numSamples) noexcept {
    const uint32_t inputChannel = m_controlRoom.talkbackInputChannel();
    if (!m_controlRoom.talkbackEnabled() || !inputs || inputChannel >= numChannels ||
        !inputs[inputChannel] ||
        numSamples == 0 || numSamples > kMaxAudioBlockSize) {
        m_talkbackInputPublished.store(false, std::memory_order_release);
        m_talkbackInputFrames.store(0, std::memory_order_release);
        return;
    }
    if (!hirari_audio_copy_talkback_input(
            inputs, numChannels, inputChannel, numSamples, m_talkbackInput.data())) {
        m_talkbackInputPublished.store(false, std::memory_order_release);
        m_talkbackInputFrames.store(0, std::memory_order_release);
        return;
    }
    m_talkbackInputFrames.store(numSamples, std::memory_order_release);
    m_talkbackInputPublished.store(true, std::memory_order_release);
}

void HirariUnifiedEngine::apply_config(const EngineConfig& cfg) {
    m_tempoMap.setCurrentBpm(cfg.tempo);
    prepareToPlay(cfg.sampleRate, cfg.blockSize);
}

void HirariUnifiedEngine::shutdown() {
    const bool wasShutdown = m_isShutdown.exchange(true, std::memory_order_acq_rel);
    m_isPlaying.store(false, std::memory_order_release);
    waitForAudioCallbacks();
    // Shutdown must cancel offline work before joining it.  Joining a render
    // that is still publishing blocks can otherwise keep app teardown alive
    // indefinitely and leave its temporary output/worker state behind.
    m_bounceCancelRequested.store(true, std::memory_order_release);
    {
        std::lock_guard<std::mutex> bounceLifecycleLock(m_bounceLifecycleMutex);
        if (m_bounceThread.joinable()) m_bounceThread.join();
        m_bounceRunning.store(false, std::memory_order_release);
    }
    m_recoveryRunning.store(false, std::memory_order_release);
    m_recoveryWake.notify_all();
    if (m_recoveryThread.joinable()) m_recoveryThread.join();

    // Drain waveform-cache jobs while their captured engine state is still
    // valid, then release the worker threads before clearing tracks/cache.
    m_threadPool.stop();

    if (wasShutdown) return;

    std::lock_guard<std::mutex> lock(m_tracksMutex);
    m_tracks.clear();
    mark_project_layout_changed();
    m_deferredDestructionTracks.clear();
    for(int i=0; i<2; ++i) {
        m_audioStates[i].trackCount = 0;
    }
}

HirariUnifiedEngine::HirariUnifiedEngine()
    : m_playhead(0)
    , m_loopEnabled(false) {
    m_masterBuffer.setSize(2, 4096);
    m_context = std::make_unique<::Hirari::DSP::ProcessContext>();
    *m_context = ::Hirari::DSP::ProcessContext{};
    m_context->sampleRate = 44100.0;
    m_context->blockSize = 512;
    m_context->bpm = 120.0;
    m_context->audioConfigGeneration = 1;
    for (auto& state : m_audioStates) state.audioConfigGeneration = 1;
    // All PDC graph edits originate from the engine/control owner. Binding
    // this instance prevents an audio callback from accidentally taking the
    // configuration mutex through a mutator.
    (void)m_pdcManager.bindControlThread();
    m_analyzer = std::make_unique<::Hirari::DSP::Analysis::MasterMeter>();
    m_masterLimiter = std::make_unique<::Hirari::DSP::Effects::MasterLimiter>();
    m_scriptManager = std::make_unique<ScriptManager>();
    // Waveform peak generation is a control-plane workload. Keep a small
    // private worker pool alive for it so the first UI request does not fall
    // back to a synchronous full-file decode on the event thread. The pool
    // is stopped before tracks/cache state is torn down in shutdown().
    m_threadPool.start(2);
    startRecoveryOrchestrator();
}

#include "diagnostics/forensic_journaler.hpp"

void HirariUnifiedEngine::startRecoveryOrchestrator() {
    const char* disabled = std::getenv("HIRARI_DISABLE_RECOVERY_THREAD");
    if (disabled && std::string_view(disabled) == "1") return;
    if (m_recoveryRunning.exchange(true, std::memory_order_acq_rel)) return;
    // Recovery and plugin health checks are control-thread maintenance. They
    // never enter processBlock() and never wait on the audio callback.
    m_forensicJournaler.start("hirari_session_sovereign.journal");
    ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "SYSTEM | RECOVERY | JOURNAL_STARTED");

    m_recoveryThread = std::thread([this]() {
        while (m_recoveryRunning.load(std::memory_order_acquire) &&
               !m_isShutdown.load(std::memory_order_acquire)) {
            const uint32_t recovered = maintain_sandboxed_plugins(true);
            if (recovered > 0) {
                ::Hirari::Core::Diagnostics::LogBuffer::post(
                    0, recovered, "SYSTEM | PLUGIN_SANDBOX | PROCESS_RECOVERED");
            }
            std::unique_lock<std::mutex> lock(m_recoveryWakeMutex);
            m_recoveryWake.wait_for(lock, std::chrono::milliseconds(100), [this] {
                return !m_recoveryRunning.load(std::memory_order_acquire) ||
                       m_isShutdown.load(std::memory_order_acquire);
            });
        }
    });
}



    void HirariUnifiedEngine::processBlock(AudioBuffer& output, uint32_t off, uint32_t sz) {
        if (sz == 0 || output.getNumChannels() < 2 || off >= output.getNumSamples()) {
            if (sz != 0 && off < output.getNumSamples())
                output.clear(off, std::min(sz, output.getNumSamples() - off));
            return;
        }
        if (sz > HirariUnifiedEngine::kMaxAudioBlockSize) {
            output.clear(off, std::min(sz, output.getNumSamples() - off));
            return;
        }
    sz = std::min(sz, output.getNumSamples() - off);
    if (m_audioConfigTransition.load(std::memory_order_acquire) &&
        !hirari_offline_render_thread) {
        output.clear(off, sz);
        return;
    }
        const uint64_t threadToken = hirari_callback_thread_token();
        RoutingEngine::SendPdcReadGuard sendPdcReadGuard(m_routingEngine);
        if (m_offlineRenderActive.load(std::memory_order_acquire) &&
            m_offlineRenderOwner.load(std::memory_order_acquire) != threadToken &&
            !hirari_offline_render_thread) {
            output.clear(off, sz);
            return;
        }
        // Clock reads are control telemetry, not part of the audio contract.
        // Sample them sparsely so the callback does not pay an OS-clock cost
        // on every block.  The last sampled load remains available to the
        // governor between samples.
        const uint32_t callbackIndex = m_diagnosticBlockCounter.fetch_add(
            1, std::memory_order_relaxed);
        const bool sampleCpu = (callbackIndex & 0xffu) == 0u;
        const auto cpuStart = sampleCpu ? std::chrono::steady_clock::now()
                                        : std::chrono::steady_clock::time_point{};
        ScopedDenormalGuard guard;
        if (m_isShutdown.load(std::memory_order_acquire)) {
            output.clear(off, sz);
            return;
        }
        // Hot-swap quiesce: bounded backoff (max 200 x 50µs = 10ms)
        // Tight spin avoided: background daemon clears flag in <5ms by design.
        auto& diagnostics = ::Hirari::Core::Diagnostics::DiagnosticsKernel::getInstance();
        if (diagnostics.isQuiesceRequested()) {
            diagnostics.acknowledgeQuiesce();
            // Never sleep or spin on the audio thread. The next block will see
            // the request again if the structural transaction is still active.
            output.clear(off, sz);
            return;
        }

        // INDUSTRIAL: Sovereign Memory Reset
        ::Hirari::Core::Concurrency::ForensicScratchpad::getThreadLocal().reset();

        uint64_t currentPlayhead = m_playhead.load(std::memory_order_relaxed);
        const uint64_t cycleStart = m_cycleStart.load(std::memory_order_acquire);
        const uint64_t cycleEnd = m_cycleEnd.load(std::memory_order_acquire);
        if (m_loopEnabled.load(std::memory_order_acquire)
            && cycleEnd > cycleStart
            && currentPlayhead >= cycleEnd) {
            currentPlayhead = cycleStart;
            m_playhead.store(cycleStart, std::memory_order_relaxed);
        }
        const uint64_t callbackGeneration = m_audioConfigGeneration.load(std::memory_order_acquire);
        const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
        if (state.audioConfigGeneration != callbackGeneration && !hirari_offline_render_thread) {
            // Structural/configuration publication is double-buffered.  Do
            // not process a state prepared for a different device format;
            // the next published state will carry the current generation.
            output.clear(off, sz);
            return;
        }
        if (m_previewVoicesNeedReset.exchange(false, std::memory_order_acq_rel)) {
            m_previewSynth.reset();
            for (auto& channel : m_previewFallbackActiveNotes)
                std::fill(std::begin(channel), std::end(channel), 0u);
        }
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            if (state.activeTracks[i]) state.activeTracks[i]->clearMidiInput();
        }
        Command command{};
        while (m_commandQueue.pop(command)) {
            // A command may have waited in the lock-free queue while the
            // control thread rebuilt the audio state. Never apply it to a
            // different device configuration.
            if (command.expectedAudioGeneration != callbackGeneration) continue;
            if (command.tid >= kMaxTrackMap) continue;
            Track* target = state.trackMap[command.tid];
            if (!target) continue;
            switch (command.type) {
                case CommandType::SetVolume:
                    if (std::isfinite(command.val)) target->setVolume(std::clamp(command.val, 0.0f, 2.0f));
                    break;
                case CommandType::SetPan:
                    if (std::isfinite(command.val)) target->setPan(std::clamp(command.val, -1.0f, 1.0f));
                    break;
                case CommandType::SetMute:
                    target->setMuted(command.val >= 0.5f);
                    break;
                case CommandType::SetSolo:
                    target->setSolo(command.val >= 0.5f);
                    break;
                default:
                    break;
            }
        }
        // Keep the device callback primed while stopped, but do not traverse
        // the full graph when no preview or offline render owns the path.
        // Command mutations are deliberately drained first so fader, pan,
        // mute, and solo edits remain visible immediately while stopped.
        const bool previewActive =
            m_previewSampleTrigger.load(std::memory_order_acquire) ||
            m_previewSamplePlaying.load(std::memory_order_acquire);
        const bool midiPanicPending = m_midiPanicRequested.load(std::memory_order_acquire);
        if (!hirari_offline_render_thread &&
            !m_offlineRenderActive.load(std::memory_order_acquire) &&
            !m_isPlaying.load(std::memory_order_acquire) &&
            !m_testTone.load(std::memory_order_relaxed) &&
            !previewActive && !midiPanicPending) {
            output.clear(off, sz);
            return;
        }
        output.clear(off, sz);
        m_blockMidi.clear();
        {
            const bool midiPlaybackActive = hirari_offline_render_thread ||
                m_offlineRenderActive.load(std::memory_order_acquire) ||
                m_isPlaying.load(std::memory_order_acquire);
            if (currentPlayhead > std::numeric_limits<uint64_t>::max() - sz) {
                output.clear(off, sz);
                m_audioRangeOverflow.store(true, std::memory_order_release);
                return;
            }
            uint64_t midiSnapshotRevision = 0;
            uint32_t midiSnapshotIndex = 0;
            bool midiSnapshotPinned = false;
            for (uint32_t attempt = 0; attempt < 8; ++attempt) {
                const uint64_t beforeRevision =
                    m_midiNoteSnapshotRevision.load(std::memory_order_acquire);
                if ((beforeRevision & 1u) != 0u) continue;
                const uint32_t candidate =
                    m_activeMidiSnapshot.load(std::memory_order_acquire);
                m_midiSnapshotReaders[candidate].fetch_add(1, std::memory_order_acq_rel);
                const uint32_t afterIndex =
                    m_activeMidiSnapshot.load(std::memory_order_acquire);
                const uint64_t afterRevision =
                    m_midiNoteSnapshotRevision.load(std::memory_order_acquire);
                if (candidate == afterIndex && beforeRevision == afterRevision &&
                    (afterRevision & 1u) == 0u) {
                    midiSnapshotIndex = candidate;
                    midiSnapshotRevision = afterRevision;
                    midiSnapshotPinned = true;
                    break;
                }
                m_midiSnapshotReaders[candidate].fetch_sub(1, std::memory_order_release);
            }
            const bool discontinuity = midiPlaybackActive &&
                (!m_hasPreviousMidiBlock || !m_previousMidiPlaybackActive ||
                 currentPlayhead != m_previousMidiBlockEnd);
            if (discontinuity) ++m_midiPlaybackPass;
            const bool snapshotChanged = midiSnapshotPinned && m_hasPreviousMidiBlock &&
                midiSnapshotRevision != m_previousMidiSnapshotRevision;
            const bool requestedMidiPanic =
                m_midiPanicRequested.exchange(false, std::memory_order_acq_rel);
            const bool reconcileMidiState =
                discontinuity || snapshotChanged || requestedMidiPanic ||
                !midiSnapshotPinned;
            const bool panicMidi = reconcileMidiState;
            if (panicMidi) {
                m_previewSynth.reset();
                for (auto& channel : m_previewFallbackActiveNotes)
                    std::fill(std::begin(channel), std::end(channel), 0u);
                for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
                    Track* track = state.activeTracks[i];
                    if (track && (track->getType() == Track::Midi ||
                                  track->getType() == Track::Instrument))
                        track->addMidiInputAllNotesOff(0);
                }
            }
            const uint64_t blockEnd = currentPlayhead + sz;
            if (midiSnapshotPinned) {
                const auto& snapshot = m_midiNoteSnapshots[midiSnapshotIndex];
                using AudioState = std::remove_reference_t<decltype(state)>;
                struct MidiDispatchContext {
                    HirariUnifiedEngine* engine;
                    AudioState* state;
                    uint64_t playhead;
                } dispatch{this, &state, currentPlayhead};
                hirari_midi_schedule_block(
                    reinterpret_cast<const HirariScheduledMidiNote*>(snapshot.notesByStart.data()),
                    snapshot.notesByStart.size(), snapshot.noteIndicesByEnd.data(),
                    snapshot.noteIndicesByEnd.size(), currentPlayhead, sz,
                    midiPlaybackActive ? 1 : 0, reconcileMidiState ? 1 : 0,
                    m_midiPlaybackPass, &dispatch,
                    +[](void* opaque, const HirariScheduledMidiNote* rawNote,
                        uint64_t offset, uint8_t noteOff) {
                        auto& context = *static_cast<MidiDispatchContext*>(opaque);
                        auto& engine = *context.engine;
                        const auto& note = *reinterpret_cast<const ScheduledMidiNote*>(rawNote);
                        const auto channel = static_cast<uint8_t>(note.midiChannel + 1);
                        Track* target = note.trackId < kMaxTrackMap
                            ? context.state->trackMap[note.trackId] : nullptr;
                        if (noteOff != 0) {
                            if (target)
                                target->addMidiInputNoteOff(channel, note.pitch, offset,
                                                            note.articulationId);
                            auto& fallback = engine.m_previewFallbackActiveNotes
                                [note.midiChannel][note.pitch];
                            if (fallback > 0 && engine.m_blockMidi.remainingCapacity() > 0) {
                                engine.m_blockMidi.addNoteOff(channel, note.pitch, offset);
                                --fallback;
                            }
                            return;
                        }
                        if (target) {
                            const double sampleRate = std::max(
                                1.0, engine.m_sampleRate.load(std::memory_order_relaxed));
                            const double startBeat = engine.m_tempoMap.samplesToBeats(
                                note.startSample, sampleRate);
                            target->forEachTimedTransitionKeySwitchOutput(
                                note.articulationId,
                                [&engine, target, startBeat, sampleRate, offset](
                                    uint8_t outputChannel, uint8_t outputPitch,
                                    uint32_t lengthTicks) {
                                    const double endBeat = startBeat +
                                        static_cast<double>(lengthTicks) / 960.0;
                                    const uint64_t endSample = engine.m_tempoMap.beatsToSamples(
                                        endBeat, sampleRate);
                                    (void)target->scheduleExpressionNoteOff(
                                        outputChannel, outputPitch, endSample, offset);
                                });
                            target->addMidiInputNoteOn(channel, note.pitch, note.velocity,
                                                       offset, note.articulationId);
                            target->forEachTimedKeySwitchOutput(
                                note.articulationId,
                                [&engine, target, startBeat, sampleRate, offset](
                                    uint8_t outputChannel, uint8_t outputPitch,
                                    uint32_t lengthTicks) {
                                    const double endBeat = startBeat +
                                        static_cast<double>(lengthTicks) / 960.0;
                                    const uint64_t endSample = engine.m_tempoMap.beatsToSamples(
                                        endBeat, sampleRate);
                                    (void)target->scheduleExpressionNoteOff(
                                        outputChannel, outputPitch, endSample, offset);
                                });
                        } else if (engine.m_blockMidi.remainingCapacity() > 0) {
                            engine.m_blockMidi.addNoteOn(channel, note.pitch, note.velocity, offset);
                            ++engine.m_previewFallbackActiveNotes[note.midiChannel][note.pitch];
                        }
                    });
                m_midiSnapshotReaders[midiSnapshotIndex].fetch_sub(
                    1, std::memory_order_release);
            } else {
                // Under sustained concurrent edits, never risk traversing a
                // mutable snapshot. Panic this block and reconcile on the
                // next callback after a stable slot can be pinned.
                m_midiPanicRequested.store(true, std::memory_order_release);
            }
            m_blockMidi.sort();
            m_previousMidiPlaybackActive = midiSnapshotPinned && midiPlaybackActive;
            m_previousMidiBlockEnd = blockEnd;
            if (midiSnapshotPinned)
                m_previousMidiSnapshotRevision = midiSnapshotRevision;
            m_hasPreviousMidiBlock = true;
        }
        m_midiOrchestrator.processBlock(m_blockMidi);

        // Deterministic built-in preview instrument. This is intentionally a
        // small built-in voice so MIDI playback can be verified without plugins.
        const double sr = std::max(1.0, m_sampleRate.load(std::memory_order_relaxed));
        const auto& macro = m_macroControlManager;
        const float morph = macro.getMacroValue(0);
        const double detuneRatio = std::pow(2.0, (static_cast<double>(macro.getMacroValue(1)) - 0.5) * 40.0 / 1200.0);
        const float drive = 0.75f + macro.getMacroValue(2) * 3.0f;
        const float cutoff = 0.01f + macro.getMacroValue(3) * 0.35f;
        const float resonance = macro.getMacroValue(4) * 0.25f;
        const float synthGain = 0.25f + macro.getMacroValue(5) * 1.25f;
        m_previewSynth.process(m_blockMidi.getEvents(), m_blockMidi.size(),
            output.getWritePointer(0, off), output.getWritePointer(1, off), sz,
            currentPlayhead, sr, morph, detuneRatio, drive, cutoff, resonance, synthGain);
        float* previewL = output.getWritePointer(0, off);
        float* previewR = output.getWritePointer(1, off);

        // One-shot preview sampler. The immutable snapshot is published by
        // the control thread; only the cursor is mutated on the audio thread.
        const PreviewSample* previewSample =
            m_previewSample.load(std::memory_order_acquire);
        if (m_previewSampleTrigger.exchange(false, std::memory_order_acq_rel)) {
            m_previewSamplePosition = 0.0;
            m_previewSamplePlaying.store(true, std::memory_order_release);
        }

        if (previewSample && m_previewSamplePlaying.load(std::memory_order_acquire) &&
            !previewSample->samples.empty()) {
            const bool stillPlaying = hirari_preview_sample_render(
                previewSample->samples.data(), previewSample->samples.size(),
                previewSample->sourceRate, sr, previewL, previewR, sz,
                &m_previewSamplePosition);
            m_previewSamplePlaying.store(stillPlaying, std::memory_order_release);
        } else {
            m_previewSamplePosition = 0.0;
            m_previewSamplePlaying.store(false, std::memory_order_release);
        }

        if (m_testTone.load(std::memory_order_relaxed)) {
            const double sr = m_sampleRate.load(std::memory_order_relaxed);
            (void)hirari_audio_test_tone_render(
                output.getWritePointer(0, off), output.getWritePointer(1, off),
                sz, currentPlayhead, sr);
        }

        // INDUSTRIAL: HYBRID BUFFERING LOOP
        // If the hardware buffer (sz) is smaller than the optimized engine block (1024),
        // we would typically use an internal FIFO here. For the industrial foundation,
        // we ensure the processing logic is decoupled from the 'sz' parameter.

        bool anySolo = false;
        for (uint32_t i = 0; i < state.trackCount; ++i) {
            if (state.activeTracks[i] && state.activeTracks[i]->isSolo()) {
                anySolo = true;
                break;
            }
        }
        const bool offlineTargetActive = m_offlineTargetActive.load(std::memory_order_acquire);
        const uint32_t offlineTargetId = m_offlineTargetId.load(std::memory_order_acquire);
        auto& routing = m_routingEngine;
        m_busSystem.clear(sz);
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            if (state.activeTracks[i] && state.activeTracks[i]->canProcess(sz)) {
                state.activeTracks[i]->setTransportState(
                    m_tempoMap.currentBpm(),
                    m_isPlaying.load(std::memory_order_relaxed));
                state.activeTracks[i]->clearWorkBuffer(sz);
            }
        }

        auto findTrack = [&](uint32_t id) -> Track* {
            for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
                if (state.activeTracks[i] && state.activeTracks[i]->getId() == id) {
                    return state.activeTracks[i];
                }
            }
            return nullptr;
        };

        auto shouldProcess = [&](Track* track) noexcept {
            if (!track) return false;
            return routing.shouldProcessNode(track->getId(), offlineTargetActive,
                                             offlineTargetId, anySolo, track->isSolo());
        };

        // Feedback edges are explicitly delayed by one engine block. They are
        // injected before any graph node runs, so a cycle cannot recurse on
        // the current block or allocate on the audio thread.
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            Track* track = state.activeTracks[i];
            if (!track || !track->canProcess(sz)) continue;
            auto& work = track->getWorkBuffer(sz);
            routing.injectFeedback(track->getId(), work.getWritePointer(0),
                                   work.getWritePointer(1), sz);
        }

        // Feed the live driver input into the selected monitor track before
        // graph processing. The fixed planar buffers were copied by the
        // driver callback, so this path performs no allocation or locking.
        if (m_inputMonitorBlockPublished.load(std::memory_order_acquire)) {
            const uint32_t monitorTrackId =
                m_inputMonitorTrackId.load(std::memory_order_acquire);
            const uint32_t publishedFrames =
                m_inputMonitorBlockFrames.load(std::memory_order_acquire);
            const uint32_t inputOffset = m_inputMonitorBlockOffset;
            if (monitorTrackId != kNoInputMonitorTrack && inputOffset < publishedFrames &&
                publishedFrames - inputOffset >= sz) {
                float* inputChannels[2] = {
                    m_inputMonitorLeft.data() + inputOffset,
                    m_inputMonitorRight.data() + inputOffset};
                AudioBuffer input;
                input.wrapChannels(inputChannels, 2, sz);
                if (Track* monitorTrack = findTrack(monitorTrackId);
                    monitorTrack && monitorTrack->canProcess(sz)) {
                    monitorTrack->addInput(input, sz);
                }
            }
        }

        // One callback-local scratch pair is reused for each sequential Send
        // edge. Delay histories live in RoutingEngine and no buffer is
        // allocated while the audio callback is running.
        std::array<float, Hirari::Core::Engine::Bus::kMaxSamples> sendPdcLeft;
        std::array<float, Hirari::Core::Engine::Bus::kMaxSamples> sendPdcRight;
        std::array<HirariRoutingTrackDestination, kMaxTracks> routingDestinations{};
        size_t routingDestinationCount = 0;
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            Track* track = state.activeTracks[i];
            if (!track || !track->canProcess(sz)) continue;
            auto& work = track->getWorkBuffer(sz);
            auto* busTrack = dynamic_cast<BusTrack*>(track);
            routingDestinations[routingDestinationCount++] = {
                track->getId(),
                static_cast<uint8_t>(shouldProcess(track)),
                static_cast<uint8_t>(busTrack != nullptr),
                work.getNumChannels(),
                work.getArrayOfWritePointers(),
                busTrack};
        }
        const auto addBusPreFx = +[](void* context, const float* left, const float* right,
                                     uint32_t frames, float gain) -> bool {
            auto* bus = static_cast<BusTrack*>(context);
            return bus && bus->addPreFxInput(left, right, frames, gain);
        };
        auto routeToNextStages = [&](Track* source) {
            if (!source || !source->canProcess(sz)) return;
            auto& sourceBuffer = source->getWorkBuffer(sz);
            // Cue buses are isolated monitor sources. Their Send inputs and
            // insert chain run normally, but their signal is selected only at
            // the Control Room boundary, never summed into the Main mix.
            if (!offlineTargetActive && m_controlRoom.isCueBus(source->getId())) return;
            if (offlineTargetActive && offlineTargetId != 0 &&
                source->getId() == offlineTargetId) {
                const auto& targetBuffer = m_offlineRenderPreFader.load(std::memory_order_acquire)
                    ? source->offlinePreFaderBuffer()
                    : (!m_offlineRenderIncludeInserts.load(std::memory_order_acquire)
                        ? source->offlinePreInsertBuffer() : sourceBuffer);
                if (targetBuffer.getNumSamples() < sz || targetBuffer.getNumChannels() < 2) return;
                const float* sourceL = targetBuffer.getReadPointer(0, 0);
                const float* sourceR = targetBuffer.getReadPointer(1, 0);
                float* destL = output.getWritePointer(0, off);
                float* destR = output.getWritePointer(1, off);
                hirari_audio_add_sanitized_stereo(destL, destR, sourceL, sourceR, sz);
                return;
            }
            const auto& sendPre = source->sendPreFaderBuffer();
            const auto& sendPost = source->sendPostFaderBuffer();
            const float* sendPreLeft = sendPre.getNumChannels() >= 2 && sendPre.getNumSamples() >= sz
                ? sendPre.getReadPointer(0, 0) : nullptr;
            const float* sendPreRight = sendPre.getNumChannels() >= 2 && sendPre.getNumSamples() >= sz
                ? sendPre.getReadPointer(1, 0) : nullptr;
            const float* sendPostLeft = sendPost.getNumChannels() >= 2 && sendPost.getNumSamples() >= sz
                ? sendPost.getReadPointer(0, 0) : nullptr;
            const float* sendPostRight = sendPost.getNumChannels() >= 2 && sendPost.getNumSamples() >= sz
                ? sendPost.getReadPointer(1, 0) : nullptr;
            hirari_routing_process_track_fanout(
                routing.nativeGainState(), routing.nativeSendPdcState(), source->getId(),
                sourceBuffer.getArrayOfReadPointers(), sourceBuffer.getNumChannels(),
                sendPreLeft, sendPreRight, sendPostLeft, sendPostRight,
                source->getTrackDelaySamples(), routingDestinations.data(),
                routingDestinationCount, output.getWritePointer(0, off),
                output.getWritePointer(1, off), sendPdcLeft.data(), sendPdcRight.data(),
                sz, Hirari::Core::Engine::Bus::kMaxSamples, addBusPreFx);
        };

        auto publishBusIfNeeded = [&](Track* track) {
            if (auto* busTrack = dynamic_cast<BusTrack*>(track)) {
                busTrack->publishPostFx(sz);
            }
        };

        std::array<uint32_t, kMaxTracks> activeTrackIds{};
        size_t activeTrackCount = 0;
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            if (state.activeTracks[i]) activeTrackIds[activeTrackCount++] = state.activeTracks[i]->getId();
        }
        std::array<uint32_t, kMaxTracks + Hirari::Core::Engine::RoutingEngine::kMaxNodes>
            processOrder{};
        uint32_t processOrderCount = 0;
        if (!hirari_routing_topology_resolve_process_order(
                routing.nativeTopologyState(), activeTrackIds.data(), activeTrackCount,
                processOrder.data(), processOrder.size(), &processOrderCount)) {
            processOrderCount = static_cast<uint32_t>(activeTrackCount);
            std::copy_n(activeTrackIds.begin(), activeTrackCount, processOrder.begin());
        }
        for (uint32_t n = 0; n < processOrderCount; ++n) {
            Track* track = findTrack(processOrder[n]);
            if (!shouldProcess(track) || !track->canProcess(sz)) continue;
            if (auto* busTrack = dynamic_cast<BusTrack*>(track)) {
                busTrack->loadPreFxIntoWork(sz);
            }
            track->processAccumulated(sz, currentPlayhead);
            publishBusIfNeeded(track);
            {
                const auto& source = track->getWorkBuffer(sz);
                const auto& preFx = track->preFxSidechainBuffer();
                const auto& postFx = track->postFxSidechainBuffer();
                m_sidechainManager.publishAudioSourceBlock(
                    track->getId(), preFx.getReadPointer(0), preFx.getReadPointer(1),
                    postFx.getReadPointer(0), postFx.getReadPointer(1),
                    source.getReadPointer(0), source.getReadPointer(1),
                    sz, static_cast<uint32_t>(std::max(
                        1.0, m_sampleRate.load(std::memory_order_acquire))),
                    callbackGeneration);
            }
            routeToNextStages(track);
        }
