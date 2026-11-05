    // Tracks
    // VCA assignment is answered by Rust now (engine_transport.rs): the track
    // is resolved under the C++ tracks lock through the session ops and the
    // assignment lands in the Rust VCA manager, so no engine forwarder is left.
    void new_project();
    // Lock-free window onto the published graph, for read-only control scans
    // such as the analysis bridge. The realtime callback traverses the same one.
    HirariTrackSnapshot published_tracks() const noexcept {
        HirariTrackSnapshot published{};
        hirari_engine_track_snapshot(handle(kHirariHandleTrackTable), &published);
        return published;
    }

    // Regions & Plugins
    bool add_region(uint32_t trackId, std::string_view path, double start_beat);
    // The plug-in chain reads (parameters, bypass, sandbox restarts) and the
    // engine-wide watchdog/non-finite polls are Rust-owned now; see
    // engine_analysis_reads.rs. Status and slot enumeration lives in
    // plugin_sandbox_maintenance.rs and the sandbox host ops in part_3a.
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed);
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const;
    // Installs the routing edit host callbacks Rust drives; see part_3b.
    static void installRoutingEditHost();
    static void installPluginEditHost();
    static void installTrackEditHost();
    static void installTrackFreezeHost();
    static void installCommandHost();
    static void syncNative(void* host);
    void publishTrackSnapshotLocked(bool stampAudioGeneration);
    // Installs the sandbox maintenance callbacks Rust drives; see part_3a.
    static void installSandboxMaintenanceHost();

private:
