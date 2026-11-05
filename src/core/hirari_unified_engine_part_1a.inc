using ScopedDenormalGuard = ::Hirari::Core::Concurrency::ScopedDenormalGuard;

HirariUnifiedEngine::~HirariUnifiedEngine() {
    // Async region jobs hold the engine host pointer; join them before any
    // store they may still reach is destroyed.
    hirari_region_jobs_shutdown();
    // Rust keys cached peaks by engine address; a later engine may reuse it.
    hirari_region_waveform_clear(this);
    shutdown();
    // Sandbox observed-health state is keyed by engine address for the same
    // reason. Drop it after shutdown() has joined the recovery thread.
    hirari_plugin_sandbox_maintenance_forget(this);
    // The bag owns the teardown order: the block goes before the stores it
    // borrows and the journal stops before the state its writer reads.
    hirari_engine_session_destroy(m_session);
    m_session = nullptr;
}

void HirariUnifiedEngine::shutdown() {
    void* block = handle(kHirariHandleEngineBlock);
    const bool wasShutdown = hirari_engine_block_take_shutdown(block) != 0;
    hirari_engine_block_set_playing(block, 0);
    hirari_engine_block_join_callbacks(block);
    // Shutdown must cancel offline work before joining it.  Joining a render
    // that is still publishing blocks can otherwise keep app teardown alive
    // indefinitely and leave its temporary output/worker state behind.
    hirari_bounce_join(handle(kHirariHandleBounceSession));
    hirari_sandbox_recovery_stop();

    if (wasShutdown) return;

    std::lock_guard<std::mutex> lock(m_tracksMutex);
    // Retiring the table releases every track handle, live and deferred, so a
    // callback that has already been joined cannot be holding a dangling Track*.
    hirari_engine_session_close_graph(m_session);
    mark_project_layout_changed();
}

HirariUnifiedEngine::HirariUnifiedEngine() {
    // The bag built and bound every store, including the block, before this
    // ran; only the recovery loop needs the engine's own address to key by.
    hirari_sandbox_recovery_start(this, handle(kHirariHandleForensicJournal));
}

void HirariUnifiedEngine::processBlock(void* output, uint32_t off, uint32_t sz) {
    // Rust owns the whole block around this device buffer, including the
    // device format, the master fader, the record-arm gate and its own
    // shutdown flag: the block reads them from the state the engine prepared.
    // Only the FPU denormal mode stays here.
    ScopedDenormalGuard guard;
    uint32_t frames = 0;
    if (hirari_engine_block_begin(handle(kHirariHandleEngineBlock), output, off, sz, &frames) == 0) return;
    hirari_engine_block_route(handle(kHirariHandleEngineBlock), output, off, frames);
    hirari_engine_block_finish(handle(kHirariHandleEngineBlock), output, off, frames);
}

// The tracks lock guards the graph a control-plane walk reads. The engine hands
// the mutex's address to its session bag, so the Rust layout gather and every
// Rust track session bracket the same graph the edits here still lock, without
// the engine translating a request per walk.
extern "C" void hirari_tracks_mutex_lock(void* tracksMutex) noexcept {
    if (tracksMutex) static_cast<std::mutex*>(tracksMutex)->lock();
}

extern "C" void hirari_tracks_mutex_unlock(void* tracksMutex) noexcept {
    if (tracksMutex) static_cast<std::mutex*>(tracksMutex)->unlock();
}

