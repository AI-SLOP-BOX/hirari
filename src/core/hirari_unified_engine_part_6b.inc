bool HirariUnifiedEngine::set_tempo(float bpm) const {
    // Thin entry point onto the Rust implementation, kept only because
    // tests/timeline_bpm_ghost_contract.cpp drives this method straight off
    // the engine singleton. Every other tempo edit reaches Rust without
    // entering C++ at all.
    return hirari_tempo_edit_set_tempo(
        handle(kHirariHandleTempo), handle(kHirariHandleUndo),
        hirari_log_state(),
        hirari_engine_block_playhead(handle(kHirariHandleEngineBlock)),
        hirari_engine_block_device_sample_rate(handle(kHirariHandleEngineBlock)), bpm);
}

bool HirariUnifiedEngine::set_master_gain(float value) {
    // Storage only: the bridge owns the range check and the undo entry. The
    // audio callback reads the block store's copy once per block.
    hirari_engine_block_set_master_gain(handle(kHirariHandleEngineBlock), value);
    return true;
}

float HirariUnifiedEngine::get_master_gain() const noexcept {
    return hirari_engine_block_master_gain(handle(kHirariHandleEngineBlock));
}

bool HirariUnifiedEngine::execute_auto_arrangement() {
    // Project extent, section boundaries and marker lengths are Rust-owned.
    // The engine keeps the region storage and the event queue behind the
    // host callbacks and the marker sink below.
    float harmonyTension = 0.5f;
    float harmonyValence = 0.5f;
    hirari_harmonic_context_get(&harmonyTension, &harmonyValence, nullptr);
    return hirari_engine_auto_arrange(
        this, hirari_tempo_map_get_current_bpm(handle(kHirariHandleTempo)),
        hirari_engine_block_device_sample_rate(handle(kHirariHandleEngineBlock)),
        harmonyTension, harmonyValence,
        [](void* context, uint64_t start, uint64_t length) {
            auto* engine = static_cast<HirariUnifiedEngine*>(context);
            // EVENT_TYPE_STRUCTURE_MARKER carries the section length as its value.
            (void)hirari_engine_event_ring_push(engine->handle(kHirariHandleEngineRings), start, 0x50, 0,
                                                static_cast<float>(length), nullptr, 0);
            hirari_log_post(0, 0, "AI | ARRANGEMENT | SECTION_DETECTED");
        });
}

namespace {
// Every host table is installed once, at image load and before any Rust caller
// can reach the engine. One owner replaces the per-table lazy initializers, so
// the install order is deterministic instead of depending on a linker's
// cross-file static order.
[[maybe_unused]] const bool kEngineHostTablesInstalled = [] {
    HirariUnifiedEngine::installTrackEditHost();
    HirariUnifiedEngine::installSandboxMaintenanceHost();
    HirariUnifiedEngine::installRoutingEditHost();
    HirariUnifiedEngine::installExpressionMapEditHost();
    HirariUnifiedEngine::installTrackFreezeHost();
    HirariUnifiedEngine::installCommandHost();
    // The tracks and their storages are Rust objects, so the spatial-edit host
    // needs no native table, and neither do the automation and control edits:
    // they read the undo history and the stores straight out of the session bag
    // the engine's address resolves to. The region storage host must install
    // before the region edit host, which routes its data callbacks only once it
    // answers.
    hirari_track_region_host_install();
    hirari_region_host_install(&kRegionEditHostOps);
    return true;
}();
}  // namespace
