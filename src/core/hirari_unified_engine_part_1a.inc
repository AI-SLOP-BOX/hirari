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
    // shutdown() has retired the bounce worker, so its session has no borrower.
    hirari_bounce_session_destroy(m_bounceSession);
    hirari_engine_rings_destroy(m_engineRings);
    hirari_midi_snapshot_store_destroy(m_midiSnapshotStore);
    hirari_midi_playback_clock_destroy(m_midiPlaybackClock);
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
    hirari_input_blocks_publish_monitor(m_inputBlocks, inputs, numChannels, numSamples);
}

void HirariUnifiedEngine::publish_talkback_input_block(const float* const* inputs,
                                                       uint32_t numChannels,
                                                       uint32_t numSamples) noexcept {
    hirari_input_blocks_publish_talkback(m_inputBlocks, inputs, numChannels, numSamples,
                                         m_controlRoomState);
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
    hirari_bounce_join(m_bounceSession);
    hirari_sandbox_recovery_stop();

    if (wasShutdown) return;

    std::lock_guard<std::mutex> lock(m_tracksMutex);
    // Destroying the table releases every track handle, live and deferred, so a
    // callback that has already been joined cannot be holding a dangling Track*.
    hirari_engine_track_table_destroy(m_trackTable);
    m_trackTable = nullptr;
    hirari_input_blocks_destroy(m_inputBlocks);
    m_inputBlocks = nullptr;
    mark_project_layout_changed();
}

HirariUnifiedEngine::HirariUnifiedEngine()
    : m_playhead(0)
    , m_loopEnabled(false) {
    m_midiSnapshotStore = hirari_midi_snapshot_store_create();
    m_midiPlaybackClock = hirari_midi_playback_clock_create();
    m_engineRings = hirari_engine_rings_create();
    m_masterBuffer.setSize(2, 4096);
    // Both published buffers start at generation one, matching the
    // preparation defaults.
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
        !hirari_bounce_offline_thread()) {
        output.clear(off, sz);
        return;
    }
        const uint64_t threadToken = hirari_callback_thread_token();
        RoutingEngine::SendPdcReadGuard sendPdcReadGuard(m_routingEngine);
        if (hirari_bounce_offline_blocks(m_bounceSession, threadToken)) {
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
        const auto state = trackView(m_trackTable);
        if (state.audioConfigGeneration != callbackGeneration && !hirari_bounce_offline_thread()) {
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
        HirariEngineCommandRecord command{};
        while (hirari_engine_command_ring_pop(m_engineRings, &command)) {
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
        const bool midiPanicPending =
            hirari_midi_playback_panic_pending(m_midiPlaybackClock);
        if (!hirari_bounce_offline_engaged(m_bounceSession) &&
            !m_isPlaying.load(std::memory_order_acquire) &&
            !m_testTone.load(std::memory_order_relaxed) &&
            !previewActive && !midiPanicPending) {
            output.clear(off, sz);
            return;
        }
        output.clear(off, sz);
        m_blockMidi.clear();
        {
            const bool midiPlaybackActive = hirari_bounce_offline_engaged(m_bounceSession) ||
                m_isPlaying.load(std::memory_order_acquire);
            if (currentPlayhead > std::numeric_limits<uint64_t>::max() - sz) {
                output.clear(off, sz);
                m_audioRangeOverflow.store(true, std::memory_order_release);
                return;
            }
            using AudioState = std::remove_reference_t<decltype(state)>;
            struct MidiBlockContext {
                HirariUnifiedEngine* engine;
                AudioState* state;
            } midiContext{this, &state};
            (void)hirari_midi_playback_prepare_block(
                m_midiPlaybackClock, m_midiSnapshotStore, currentPlayhead, sz,
                midiPlaybackActive ? 1 : 0, &midiContext,
                +[](void* opaque) {
                    auto& context = *static_cast<MidiBlockContext*>(opaque);
                    hirari_preview_synth_reset(context.engine->m_previewSynthState);
                    for (auto& channel : context.engine->m_previewFallbackActiveNotes)
                        std::fill(std::begin(channel), std::end(channel), 0u);
                    for (uint32_t i = 0; i < context.state->trackCount && i < kMaxTracks; ++i) {
                        Track* track = context.state->activeTracks[i];
                        if (track && (track->getType() == Track::Midi ||
                                      track->getType() == Track::Instrument))
                            track->addMidiInputAllNotesOff(0);
                    }
                },
                +[](void* opaque, const HirariScheduledMidiNote* rawNote,
                    uint64_t offset, uint8_t noteOff) {
                    auto& context = *static_cast<MidiBlockContext*>(opaque);
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
            m_blockMidi.sort();
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
        const bool offlineTargetActive = hirari_bounce_target_active(m_bounceSession) != 0;
        const uint32_t offlineTargetId = hirari_bounce_target_id(m_bounceSession);
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
        // graph processing. The staged planes were copied by the driver
        // callback, so this path performs no allocation or locking.
        const auto monitor = hirari_input_blocks_take_monitor(m_inputBlocks,
                                                             m_inputMonitorBlockOffset, sz);
        if (monitor.left) {
            float* inputChannels[2] = { monitor.left, monitor.right };
            AudioBuffer input;
            input.wrapChannels(inputChannels, 2, sz);
            if (Track* monitorTrack = findTrack(monitor.track_id);
                monitorTrack && monitorTrack->canProcess(sz)) {
                monitorTrack->addInput(input, sz);
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
                const auto& targetBuffer = hirari_bounce_pre_fader(m_bounceSession)
                    ? source->offlinePreFaderBuffer()
                    : (!hirari_bounce_include_inserts(m_bounceSession)
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
