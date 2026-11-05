    // Atomic so a control-thread device change can never be seen half-applied
    // by Track preparation.
    std::atomic<uint32_t> m_blockSize{512};
    // Both DSP objects are Rust-owned; the engine keeps only their handles.
    void* m_meterCore = hirari_master_meter_create(44'100.0);
    void* m_masterLimiter = hirari_master_limiter_create(44'100.0);

    // The block tail (feedback, metering publication, metronome, master chain,
    // Control Room cue, playhead advance) is Rust-owned; the engine binds its
    // handles once and keeps only this handle.
    void* m_engineBlock = nullptr;
    // Per-engine Rust telemetry is allocated on the control thread and reused
    // by the audio callback; it must not lazily initialize on first process.
    void* m_mixerTelemetry = hirari_mixer_telemetry_create();

    std::atomic<bool> m_isShutdown{false};
    // Project hydration mutates tracks, routing and several process-wide
    // managers as one control-plane transaction.  Reject a concurrent load
    // instead of allowing two documents to interleave partial state.
    std::atomic<bool> m_projectHydrationActive{false};
    // A bounce is control-plane state, so Rust owns all of it (bounce_driver.rs):
    // offline-render ownership and its thread gate, the async worker, progress,
    // the lifecycle state, the cancel/pause requests and the publish boundary.
    // The engine keeps only this handle, which the realtime callback and the
    // control surface both read through narrow FFI entry points.
    void* m_bounceSession = hirari_bounce_session_create();
    // The layout revision is a Rust counter (engine_track.rs); a track marks it
    // when it changes and the control side polls it.
    void* m_layoutRevision = hirari_layout_revision_create();
    std::atomic<uint64_t> m_projectControlRevision{0};
    mutable std::mutex m_tracksMutex;

    // The document graph lives in the Rust track table (engine_track_table.rs)
    // and its tracks are Rust objects (engine_track.rs). A track pointer is the
    // Arc's own address: it keys the region store and is what every
    // hirari_track_* entry point takes. Callers must already hold m_tracksMutex,
    // except findPublishedTrack, which reads the lock-free snapshot.
    void* findTrackLocked(uint32_t tid) const {
        return hirari_engine_track_find(m_trackTable, tid);
    }
    void* findPublishedTrack(uint32_t tid) const {
        const auto published = published_tracks();
        if (published.track_map == nullptr || tid >= kMaxTrackMap) return nullptr;
        return published.track_map[tid];
    }

    // Borrowed window onto the graph in layout order, valid while the lock is held.
    struct TrackRange {
        void* const* items;
        size_t count;
        size_t size() const noexcept { return count; }
        void* const* begin() const noexcept { return items; }
        void* const* end() const noexcept { return items + count; }
    };
    TrackRange lockedTracks() const noexcept {
        return { hirari_engine_track_ptrs(m_trackTable),
                 hirari_engine_track_count(m_trackTable) };
    }
    void* lockedTrackAt(size_t index) const noexcept {
        const TrackRange tracks = lockedTracks();
        return index < tracks.count ? tracks.items[index] : nullptr;
    }

    // A copy of the track that outlives the topology lock: an operation that can
    // block on a worker or vendor UI code holds this and releases it when done.
    void* takeTrackOwner(uint32_t tid) const {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        return hirari_engine_track_retain(m_trackTable, tid);
    }

    // Same resolution for the host callbacks, which only hold the opaque engine
    // pointer; each handler keeps its mutation instead of the lock prologue.
    template <typename Fn>
    static std::invoke_result_t<Fn, void*> withHostTrack(
        void* host, uint32_t tid, std::invoke_result_t<Fn, void*> missing, const Fn& fn) {
        if (!host) return missing;
        auto* engine = static_cast<HirariUnifiedEngine*>(host);
        std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
        void* track = engine->findTrackLocked(tid);
        return track != nullptr ? fn(track) : missing;
    }

    // Same resolution for the read-only accessors, with the fallback the caller
    // supplies instead of a repeated scan.
    template <typename Fn>
    std::invoke_result_t<Fn, void*> queryLockedTrack(
        uint32_t tid, std::invoke_result_t<Fn, void*> missing, const Fn& fn) const {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        void* track = findTrackLocked(tid);
        return track != nullptr ? fn(track) : missing;
    }

    // Sidechain route bookkeeping is project state owned by Rust
    // (sidechain_route_store.rs); the engine keeps only the opaque handle.
    void* m_sidechainRoutes = hirari_sidechain_routes_create();

    // Rust owns the layout snapshot (layout_gather.rs); this only exposes the
    // live-track accessors it reads through.
    const HirariLayoutHostOps* layoutHostOps() const;

    void* m_midiSnapshotStore = nullptr;

    std::atomic<uint32_t> m_autoRecordMode{0};
    std::atomic<int32_t> m_rootNote{0}, m_scaleType{0};
