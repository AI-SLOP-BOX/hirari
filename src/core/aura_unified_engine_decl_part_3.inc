    // Tracks
    uint32_t add_track(uint32_t type);
    bool add_vca_group(uint32_t group_id, float gain);
    bool assign_track_to_vca(uint32_t track_id, uint32_t group_id);
    bool set_vca_group_gain(uint32_t group_id, float gain);
    float get_vca_track_gain(uint32_t track_id) const;
    rust::String get_vca_snapshot_json() const;
    void clear_vca_groups();
    bool remove_track(uint32_t id);
    uint32_t duplicate_track(uint32_t id);
    void new_project();
    bool set_track_name(uint32_t id, std::string_view name);
    uint32_t get_track_count() const;
    std::vector<std::shared_ptr<Track>> get_tracks_snapshot() const;
    // Freeze is a control/offline operation.  The engine owns the track
    // lifetime and serializes the render against the callback boundary
    // before publishing the immutable frozen buffer.
    bool freeze_track(uint32_t tid, uint64_t total_samples, uint32_t sample_rate);
    bool freeze_track_to_project_end(uint32_t tid, uint32_t sample_rate);
    bool freeze_track_to_file(uint32_t tid, std::string_view path,
                              uint64_t total_samples, uint32_t sample_rate);
    bool restore_track_freeze_from_file(uint32_t tid, std::string_view path,
                                        uint64_t total_samples, uint32_t sample_rate);
    bool set_track_freeze_cache_path(uint32_t tid, std::string_view path);
    bool unfreeze_track(uint32_t tid);
    bool is_track_frozen(uint32_t tid) const;
    rust::Slice<const float> get_track_peaks_l() const;
    rust::Slice<const float> get_track_peaks_r() const;
    Track& get_track(uint32_t id);

    // Regions & Plugins
    bool add_region(uint32_t trackId, std::string_view path, double start_beat);
    bool replace_region_audio(uint32_t trackId, uint32_t regionId, std::string_view path);
    bool add_plugin(uint32_t tid, uint32_t pluginType);
    std::vector<uint8_t> get_plugin_state(uint32_t tid, uint32_t pluginIndex) const;
    bool set_plugin_state(uint32_t tid, uint32_t pluginIndex,
                          const std::vector<uint8_t>& state);
    bool remove_plugin(uint32_t tid, uint32_t pluginIndex);
    bool move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex);
    bool add_sandboxed_plugin(uint32_t tid, std::string_view pluginPath);
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
    bool reset_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex);
    struct SandboxStatus {
        uint32_t trackId = 0;
        uint32_t pluginIndex = 0;
        bool alive = false;
        bool canRetry = false;
        uint8_t failure = 0;
        uint32_t droppedOutputMidi = 0;
        uint32_t mailboxOverruns = 0;
        uint32_t inputMidiTruncations = 0;
        uint32_t recoveryMode = 0;
    };
    std::vector<SandboxStatus> get_sandbox_statuses();
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
    bool set_plugin_parameter(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId, float value);
    // Internal automation/hydration path. It applies the value without
    // creating a user-facing Undo step for every generated control event.
    bool set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId, float value);
    float get_plugin_parameter(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    uint32_t get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const;
    bool has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const;
    uint64_t open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                       uintptr_t parent) const;
    bool close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                    uint64_t session) const;
    std::string get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    bool save_plugin_preset(uint32_t tid, uint32_t pluginIndex, std::string_view path) const;
    bool load_plugin_preset(uint32_t tid, uint32_t pluginIndex, std::string_view path);
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed);
    bool get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const;
    void set_macro_value(uint32_t macroIndex, float value);
    void set_preview_synth_engine(uint32_t engine);
    bool set_route(uint32_t sourceId, uint32_t destId, bool enabled);
    bool set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled);
    bool set_feedback_route(uint32_t sourceId, uint32_t destId, float gain, bool enabled);
    bool set_sidechain_link(uint32_t sourceId, uint32_t destId,
                            uint32_t pluginIndex, uint32_t tapPoint, bool enabled);
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const;

private:
    bool set_sidechain_link_internal(uint32_t sourceId, uint32_t destId,
                                     uint32_t pluginIndex, uint32_t tapPoint,
                                     bool enabled, bool recordUndo);

