extern "C" void hirari_audio_publish_monitor_block(
    const float* const*, uint32_t, uint32_t, const void*, const void*, void*, void*,
    uint32_t, float*, float*);
extern "C" void hirari_audio_publish_talkback_block(
    const float* const*, uint32_t, uint32_t, const void*, void*, void*, float*);

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

namespace {
// Rust hands back an owned array for every routing snapshot. Copy the records
// once, then release the Rust allocation through the matching free entry so
// no control path has to remember the take/cast/free sequence.
std::vector<RoutingEngine::Connection> takeRoutingConnections(void* records, size_t count) {
    if (!records) return {};
    const auto* typed = static_cast<const RoutingEngine::Connection*>(records);
    std::vector<RoutingEngine::Connection> out(typed, typed + count);
    hirari_routing_connection_array_free(records, count);
    return out;
}

std::vector<RoutingEngine::FeedbackConnection> takeRoutingFeedback(void* records, size_t count) {
    if (!records) return {};
    const auto* typed = static_cast<const RoutingEngine::FeedbackConnection*>(records);
    std::vector<RoutingEngine::FeedbackConnection> out(typed, typed + count);
    hirari_routing_feedback_array_free(records, count);
    return out;
}

// Emits one articulation's timed key-switch events as scheduled note-offs.
// `transition` selects the pre-note-on transition set; the tempo mapping stays
// here because the Rust side has no access to the owned tempo state.
void scheduleTimedKeySwitchOutputs(void* tempoState, Track* target, uint8_t articulationId,
                                   bool transition, double startBeat, double sampleRate,
                                   uint64_t offset) {
    const auto emit = [tempoState, target, startBeat, sampleRate, offset](
                          uint8_t outputChannel, uint8_t outputPitch,
                          uint32_t lengthTicks) {
        const double endBeat = startBeat + static_cast<double>(lengthTicks) / 960.0;
        const uint64_t endSample = hirari_tempo_map_beats_to_samples(
            tempoState, endBeat, sampleRate);
        (void)target->scheduleExpressionNoteOff(outputChannel, outputPitch, endSample, offset);
    };
    if (transition)
        target->forEachTimedTransitionKeySwitchOutput(articulationId, emit);
    else
        target->forEachTimedKeySwitchOutput(articulationId, emit);
}
}  // namespace

HirariUnifiedEngine::~HirariUnifiedEngine() {
    // Async region jobs hold the engine host pointer; join them before any
    // member (decoder, tracks) is destroyed.
    hirari_region_jobs_shutdown();
    // Rust keys cached peaks by engine address; a later engine may reuse it.
    hirari_region_waveform_clear(this);
    shutdown();
    // Sandbox observed-health state is keyed by engine address for the same
    // reason. Drop it after shutdown() has joined the recovery thread.
    hirari_plugin_sandbox_maintenance_forget(this);
    hirari_midi_snapshot_store_destroy(m_midiSnapshotStore);
    hirari_control_room_state_destroy(m_controlRoomState);
    hirari_native_pdc_destroy(m_pdcState);
    hirari_tempo_map_destroy(m_tempoState);
    hirari_external_sync_destroy(m_externalSync);
    hirari_arrangement_document_destroy(m_arrangementDocument);
    hirari_metronome_destroy(m_metronome);
    hirari_mixer_telemetry_destroy(m_mixerTelemetry);
    hirari_macro_mapping_destroy(m_macroControl);
    hirari_midi_learn_destroy(m_midiLearnState);
    hirari_preview_synth_destroy(m_previewSynthState);
    hirari_preview_sampler_destroy(m_previewSampler);
    hirari_mpe_state_free(m_mpeState);
    hirari_routing_engine_destroy(m_routingEngine);
    hirari_sidechain_routes_destroy(m_sidechainRoutes);
    hirari_sidechain_engine_destroy(m_sidechainEngine);
    hirari_mpe_manager_destroy(m_mpeManager);
    hirari_bus_system_destroy(m_busSystem);
    hirari_forensic_journaler_stop(m_forensicJournal);
    hirari_forensic_journaler_destroy(m_forensicJournal);
    hirari_master_limiter_destroy(m_masterLimiter);
    hirari_master_meter_destroy(m_meterCore);
    hirari_undo_manager_destroy(m_undoState);
}

