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

// The three track reads the Rust block tail cannot make through a handle
// (engine_block_host.rs). A bus is the track type, not a downcast.
void* engineTrackRuntime(void* track) {
    return static_cast<Track*>(track)->realtimeRuntime();
}
void* engineTrackBusState(void* track) {
    auto* target = static_cast<Track*>(track);
    return target->getType() == Track::Bus
        ? static_cast<BusTrack*>(target)->busStateHandle() : nullptr;
}
uint8_t engineTrackSolo(void* track) {
    return static_cast<Track*>(track)->isSolo() ? 1u : 0u;
}
void engineTrackRecord(void* track, uint64_t playhead) {
    auto* target = static_cast<Track*>(track);
    if (target->isRecordArmed()) target->recordCurrentState(playhead);
}
}  // namespace

HirariUnifiedEngine::~HirariUnifiedEngine() {
    // Async region jobs hold the engine host pointer; join them before any
    // member (decoder, tracks) is destroyed.
    hirari_region_jobs_shutdown();
    // Rust keys cached peaks by engine address; a later engine may reuse it.
    hirari_region_waveform_clear(this);
    shutdown();
    // The tail only borrows the handles below, so retire it before any of them.
    hirari_engine_block_destroy(m_engineBlock);
    m_engineBlock = nullptr;
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
    static const HirariEngineBlockOps blockOps{&engineTrackRuntime, &engineTrackBusState,
                                               &engineTrackSolo, &engineTrackRecord};
    m_engineBlock = hirari_engine_block_create(
        m_routingEngine, m_bounceSession, m_inputBlocks, m_controlRoomState,
        m_mixerTelemetry, m_metronome, m_masterLimiter, m_meterCore, m_tempoState,
        m_busSystem, m_sidechainEngine, &blockOps);
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

        // Rust owns the graph half of the block: transport staging, the one
        // block delayed feedback edges, the live driver input, the routing
        // destination table and the process-order walk that fans every eligible
        // track into its buses and the master window. The table and the Send PDC
        // scratch are bound once with the block state, so no block allocates.
        hirari_engine_block_route(
            m_engineBlock, output.storageHandle(), off, sz, currentPlayhead,
            m_sampleRate.load(std::memory_order_relaxed), m_inputMonitorBlockOffset,
            callbackGeneration, m_isPlaying.load(std::memory_order_relaxed) ? 1u : 0u,
            reinterpret_cast<void* const*>(state.activeTracks), state.trackCount);
