inline int64_t aura_sys(int /*op*/, int64_t /*a1*/, int64_t /*a2*/, int64_t /*a3*/) {
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
inline uint64_t aura_callback_thread_token() noexcept {
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
inline thread_local bool aura_offline_render_thread = false;

using ScopedDenormalGuard = ::Aura::Core::Concurrency::ScopedDenormalGuard;

// Singleton Implementation
AuraUnifiedEngine& AuraUnifiedEngine::getInstance() {
    static AuraUnifiedEngine instance;
    return instance;
}

AuraUnifiedEngine::~AuraUnifiedEngine() {
    shutdown();
}

void AuraUnifiedEngine::publish_input_monitor_block(const float* const* inputs,
                                                    uint32_t numChannels,
                                                    uint32_t numSamples) noexcept {
    const uint32_t monitorTrack = m_inputMonitorTrackId.load(std::memory_order_acquire);
    if (!inputs || numChannels < 2 || !inputs[0] || !inputs[1] ||
        monitorTrack == kNoInputMonitorTrack || numSamples == 0 ||
        numSamples > kMaxAudioBlockSize) {
        m_inputMonitorBlockPublished.store(false, std::memory_order_release);
        m_inputMonitorBlockFrames.store(0, std::memory_order_release);
        return;
    }
    for (uint32_t i = 0; i < numSamples; ++i) {
        const float left = inputs[0][i];
        const float right = inputs[1][i];
        m_inputMonitorLeft[i] = std::isfinite(left) ? left : 0.0f;
        m_inputMonitorRight[i] = std::isfinite(right) ? right : 0.0f;
    }
    m_inputMonitorBlockFrames.store(numSamples, std::memory_order_release);
    m_inputMonitorBlockPublished.store(true, std::memory_order_release);
}

void AuraUnifiedEngine::apply_config(const EngineConfig& cfg) {
    m_tempo.store(cfg.tempo);
    prepareToPlay(cfg.sampleRate, cfg.blockSize);
}

void AuraUnifiedEngine::shutdown() {
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
    m_deferredDestructionTracks.clear();
    for(int i=0; i<2; ++i) {
        m_audioStates[i].trackCount = 0;
    }
}

AuraUnifiedEngine::AuraUnifiedEngine() 
    : m_playhead(0)
    , m_loopEnabled(false) {
    m_masterBuffer.setSize(2, 4096); 
    m_context = std::make_unique<::Aura::DSP::ProcessContext>();
    *m_context = ::Aura::DSP::ProcessContext{};
    m_context->sampleRate = 44100.0;
    m_context->blockSize = 512;
    m_context->bpm = 120.0;
    m_context->audioConfigGeneration = 1;
    for (auto& state : m_audioStates) state.audioConfigGeneration = 1;
    // All PDC graph edits originate from the engine/control owner. Binding
    // this instance prevents an audio callback from accidentally taking the
    // configuration mutex through a mutator.
    (void)m_pdcManager.bindControlThread();
    m_analyzer = std::make_unique<::Aura::DSP::Analysis::MasterMeter>();
    m_masterLimiter = std::make_unique<::Aura::DSP::Effects::MasterLimiter>();
    m_scriptManager = std::make_unique<ScriptManager>();
    m_processGraph = std::make_unique<ProcessGraph>();
    // Waveform peak generation is a control-plane workload. Keep a small
    // private worker pool alive for it so the first UI request does not fall
    // back to a synchronous full-file decode on the event thread. The pool
    // is stopped before tracks/cache state is torn down in shutdown().
    m_threadPool.start(2);
    startRecoveryOrchestrator();
}

#include "diagnostics/forensic_journaler.hpp"

void AuraUnifiedEngine::startRecoveryOrchestrator() {
    const char* disabled = std::getenv("AURA_DISABLE_RECOVERY_THREAD");
    if (disabled && std::string_view(disabled) == "1") return;
    if (m_recoveryRunning.exchange(true, std::memory_order_acq_rel)) return;
    // Recovery and plugin health checks are control-thread maintenance. They
    // never enter processBlock() and never wait on the audio callback.
    m_forensicJournaler.start("aura_session_sovereign.journal");
    ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "SYSTEM | RECOVERY | JOURNAL_STARTED");

    m_recoveryThread = std::thread([this]() {
        while (m_recoveryRunning.load(std::memory_order_acquire) &&
               !m_isShutdown.load(std::memory_order_acquire)) {
            const uint32_t recovered = maintain_sandboxed_plugins(true);
            if (recovered > 0) {
                ::Aura::Core::Diagnostics::LogBuffer::post(
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



    void AuraUnifiedEngine::processBlock(AudioBuffer& output, uint32_t off, uint32_t sz) {
        if (sz == 0 || output.getNumChannels() < 2 || off >= output.getNumSamples()) {
            if (sz != 0 && off < output.getNumSamples())
                output.clear(off, std::min(sz, output.getNumSamples() - off));
            return;
        }
        if (sz > AuraUnifiedEngine::kMaxAudioBlockSize) {
            output.clear(off, std::min(sz, output.getNumSamples() - off));
            return;
        }
    sz = std::min(sz, output.getNumSamples() - off);
    if (m_audioConfigTransition.load(std::memory_order_acquire) &&
        !aura_offline_render_thread) {
        output.clear(off, sz);
        return;
    }
        const uint64_t threadToken = aura_callback_thread_token();
        if (m_offlineRenderActive.load(std::memory_order_acquire) &&
            m_offlineRenderOwner.load(std::memory_order_acquire) != threadToken &&
            !aura_offline_render_thread) {
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
        auto& diagnostics = ::Aura::Core::Diagnostics::DiagnosticsKernel::getInstance();
        if (diagnostics.isQuiesceRequested()) {
            diagnostics.acknowledgeQuiesce();
            // Never sleep or spin on the audio thread. The next block will see
            // the request again if the structural transaction is still active.
            output.clear(off, sz);
            return;
        }

        // INDUSTRIAL: Sovereign Memory Reset
        ::Aura::Core::Concurrency::ForensicScratchpad::getThreadLocal().reset();

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
        if (state.audioConfigGeneration != callbackGeneration && !aura_offline_render_thread) {
            // Structural/configuration publication is double-buffered.  Do
            // not process a state prepared for a different device format;
            // the next published state will carry the current generation.
            output.clear(off, sz);
            return;
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
        if (!aura_offline_render_thread &&
            !m_offlineRenderActive.load(std::memory_order_acquire) &&
            !m_isPlaying.load(std::memory_order_acquire) &&
            !m_testTone.load(std::memory_order_relaxed) &&
            m_previewSample.load(std::memory_order_acquire) == nullptr) {
            output.clear(off, sz);
            return;
        }
        output.clear(off, sz);
        m_blockMidi.clear();
        {
            const auto& scheduledNotes = m_midiNoteSnapshots[
                m_activeMidiSnapshot.load(std::memory_order_acquire)];
            if (currentPlayhead > std::numeric_limits<uint64_t>::max() - sz) {
                output.clear(off, sz);
                m_audioRangeOverflow.store(true, std::memory_order_release);
                return;
            }
            const uint64_t blockEnd = currentPlayhead + sz;
            for (const auto& note : scheduledNotes) {
                const uint64_t noteEnd = note.lengthSamples >
                    std::numeric_limits<uint64_t>::max() - note.startSample
                    ? std::numeric_limits<uint64_t>::max()
                    : note.startSample + note.lengthSamples;
                if (note.startSample >= currentPlayhead && note.startSample < blockEnd) {
                    m_blockMidi.addNoteOn(1, note.pitch, note.velocity, note.startSample - currentPlayhead);
                }
                if (noteEnd > currentPlayhead && noteEnd < blockEnd) {
                    m_blockMidi.addNoteOff(1, note.pitch, noteEnd - currentPlayhead);
                }
            }
            m_blockMidi.sort();
        }
        m_midiOrchestrator.processBlock(m_blockMidi);

        // Deterministic built-in preview instrument. This is intentionally a
        // small built-in voice so MIDI playback can be verified without plugins.
        const double sr = std::max(1.0, m_sampleRate.load(std::memory_order_relaxed));
        const auto& macro = m_macroControlManager;
        const uint32_t synthEngine = m_previewSynthEngine.load(std::memory_order_acquire);
        const float morph = macro.getMacroValue(0);
        const double detuneRatio = std::pow(2.0, (static_cast<double>(macro.getMacroValue(1)) - 0.5) * 40.0 / 1200.0);
        const float drive = 0.75f + macro.getMacroValue(2) * 3.0f;
        const float cutoff = 0.01f + macro.getMacroValue(3) * 0.35f;
        const float resonance = macro.getMacroValue(4) * 0.25f;
        const float synthGain = 0.25f + macro.getMacroValue(5) * 1.25f;
        float* previewL = output.getWritePointer(0, off);
        float* previewR = output.getWritePointer(1, off);
        // MIDI is sorted by sample offset above. Consume it with a monotonic
        // cursor instead of scanning the entire event array for every sample;
        // dense controller/key-switch streams therefore remain O(frames +
        // events) on the realtime thread.
        size_t midiEventIndex = 0;
        for (uint32_t i = 0; i < sz; ++i) {
            while (midiEventIndex < m_blockMidi.size() &&
                   m_blockMidi.getEvents()[midiEventIndex].sampleOffset <= i) {
                const auto& event = m_blockMidi.getEvents()[midiEventIndex++];
                if (event.sampleOffset != i || event.size < 3) continue;
                const uint8_t status = event.data[0] & 0xF0;
                const uint8_t pitch = event.data[1] & 0x7F;
                if (status == 0x90 && event.data[2] > 0) {
                    auto* voice = std::find_if(std::begin(m_previewVoices), std::end(m_previewVoices),
                        [](const PreviewVoice& v) { return !v.active; });
                    if (voice == std::end(m_previewVoices)) voice = std::begin(m_previewVoices);
                    voice->active = true;
                    voice->pitch = pitch;
                    // Keep the built-in preview instrument at a usable monitor
                    // level.  The previous 0.12 scalar produced renders around
                    // -49 dBFS with the default macro settings, which is
                    // effectively silent on real speakers even though MIDI was
                    // being scheduled correctly.
                    voice->level = static_cast<float>(event.data[2]) / 127.0f * 0.65f;
                    voice->phase = 0.0;
                    voice->releaseAt = std::numeric_limits<uint64_t>::max();
                    voice->startedAt = currentPlayhead + i;
                } else if (status == 0x80 || (status == 0x90 && event.data[2] == 0)) {
                    // Sustain overlapping same-pitch notes independently:
                    // one MIDI note-off releases the oldest still-held voice,
                    // not every voice sharing that pitch.
                    auto* oldest = static_cast<PreviewVoice*>(nullptr);
                    for (auto& voice : m_previewVoices) {
                        if (!voice.active || voice.pitch != pitch ||
                            voice.releaseAt != std::numeric_limits<uint64_t>::max()) continue;
                        if (oldest == nullptr || voice.startedAt < oldest->startedAt)
                            oldest = &voice;
                    }
                    if (oldest != nullptr) oldest->releaseAt = currentPlayhead + i;
                }
            }
            float sample = 0.0f;
            for (auto& voice : m_previewVoices) {
                if (!voice.active) continue;
                if (currentPlayhead + i >= voice.releaseAt) {
                    voice.level *= 0.995f;
                    if (voice.level < 0.0005f) { voice.active = false; continue; }
                }
                const double freq = 440.0 * std::pow(2.0, (static_cast<int>(voice.pitch) - 69) / 12.0) * detuneRatio;
                const float sine = static_cast<float>(std::sin(voice.phase));
                const float normalizedPhase = static_cast<float>(std::fmod(voice.phase + 2.0 * M_PI, 2.0 * M_PI) / (2.0 * M_PI));
                const float saw = normalizedPhase * 2.0f - 1.0f;
                float oscillator = sine;
                if (synthEngine == 1) {
                    oscillator = sine * (1.0f - morph) + saw * morph;
                } else if (synthEngine == 2) {
                    const float triangle = 1.0f - 4.0f * std::abs(normalizedPhase - 0.5f);
                    oscillator = triangle * (1.0f - morph) + saw * morph;
                }
                sample += oscillator * voice.level;
                voice.phase += 2.0 * M_PI * freq / sr;
                if (voice.phase >= 2.0 * M_PI) voice.phase = std::fmod(voice.phase, 2.0 * M_PI);
            }
            const float driven = std::tanh(sample * drive);
            m_previewFilterState += cutoff * (driven - m_previewFilterState);
            sample = (m_previewFilterState + resonance * (driven - m_previewFilterState)) * synthGain;
            previewL[i] += sample;
            previewR[i] += sample;
        }

        // One-shot preview sampler. The immutable snapshot is published by
        // the control thread; only the cursor is mutated on the audio thread.
        const PreviewSample* previewSample =
            m_previewSample.load(std::memory_order_acquire);
        if (m_previewSampleTrigger.exchange(false, std::memory_order_acq_rel)) {
            m_previewSamplePosition = 0.0;
        }

        if (previewSample && !previewSample->samples.empty()) {
            const double sourceRate = std::isfinite(previewSample->sourceRate)
                ? std::max(1.0, previewSample->sourceRate)
                : sr;
            const double step = sourceRate / sr;
            for (uint32_t i = 0; i < sz && m_previewSamplePosition < previewSample->samples.size(); ++i) {
                const size_t index = static_cast<size_t>(m_previewSamplePosition);
                const float a = previewSample->samples[index];
                const float b = index + 1 < previewSample->samples.size()
                    ? previewSample->samples[index + 1]
                    : 0.0f;
                const float frac = static_cast<float>(m_previewSamplePosition - index);
                const float sample = (a + (b - a) * frac) * 0.8f;
                previewL[i] += sample;
                previewR[i] += sample;
                m_previewSamplePosition += step;
            }
            if (m_previewSamplePosition >= previewSample->samples.size()) {
                m_previewSamplePosition = 0.0;
            }
        } else {
            m_previewSamplePosition = 0.0;
        }

        if (m_testTone.load(std::memory_order_relaxed)) {
            const double sr = m_sampleRate.load(std::memory_order_relaxed);
            for (uint32_t i = 0; i < sz; ++i) {
                const double phase = (static_cast<double>(currentPlayhead + i) * 440.0 * 2.0 * M_PI) / sr;
                const float sample = static_cast<float>(std::sin(phase) * 0.15);
                output.getWritePointer(0, off)[i] = sample;
                output.getWritePointer(1, off)[i] = sample;
            }
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
        std::array<bool, kMaxTracks> processed{};
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            if (state.activeTracks[i] && state.activeTracks[i]->canProcess(sz)) {
                state.activeTracks[i]->setTransportState(
                    m_tempo.load(std::memory_order_relaxed),
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

        // A bus stem must include every upstream node without mutating user
        // solo state. The fixed-size breadth-first walk is intentionally
        // bounded and uses the published gain matrix, so it is safe on the
        // audio callback and cannot allocate while a render is running.
        auto reachesOfflineTarget = [&](uint32_t sourceId) noexcept {
            if (!offlineTargetActive || offlineTargetId == 0) return true;
            if (sourceId == offlineTargetId) return true;
            std::array<uint32_t, RoutingEngine::kMaxNodes> queue{};
            std::array<bool, RoutingEngine::kMaxNodes> visited{};
            uint32_t head = 0;
            uint32_t tail = 0;
            if (sourceId >= RoutingEngine::kMaxNodes) return false;
            queue[tail++] = sourceId;
            visited[sourceId] = true;
            while (head < tail) {
                const uint32_t current = queue[head++];
                for (uint32_t candidate = 0; candidate < RoutingEngine::kMaxNodes; ++candidate) {
                    if (routing.connectionGain(current, candidate) <= 0.0f || visited[candidate]) continue;
                    if (candidate == offlineTargetId) return true;
                    visited[candidate] = true;
                    if (tail < queue.size()) queue[tail++] = candidate;
                }
            }
            return false;
        };

        auto shouldProcess = [&](Track* track) noexcept {
            if (!track) return false;
            if (offlineTargetActive) return reachesOfflineTarget(track->getId());
            return !anySolo || track->isSolo();
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

        auto routeToNextStages = [&](Track* source) {
            if (!source || !source->canProcess(sz)) return;
            auto& sourceBuffer = source->getWorkBuffer(sz);
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
                for (uint32_t sample = 0; sample < sz; ++sample) {
                    destL[sample] += std::isfinite(sourceL[sample]) ? sourceL[sample] : 0.0f;
                    destR[sample] += std::isfinite(sourceR[sample]) ? sourceR[sample] : 0.0f;
                }
                return;
            }
            const float* sourceL = sourceBuffer.getReadPointer(0, 0);
            const float* sourceR = sourceBuffer.getReadPointer(1, 0);
            bool routed = false;
            for (uint32_t destIndex = 0; destIndex < state.trackCount; ++destIndex) {
                Track* dest = state.activeTracks[destIndex];
                if (!dest || dest == source || !shouldProcess(dest) || !dest->canProcess(sz)) continue;
                const float gain = routing.connectionGain(source->getId(), dest->getId());
                if (!std::isfinite(gain) || gain <= 0.0f) continue;
                dest->addInput(sourceBuffer, sz);
                if (auto* busDest = dynamic_cast<BusTrack*>(dest)) {
                    busDest->addPreFxInput(sourceBuffer, sz, gain);
                }
                if (gain != 1.0f) {
                    auto& destBuffer = dest->getWorkBuffer(sz);
                    float* destL = destBuffer.getWritePointer(0, 0);
                    float* destR = destBuffer.getWritePointer(1, 0);
                    for (uint32_t sample = 0; sample < sz; ++sample) {
                        const float l = std::isfinite(sourceL[sample]) ? sourceL[sample] : 0.0f;
                        const float r = std::isfinite(sourceR[sample]) ? sourceR[sample] : 0.0f;
                        destL[sample] += l * (gain - 1.0f);
                        destR[sample] += r * (gain - 1.0f);
                    }
                }
                routed = true;
            }
            if (!routed) {
                float* destL = output.getWritePointer(0, off);
                float* destR = output.getWritePointer(1, off);
                for (uint32_t sample = 0; sample < sz; ++sample) {
                    destL[sample] += sourceL[sample];
                    destR[sample] += sourceR[sample];
                }
            }
        };

        auto publishBusIfNeeded = [&](Track* track) {
            if (auto* busTrack = dynamic_cast<BusTrack*>(track)) {
                busTrack->publishPostFx(sz);
            }
        };

        std::array<uint32_t, Aura::Core::Engine::RoutingEngine::kMaxNodes> topology{};
        uint32_t topologyCount = 0;
        const bool hasTopology = routing.getTopologyOrder(topology, topologyCount);
        if (hasTopology) {
            for (uint32_t n = 0; n < topologyCount; ++n) {
                Track* track = findTrack(topology[n]);
            if (!shouldProcess(track) || !track->canProcess(sz)) continue;
                if (auto* busTrack = dynamic_cast<BusTrack*>(track)) {
                    busTrack->loadPreFxIntoWork(sz);
                }
                track->processAccumulated(sz, currentPlayhead);
                publishBusIfNeeded(track);
                {
                    const auto& source = track->getWorkBuffer(sz);
                    m_sidechainManager.publishAudioSourceBlock(
                        track->getId(), source.getReadPointer(0),
                        source.getNumChannels() > 1 ? source.getReadPointer(1)
                                                    : source.getReadPointer(0),
                        sz, static_cast<uint32_t>(std::max(
                            1.0, m_sampleRate.load(std::memory_order_acquire))),
                        callbackGeneration);
                }
                for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
                    if (state.activeTracks[i] == track) processed[i] = true;
                }
                routeToNextStages(track);
            }
        }
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            Track* track = state.activeTracks[i];
            if (!track || processed[i] || !shouldProcess(track) || !track->canProcess(sz)) continue;
            if (auto* busTrack = dynamic_cast<BusTrack*>(track)) {
                busTrack->loadPreFxIntoWork(sz);
            }
            track->processAccumulated(sz, currentPlayhead);
            publishBusIfNeeded(track);
            {
                const auto& source = track->getWorkBuffer(sz);
                m_sidechainManager.publishAudioSourceBlock(
                    track->getId(), source.getReadPointer(0),
                    source.getNumChannels() > 1 ? source.getReadPointer(1)
                                                : source.getReadPointer(0),
                    sz, static_cast<uint32_t>(std::max(
                        1.0, m_sampleRate.load(std::memory_order_acquire))),
                    callbackGeneration);
            }
            routeToNextStages(track);
        }

        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            Track* track = state.activeTracks[i];
            if (!track || !track->canProcess(sz)) continue;
            const auto& work = track->getWorkBuffer(sz);
            routing.captureFeedback(track->getId(), work.getReadPointer(0),
                                    work.getReadPointer(1), sz);
        }

        // The metronome is part of the rendered monitor signal, not a track
        // node.  Inject it after graph/routing processing so it remains
        // sample-accurate even when tracks are muted or an offline target is
        // selected, and let the master limiter handle the final headroom.
        if (m_isPlaying.load(std::memory_order_relaxed) &&
            m_metronome.isEnabled()) {
            m_metronome.process(output.getWritePointer(0, off),
                                output.getWritePointer(1, off), sz,
                                currentPlayhead,
                                m_sampleRate.load(std::memory_order_relaxed),
                                m_tempo.load(std::memory_order_relaxed));
        }

        // --- INDUSTRIAL-GRADE NAN/INF SANITIZER ---
        // Must run BEFORE the limiter so it sees clean signal (not distorted clips).
        float* dest_l = output.getWritePointer(0, off);
        float* dest_r = output.getWritePointer(1, off);
        for (uint32_t s_idx = 0; s_idx < sz; ++s_idx) {
            if (!std::isfinite(dest_l[s_idx])) dest_l[s_idx] = 0.0f;
            if (!std::isfinite(dest_r[s_idx])) dest_r[s_idx] = 0.0f;
        }
        const float masterGain = m_masterGain.load(std::memory_order_acquire);
        for (uint32_t s_idx = 0; s_idx < sz; ++s_idx) {
            dest_l[s_idx] *= masterGain;
            dest_r[s_idx] *= masterGain;
        }

        // Build the callback context locally. The control thread may update
        // the persistent preparation context during device reconfiguration;
        // the audio callback must never observe a partially updated struct.
        ::Aura::DSP::ProcessContext callbackContext{};
        callbackContext.playhead = currentPlayhead;
        callbackContext.blockStart = currentPlayhead;
                callbackContext.blockEnd = currentPlayhead <=
                    std::numeric_limits<uint64_t>::max() - sz
                    ? currentPlayhead + sz
                    : std::numeric_limits<uint64_t>::max();
        callbackContext.sampleRate = std::max(1.0, m_sampleRate.load(std::memory_order_acquire));
        callbackContext.bpm = m_tempo.load(std::memory_order_acquire);
        callbackContext.blockSize = sz;
        callbackContext.audioConfigGeneration = callbackGeneration;
        callbackContext.isPlaying = m_isPlaying.load(std::memory_order_acquire);
        callbackContext.anyoneSoloed = anySolo;
        callbackContext.isLooping = m_loopEnabled.load(std::memory_order_acquire);
        callbackContext.cycleStart = cycleStart;
        callbackContext.cycleEnd = cycleEnd;
        // The master limiter is part of the rendered signal as well as the
        // live monitor path. Offline bounces now append the configured tail,
        // so its look-ahead history is flushed instead of being truncated at
        // the content boundary.
        if (m_masterLimiter) {
            // Limit only the active [offset, offset+size) window. Passing the
            // owning output buffer here would re-process stale samples when
            // processBlockDirect() renders into a non-zero offset.
            float* masterChannels[2] = {dest_l, dest_r};
            AudioBuffer masterView;
            masterView.wrapChannels(masterChannels, 2, sz);
            m_masterLimiter->process(masterView, m_blockMidi, callbackContext);
        }

        // The public meter API measures the post-limiter master before
        // monitor-only speaker trim and DIM are applied.
        if (m_analyzer) {
            m_analyzer->process(output.getReadPointer(0, off),
                                output.getReadPointer(1, off), sz);
        }

        // Control Room is strictly device-monitoring. Never bake speaker trim
        // or DIM into an offline master/stem render.
        if (!m_offlineRenderActive.load(std::memory_order_acquire)) {
            m_controlRoom.processMonitor(output.getWritePointer(0, off),
                                         output.getWritePointer(1, off), sz);
        }


        // 3. System Telemetry & Adaptive Sovereignty (CPU Governor)
        float currentLoad = m_dspLoad.load(std::memory_order_relaxed);
        // Diagnostics are useful, but formatting/publishing on every block
        // needlessly competes with the audio callback. Keep a coarse heartbeat
        // and leave the detailed telemetry to the control-side poller.
        if (sampleCpu) {
            m_engineDiagnostics.recordRealtimeBlock();
        }

        // INDUSTRIAL: Adaptive CPU Governor
        // If the system is nearing its limits (>95%), we forensic-ly throttle background tracks.
        if (currentLoad > 0.95f) {
            for (uint32_t i = 0; i < state.trackCount; ++i) {
                Track* track = state.activeTracks[i];
                if (track && (track->getType() == Track::Audio || track->getType() == Track::Instrument)) {
                    track->setEcoMode(true);
                }
            }
            m_ecoModeLogTriggered.store(true, std::memory_order_release);
        } else if (currentLoad < 0.70f) {
            for (uint32_t i = 0; i < state.trackCount; ++i) {
                Track* track = state.activeTracks[i];
                if (track) {
                    track->setEcoMode(false);
                }
            }
        }

        // --- AUTOMATION RECORDING ---
        if (m_isPlaying.load(std::memory_order_acquire) &&
            m_autoRecordMode.load(std::memory_order_relaxed) > 0) {
            for (uint32_t i = 0; i < state.trackCount; ++i) {
                if (state.activeTracks[i] && state.activeTracks[i]->isRecordArmed()) {
                    state.activeTracks[i]->recordCurrentState(currentPlayhead);
                }
            }
        }

        // (NaN/Inf sanitizer has already run before the limiter above)
        if (m_isPlaying.load(std::memory_order_relaxed)) {
            // INDUSTRIAL: Real-time Tonal Sync (Scale Quantize)
            if (m_isAscended.load(std::memory_order_relaxed)) {
                // ...
            }

            // INDUSTRIAL: Frame-Accurate Video Sync
            if (m_showVideo.load(std::memory_order_relaxed)) {
                // In a production bridge, we would call the Rust Orchestrator here.
                m_videoSyncLogTriggered.store(true, std::memory_order_release);
            }

            if (currentPlayhead <= std::numeric_limits<uint64_t>::max() - sz) {
                m_playhead.fetch_add(sz, std::memory_order_relaxed);
            } else {
                m_audioRangeOverflow.store(true, std::memory_order_release);
            }
        }
        if (sampleCpu) {
            const auto elapsed = std::chrono::duration<float>(
                std::chrono::steady_clock::now() - cpuStart).count();
            const float blockSeconds = static_cast<float>(sz) /
                static_cast<float>(std::max(1.0, m_sampleRate.load(std::memory_order_relaxed)));
            const float load = blockSeconds > 0.0f ? elapsed / blockSeconds : 0.0f;
            m_dspLoad.store(std::clamp(load, 0.0f, 4.0f), std::memory_order_relaxed);
        }
    }

void AuraUnifiedEngine::processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples) {
    m_audioCallbacksInFlight.fetch_add(1, std::memory_order_acquire);
    struct CallbackGuard {
        std::atomic<uint32_t>& count;
        ~CallbackGuard() { count.fetch_sub(1, std::memory_order_release); }
    } callbackGuard{m_audioCallbacksInFlight};

    const bool validInput = buffers != nullptr && numChannels >= 2 && numChannels <= 32 &&
        numSamples > 0 && numSamples <= AuraUnifiedEngine::kMaxAudioBlockSize &&
        buffers[0] != nullptr && buffers[1] != nullptr;
    if (!validInput) {
        // The caller does not provide the actual allocation length.  When the
        // declared block size is invalid, writing even a bounded prefix could
        // overrun a shorter external buffer.  Fail closed without touching
        // memory whose extent cannot be proven at this boundary.
        return;
    }
    for (uint32_t channel = 0; channel < numChannels; ++channel) {
        if (buffers[channel] == nullptr) return;
    }
    if (m_isShutdown.load(std::memory_order_acquire)) {
        AudioBuffer silence;
        silence.wrapChannels(buffers, numChannels, numSamples);
        silence.clear(numSamples);
        return;
    }
    // Zero-Allocation Wrapper (Allocated on stack for absolute concurrency safety).
    // Track work buffers and the sandbox protocol are prepared for bounded
    // engine blocks.  A host/device callback may legally be larger than that
    // bound, so feed the same external buffer in bounded slices rather than
    // silently skipping every track whose prepared buffer is smaller.
    AudioBuffer wrap;
    wrap.wrapChannels(buffers, numChannels, numSamples);
    constexpr uint32_t kEngineBlockSize = 4096;
    for (uint32_t offset = 0; offset < numSamples;) {
        const uint32_t remaining = numSamples - offset;
        uint32_t chunk = std::min(kEngineBlockSize, remaining);

        // A hardware callback may straddle the cycle end.  Process the tail
        // of the current cycle and the head of the next cycle as separate
        // engine chunks so notes, automation and click timing never observe
        // a playhead that runs past the loop boundary.
        if (m_isPlaying.load(std::memory_order_acquire) &&
            m_loopEnabled.load(std::memory_order_acquire)) {
            const uint64_t cycleStart = m_cycleStart.load(std::memory_order_acquire);
            const uint64_t cycleEnd = m_cycleEnd.load(std::memory_order_acquire);
            const uint64_t playhead = m_playhead.load(std::memory_order_relaxed);
            if (cycleEnd > cycleStart) {
                if (playhead >= cycleEnd) {
                    m_playhead.store(cycleStart, std::memory_order_relaxed);
                    continue;
                }
                const uint64_t untilCycleEnd = cycleEnd - playhead;
                if (untilCycleEnd < chunk) {
                    chunk = static_cast<uint32_t>(untilCycleEnd);
                }
            }
        }
        if (chunk == 0) continue;
        m_inputMonitorBlockOffset = offset;
        processBlock(wrap, offset, chunk);
        offset += chunk;
    }
    m_inputMonitorBlockPublished.store(false, std::memory_order_release);
}