void HirariUnifiedEngine::publish_input_monitor_block(const float* const* inputs,
                                                    uint32_t numChannels,
                                                    uint32_t numSamples) noexcept {
    hirari_audio_publish_monitor_block(
        inputs, numChannels, numSamples, &m_inputMonitorTrackId,
        &m_inputMonitorChannelPair, &m_inputMonitorBlockPublished,
        &m_inputMonitorBlockFrames, kNoInputMonitorTrack,
        m_inputMonitorLeft.data(), m_inputMonitorRight.data());
}

void HirariUnifiedEngine::publish_talkback_input_block(const float* const* inputs,
                                                       uint32_t numChannels,
                                                       uint32_t numSamples) noexcept {
    hirari_audio_publish_talkback_block(
        inputs, numChannels, numSamples, m_controlRoomState,
        &m_talkbackInputPublished, &m_talkbackInputFrames,
        m_talkbackInput.data());
}

void HirariUnifiedEngine::apply_config(const EngineConfig& cfg) {
    hirari_tempo_map_set_current_bpm(m_tempoState, cfg.tempo);
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
    hirari_sandbox_recovery_stop();

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
    m_midiSnapshotStore = hirari_midi_snapshot_store_create();
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
    (void)hirari_native_pdc_bind_control_thread(m_pdcState);
    m_masterLimiter = hirari_master_limiter_create(44'100.0);
    hirari_sandbox_recovery_start(this, m_forensicJournal);
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
        ScopedDenormalGuard guard;
        if (m_isShutdown.load(std::memory_order_acquire)) {
            output.clear(off, sz);
            return;
        }
        // Hot-swap quiesce: bounded backoff (max 200 x 50µs = 10ms)
        // Tight spin avoided: background daemon clears flag in <5ms by design.
        if (hirari_hot_swap_default_quiesce_requested()) {
            hirari_hot_swap_default_acknowledge();
            // Never sleep or spin on the audio thread. The next block will see
            // the request again if the structural transaction is still active.
            output.clear(off, sz);
            return;
        }

        // INDUSTRIAL: Sovereign Memory Reset
        hirari_forensic_scratchpad_reset();

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
            hirari_preview_synth_reset(m_previewSynthState);
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
            float value = 0.0f;
            switch (hirari_engine_audio_command(
                        static_cast<uint32_t>(command.type), command.val, &value)) {
                case 1: target->setVolume(value); break;   // SetVolume
                case 2: target->setPan(value); break;      // SetPan
                case 3: target->setMuted(value >= 0.5f); break;  // SetMute
                case 4: target->setSolo(value >= 0.5f); break;   // SetSolo
                default: break;
            }
        }
        // Keep the device callback primed while stopped, but do not traverse
        // the full graph when no preview or offline render owns the path.
        // Command mutations are deliberately drained first so fader, pan,
        // mute, and solo edits remain visible immediately while stopped.
        const bool previewActive = hirari_preview_sampler_is_active(m_previewSampler);
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
            HirariMidiSnapshotView midiSnapshot{};
            const bool midiSnapshotPinned = hirari_midi_snapshot_store_pin(
                m_midiSnapshotStore, &midiSnapshot);
            const uint64_t midiSnapshotRevision = midiSnapshot.revision;
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
                hirari_preview_synth_reset(m_previewSynthState);
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
                using AudioState = std::remove_reference_t<decltype(state)>;
                struct MidiDispatchContext {
                    HirariUnifiedEngine* engine;
                    AudioState* state;
                    uint64_t playhead;
                } dispatch{this, &state, currentPlayhead};
                hirari_midi_schedule_block(
                    midiSnapshot.notes, midiSnapshot.note_count, midiSnapshot.indices_by_end,
                    midiSnapshot.end_count, currentPlayhead, sz,
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
                            const double startBeat = hirari_tempo_map_samples_to_beats(
                                engine.m_tempoState, note.startSample, sampleRate);
                            scheduleTimedKeySwitchOutputs(engine.m_tempoState, target,
                                                          note.articulationId, true, startBeat,
                                                          sampleRate, offset);
                            target->addMidiInputNoteOn(channel, note.pitch, note.velocity,
                                                       offset, note.articulationId);
                            scheduleTimedKeySwitchOutputs(engine.m_tempoState, target,
                                                          note.articulationId, false, startBeat,
                                                          sampleRate, offset);
                        } else if (engine.m_blockMidi.remainingCapacity() > 0) {
                            engine.m_blockMidi.addNoteOn(channel, note.pitch, note.velocity, offset);
                            ++engine.m_previewFallbackActiveNotes[note.midiChannel][note.pitch];
                        }
                    });
                hirari_midi_snapshot_store_release(m_midiSnapshotStore, midiSnapshot.slot);
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
        hirari_midi_process_mpe(m_mpeState, m_blockMidi.getEvents(), m_blockMidi.size());
        hirari_midi_apply_articulations_from_state(
            m_mpeState, m_blockMidi.getMutableEvents(), m_blockMidi.size());

        // Deterministic built-in preview instrument. This is intentionally a
        // small built-in voice so MIDI playback can be verified without plugins.
        const double sr = std::max(1.0, m_sampleRate.load(std::memory_order_relaxed));
        hirari_preview_synth_process(m_previewSynthState, m_blockMidi.getEvents(),
            m_blockMidi.size(), output.getWritePointer(0, off),
            output.getWritePointer(1, off), sz, currentPlayhead, sr, m_macroControl);
        // One-shot preview sampler. Rust owns the published buffer, the
        // trigger/playing flags and the cursor; the callback only renders.
        hirari_preview_sampler_render(m_previewSampler, sr, output.getWritePointer(0, off),
                                      output.getWritePointer(1, off), sz);

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
        void* routing = m_routingEngine;
        hirari_bus_system_clear(m_busSystem, sz);
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            if (state.activeTracks[i] && state.activeTracks[i]->canProcess(sz)) {
                state.activeTracks[i]->setTransportState(
                    hirari_tempo_map_get_current_bpm(m_tempoState),
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
            return hirari_routing_engine_should_process(
                routing, track->getId(), offlineTargetActive, offlineTargetId, anySolo,
                track->isSolo());
        };

        // Feedback edges are explicitly delayed by one engine block. They are
        // injected before any graph node runs, so a cycle cannot recurse on
        // the current block or allocate on the audio thread.
        for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
            Track* track = state.activeTracks[i];
            if (!track || !track->canProcess(sz)) continue;
            auto& work = track->getWorkBuffer(sz);
            hirari_routing_engine_inject_feedback(
                routing, track->getId(), work.getWritePointer(0),
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
        std::array<float, BusTrack::kMaxSamples> sendPdcLeft;
        std::array<float, BusTrack::kMaxSamples> sendPdcRight;
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
            if (!offlineTargetActive && hirari_control_room_is_cue_bus(m_controlRoomState, source->getId())) return;
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
            hirari_routing_engine_process_track_fanout(
                routing, source->getId(),
                sourceBuffer.getArrayOfReadPointers(), sourceBuffer.getNumChannels(),
                sendPreLeft, sendPreRight, sendPostLeft, sendPostRight,
                source->getTrackDelaySamples(), routingDestinations.data(),
                routingDestinationCount, output.getWritePointer(0, off),
                output.getWritePointer(1, off), sendPdcLeft.data(), sendPdcRight.data(),
                sz, BusTrack::kMaxSamples, addBusPreFx);
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
        if (!hirari_routing_engine_resolve_process_order(
                routing, activeTrackIds.data(), activeTrackCount,
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
                hirari_sidechain_engine_publish_source(
                    m_sidechainEngine,
                    track->getId(), preFx.getReadPointer(0), preFx.getReadPointer(1),
                    postFx.getReadPointer(0), postFx.getReadPointer(1),
                    source.getReadPointer(0), source.getReadPointer(1),
                    sz, static_cast<uint32_t>(std::max(
                        1.0, m_sampleRate.load(std::memory_order_acquire))),
                    callbackGeneration);
            }
            routeToNextStages(track);
        }
