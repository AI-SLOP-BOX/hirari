using ScopedDenormalGuard = ::Hirari::Core::Concurrency::ScopedDenormalGuard;

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

void HirariUnifiedEngine::shutdown() {
    const bool wasShutdown = m_isShutdown.exchange(true, std::memory_order_acq_rel);
    hirari_engine_block_set_playing(m_engineBlock, 0);
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

HirariUnifiedEngine::HirariUnifiedEngine() {
    m_midiSnapshotStore = hirari_midi_snapshot_store_create();
    m_midiPlaybackClock = hirari_midi_playback_clock_create();
    m_engineRings = hirari_engine_rings_create();
    // Both published buffers start at generation one, matching the
    // preparation defaults.
    // All PDC graph edits originate from the engine/control owner. Binding
    // this instance prevents an audio callback from accidentally taking the
    // configuration mutex through a mutator.
    (void)hirari_native_pdc_bind_control_thread(m_pdcState);
    m_masterLimiter = hirari_master_limiter_create(44'100.0);
    const HirariEngineBlockHandles blockHandles{
        m_routingEngine, m_trackTable, m_bounceSession, m_inputBlocks,
        m_controlRoomState, m_mixerTelemetry, m_metronome, m_masterLimiter,
        m_meterCore, m_tempoState, m_busSystem, m_sidechainEngine,
        m_engineRings, m_midiPlaybackClock, m_midiSnapshotStore,
        m_previewSynthState, m_previewSampler, m_mpeState, m_macroControl};
    m_engineBlock = hirari_engine_block_create(blockHandles);
    hirari_sandbox_recovery_start(this, m_forensicJournal);
}

    void HirariUnifiedEngine::processBlock(void* output, uint32_t off, uint32_t sz) {
        // Rust owns the whole block around this device buffer: the gates that
        // decide whether it renders at all, the cycle wrap, the command drain,
        // the timeline MIDI staging into each track and the built-in preview
        // voices; then the graph walk and the post-graph tail that advances the
        // playhead it holds. Only the FPU denormal mode and the two scalars that
        // stay control-side are the engine's own.
        ScopedDenormalGuard guard;
        uint32_t frames = 0;
        if (hirari_engine_block_begin(
                m_engineBlock, output, off, sz,
                m_sampleRate.load(std::memory_order_relaxed),
                m_isShutdown.load(std::memory_order_acquire) ? 1u : 0u,
                &frames) == 0)
            return;
        hirari_engine_block_route(m_engineBlock, output, off, frames,
                                  m_sampleRate.load(std::memory_order_relaxed));
        hirari_engine_block_finish(
            m_engineBlock, output, off, frames,
            m_sampleRate.load(std::memory_order_relaxed),
            m_masterGain.load(std::memory_order_acquire),
            m_autoRecordMode.load(std::memory_order_relaxed) > 0 ? 1u : 0u);
    }

