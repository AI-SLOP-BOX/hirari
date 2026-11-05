    // Tracks
    bool assign_track_to_vca(uint32_t track_id, uint32_t group_id);
    void new_project();
    // Lock-free window onto the published graph, for read-only control scans
    // such as the analysis bridge. The realtime callback traverses the same one.
    HirariTrackSnapshot published_tracks() const noexcept {
        HirariTrackSnapshot published{};
        hirari_engine_track_snapshot(m_trackTable, &published);
        return published;
    }

    // Regions & Plugins
    bool add_region(uint32_t trackId, std::string_view path, double start_beat);
    // The plug-in chain reads (parameters, bypass, sandbox restarts) and the
    // engine-wide watchdog/non-finite polls are Rust-owned now; see
    // engine_analysis_reads.rs. Status and slot enumeration lives in
    // plugin_sandbox_maintenance.rs and the sandbox host ops in part_3a.
    bool has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const;
    uint64_t open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                       uintptr_t parent) const;
    bool close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                    uint64_t session) const;
    std::string get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex, uint32_t parameterId) const;
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed);
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const;
    // Installs the routing edit host callbacks Rust drives; see part_3b.
    static void installRoutingEditHost();
    static void installControlEditHost();
    static void installPluginEditHost();
    // Installs the shared track-edit session (track_session.rs); see part_3b.
    static void installTrackSessionHost();
    static void installTrackEditHost();
    static void installTrackFreezeHost();
    static void installCommandHost();
    static void installTrackControlHost();
    static void syncNative(void* host);
    std::shared_ptr<Track> makeTrack(uint32_t id, const std::string& name, Track::Type type);
    void publishTrackSnapshotLocked(bool stampAudioGeneration);
    // Installs the sandbox maintenance callbacks Rust drives; see part_3a.
    static void installSandboxMaintenanceHost();

private:
