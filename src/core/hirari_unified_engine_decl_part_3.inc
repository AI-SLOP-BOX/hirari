    // Tracks
    bool assign_track_to_vca(uint32_t track_id, uint32_t group_id);
    void new_project();
    std::vector<std::shared_ptr<Track>> get_tracks_snapshot() const;

    // Regions & Plugins
    bool add_region(uint32_t trackId, std::string_view path, double start_beat);
    bool region_table_mirror_matches(uint32_t trackId) const;
    // 0 = still decoding, 1 = committed, 2 = failed or stale, 3 = unknown ticket.
    bool move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex);
    bool process_sandboxed_plugin_block(uint32_t tid, uint32_t sandboxIndex,
                                        float* left, float* right, uint32_t frames);
    std::vector<uint8_t> process_sandboxed_plugin_midi_block(
        uint32_t tid, uint32_t sandboxIndex, uint32_t frames,
        const uint8_t* midiData, uint32_t midiSize);
    // Control-thread maintenance. Returns the number of sandbox processes
    // successfully recovered during this pass.
    uint32_t maintain_sandboxed_plugins(bool autoRestart);
    bool retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex);
    bool restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex);
    // Status enumeration for the UI lives in Rust; see
    // plugin_sandbox_maintenance.rs and AudioEngine::get_sandbox_statuses.
    // Control-thread diagnostic edge: number of AU processors that crossed
    // the consecutive-overrun watchdog since the previous poll.
    uint32_t take_watchdog_trips();
    uint64_t non_finite_plugin_samples() const;
    uint8_t get_last_sandbox_failure(uint32_t trackId) const;
    std::string get_last_sandbox_failure_text(uint32_t trackId) const;
    std::vector<std::string> get_sandbox_plugin_paths() const;
    std::vector<uint8_t> get_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex) const;
    bool set_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex,
                                  const std::vector<uint8_t>& state);
    uint8_t get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const;
    std::string get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const;
    // Internal automation/hydration path. It applies the value without
    // creating a user-facing Undo step for every generated control event.
    float get_plugin_parameter(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    uint32_t get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const;
    bool has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const;
    uint64_t open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                       uintptr_t parent) const;
    bool close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                    uint64_t session) const;
    std::string get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed);
    bool get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const;
    void set_preview_synth_engine(uint32_t engine);
    bool set_sidechain_link(uint32_t sourceId, uint32_t destId,
                            uint32_t pluginIndex, uint32_t tapPoint, bool enabled);
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const;
    // Installs the routing edit host callbacks Rust drives; see part_3b.
    static void installRoutingEditHost();
    static void installControlEditHost();
    static void installPluginEditHost();
    static void installTrackEditHost();
    static void installTrackFreezeHost();
    static void installCommandHost();
    static void installTrackControlHost();
    static void syncNative(void* host);
    std::shared_ptr<Track> makeTrack(uint32_t id, const std::string& name, Track::Type type);
    void publishTrackSnapshotLocked(bool stampAudioGeneration);
    std::shared_ptr<Track> detachTrackLocked(uint32_t tid, bool stampAudioGeneration);
    void insertTrackLocked(const std::shared_ptr<Track>& track, size_t index,
                           bool stampAudioGeneration);
    // Installs the sandbox maintenance callbacks Rust drives; see part_3a.
    static void installSandboxMaintenanceHost();

private:
