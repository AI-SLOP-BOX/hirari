bool HirariUnifiedEngine::set_tempo(float bpm) const {
    // Thin entry point onto the Rust implementation, kept only because
    // tests/timeline_bpm_ghost_contract.cpp drives this method straight off
    // the engine singleton. Every other tempo edit reaches Rust without
    // entering C++ at all.
    return hirari_tempo_edit_set_tempo(
        m_tempoState, m_undoState,
        hirari_log_state(),
        hirari_engine_block_playhead(m_engineBlock),
        m_sampleRate.load(std::memory_order_relaxed), bpm);
}

bool HirariUnifiedEngine::set_master_gain(float value) {
    // Storage only: the bridge owns the range check and the undo entry. The
    // audio callback reads this atomic once per block.
    m_masterGain.store(value, std::memory_order_release);
    return true;
}

float HirariUnifiedEngine::get_master_gain() const noexcept {
    return m_masterGain.load(std::memory_order_acquire);
}

namespace {

// The curves live in each track's own Rust automation store, so an edit needs
// exactly one thing from the engine: the undo history to record it in.
const HirariAutomationHostOps kAutomationEditHostOps = {
    [](void* host) -> void* {
        return host ? static_cast<HirariUnifiedEngine*>(host)->undo_state() : nullptr;
    },
};

}  // namespace

bool HirariUnifiedEngine::execute_auto_arrangement() {
    // Project extent, section boundaries and marker lengths are Rust-owned.
    // The engine keeps the region storage and the event queue behind the
    // host callbacks and the marker sink below.
    float harmonyTension = 0.5f;
    float harmonyValence = 0.5f;
    hirari_harmonic_context_get(&harmonyTension, &harmonyValence, nullptr);
    return hirari_engine_auto_arrange(
        this, hirari_tempo_map_get_current_bpm(m_tempoState), m_sampleRate.load(),
        harmonyTension, harmonyValence,
        [](void* context, uint64_t start, uint64_t length) {
            auto* engine = static_cast<HirariUnifiedEngine*>(context);
            // EVENT_TYPE_STRUCTURE_MARKER carries the section length as its value.
            (void)hirari_engine_event_ring_push(engine->m_engineRings, start, 0x50, 0,
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
    HirariUnifiedEngine::installTrackSessionHost();
    HirariUnifiedEngine::installTrackEditHost();
    HirariUnifiedEngine::installSandboxMaintenanceHost();
    HirariUnifiedEngine::installRoutingEditHost();
    HirariUnifiedEngine::installControlEditHost();
    HirariUnifiedEngine::installPluginEditHost();
    HirariUnifiedEngine::installExpressionMapEditHost();
    HirariUnifiedEngine::installTrackFreezeHost();
    HirariUnifiedEngine::installCommandHost();
    // The tracks and their storages are Rust objects, so the spatial-edit host
    // needs no native table. The region storage host must install before the
    // region edit host, which routes its data callbacks only once it answers.
    hirari_track_region_host_install();
    hirari_region_host_install(&kRegionEditHostOps);
    hirari_automation_host_install(&kAutomationEditHostOps);
    return true;
}();
}  // namespace
