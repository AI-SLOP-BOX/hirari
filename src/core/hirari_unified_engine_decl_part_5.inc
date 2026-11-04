    // Analysis
    struct MeterData {
        float peakL = 0.0f;
        float peakR = 0.0f;
        float truePeakL = 0.0f;
        float truePeakR = 0.0f;
        float rmsL = 0.0f;
        float rmsR = 0.0f;
        float lufsShortTerm = 0.0f;
        float lufsIntegrated = 0.0f;
        float correlation = 0.0f;
        float balance = 0.0f;
    };
    MeterData getMasterMeterData() const;
    bool execute_auto_arrangement();
    rust::String get_project_layout_json() const;
    rust::String get_project_layout_json_cancellable(
        const std::atomic<bool>* cancellation) const;
    // Stable generation token for UI/CLI snapshots. Commands must echo this
    // value back through push_command; a changed layout makes the token stale.
    uint64_t get_project_generation() const;
    // Cheap monotonic edit token for UI polling. Strict command validation
    // continues to use get_project_generation().
    uint64_t get_project_layout_revision() const noexcept;
    uint64_t get_project_state_revision() const noexcept;
    void mark_project_state_changed() {
        m_projectControlRevision.fetch_add(1, std::memory_order_acq_rel);
    }
    void mark_project_layout_changed() const {
        if (m_projectLayoutRevision) m_projectLayoutRevision->markChanged();
    }
    bool set_articulation_map(uint32_t trackId, uint32_t mapHash);
    static void installExpressionMapEditHost();
    bool set_track_eq(uint32_t trackId, float lowBoostDb, float lowCutDb,
                      float highBoostDb, float highCutDb);
    bool set_project_scale(int32_t root, int32_t type);
    
    // Phase 11: Neural Telemetry
    
    // Phase 14: Compositional Telemetry
    
    // Phase 16: Plugin Sovereignty
    
    // Diagnostics & Health

    void shutdown();

    // Telemetry access (RT-Safe). The double buffer and its publication live in
    // Rust (engine_block_host.rs); these are the control-side reads.
    uint32_t get_active_telemetry_idx() const {
        return hirari_engine_telemetry_active_index(m_engineBlock);
    }
    bool copy_telemetry(uint32_t idx, HirariEngineTelemetry& destination) const {
        return hirari_engine_telemetry_snapshot(m_engineBlock, idx, &destination);
    }

    bool pop_event(::Hirari::Core::EngineEvent& e) const;
