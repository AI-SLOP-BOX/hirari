    // Analysis
    // The engine-block state and the master meter core are Rust-owned stores
    // (engine_block_host.rs, master_meter.rs); the Rust analysis reads
    // (engine_analysis_reads.rs) sample telemetry, meter and the published
    // graph straight from these handles, so no MeterData copy crosses here.
    uintptr_t engine_block_state() const noexcept {
        return reinterpret_cast<uintptr_t>(handle(kHirariHandleEngineBlock));
    }
    uintptr_t meter_state() const noexcept {
        return reinterpret_cast<uintptr_t>(handle(kHirariHandleMeter));
    }
    // The bounce session and the live track table are Rust-owned stores too;
    // the Rust offline-target and volume reads use the layout host ops
    // (layout_gather.rs) plus this session handle, so no C++ forwarder is left.
    uintptr_t bounce_session_state() const noexcept {
        return reinterpret_cast<uintptr_t>(handle(kHirariHandleBounceSession));
    }
    bool execute_auto_arrangement();
    // The tracks lock guards the graph while a control-plane walk reads it. The
    // Rust layout gather brackets its own walk with these two calls, so it
    // reaches every store through the session bag instead of a host table.
    void lockTracks() const { m_tracksMutex.lock(); }
    void unlockTracks() const { m_tracksMutex.unlock(); }
    // Stable generation token for UI/CLI snapshots. Commands must echo this
    // value back through push_command; a changed layout makes the token stale.
    uint64_t get_project_generation() const;
    void mark_project_state_changed() { hirari_engine_block_mark_control(handle(kHirariHandleEngineBlock)); }
    void mark_project_layout_changed() const {
        hirari_layout_revision_mark(hirari_engine_block_layout_revision(handle(kHirariHandleEngineBlock)));
    }
    bool set_articulation_map(uint32_t trackId, uint32_t mapHash);
    static void installExpressionMapEditHost();
    bool set_project_scale(int32_t root, int32_t type);
    
    // Phase 11: Neural Telemetry
    
    // Phase 14: Compositional Telemetry
    
    // Phase 16: Plugin Sovereignty
    
    // Diagnostics & Health

    void shutdown();

    bool pop_event(::Hirari::Core::EngineEvent& e) const;
