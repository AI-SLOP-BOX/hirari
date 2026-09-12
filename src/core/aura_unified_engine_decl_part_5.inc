    // Automation
    bool set_automation_data(uint32_t tid, uint32_t param_id, const std::vector<AutomationPoint>& points);
    bool set_track_delay_automation(uint32_t tid, const std::vector<AutomationPoint>& points);
    bool push_command(::Aura::Core::CommandType type, uint32_t targetId, float value,
                      uint64_t timestamp, uint64_t expectedProjectGeneration,
                      uint64_t expectedAudioGeneration);
    uint8_t get_last_command_error() const noexcept {
        return m_lastCommandError.load(std::memory_order_acquire);
    }

    // Spatial
    void set_spatial_mode(uint32_t tid, uint32_t mode);
    bool set_spatial_position(uint32_t tid, float x, float y, float z);
    bool set_hrtf_kernel(uint32_t tid, const std::vector<float>& left,
                         const std::vector<float>& right);
    bool clear_hrtf_kernel(uint32_t tid);

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
    bool execute_auto_mixing();
    bool execute_auto_arrangement();
    void undo();
    void redo();
    void begin_undo_transaction(std::string_view name);
    bool end_undo_transaction();
    bool abort_undo_transaction();
    void clear_undo_history();
    uint32_t get_undo_count() const;
    uint32_t get_redo_count() const;
    rust::String get_project_layout_json() const;
    rust::String get_routing_snapshot_json() const;
    // Stable generation token for UI/CLI snapshots. Commands must echo this
    // value back through push_command; a changed layout makes the token stale.
    uint64_t get_project_generation() const;
    bool set_articulation_map(uint32_t trackId, uint32_t mapHash);
    bool set_track_eq(uint32_t trackId, float lowBoostDb, float lowCutDb,
                      float highBoostDb, float highCutDb);
    bool set_project_scale(int32_t root, int32_t type);
    std::map<std::string, float> get_vibe_scores() const;
    void perform_autonomous_correction();
    
    // Phase 11: Neural Telemetry
    rust::Vec<Bridge::PlainClash> run_masking_analysis(uint32_t trackA);
    
    // Phase 14: Compositional Telemetry
    rust::Vec<Aura::Core::Composition::NeuralArrangementKernel::Section> run_structural_analysis();
    
    // Phase 16: Plugin Sovereignty
    void autonomous_plugin_audit();
    
    // Diagnostics & Health
    bool checkSystemHealth() const;

    void shutdown();

    // Telemetry access (RT-Safe)
    struct TelemetryData {
        float peaksL[256]; 
        float peaksR[256];
        uint32_t count = 0;
        float spectrum[512]; 
        float correlation;
        float dspLoad = 0.0f;
        uint32_t activeVoices = 0;
        Mixing::AestheticFeatures aesthetic; 
        uint64_t forensicAuditHash; // --- PHASE 50: CRYPTOGRAPHIC SOVEREIGNTY ---
        uint64_t version = 0; 
    };

    uint32_t get_active_telemetry_idx() const { return m_activeTelemetryIdx.load(std::memory_order_acquire); }
    const TelemetryData& get_telemetry(uint32_t idx) const { return m_telemetryBuffers[idx]; }
    bool copy_telemetry(uint32_t idx, TelemetryData& destination) const {
        if (idx >= 2 || m_telemetryCopyLock.test_and_set(std::memory_order_acquire)) return false;
        destination = m_telemetryBuffers[idx];
        m_telemetryCopyLock.clear(std::memory_order_release);
        return true;
    }
    uint64_t get_telemetry_sequence() const { return m_telemetrySequence.load(std::memory_order_acquire); }
    
    void push_event(const ::Aura::Core::EngineEvent& e);
    bool pop_event(::Aura::Core::EngineEvent& e) const;

