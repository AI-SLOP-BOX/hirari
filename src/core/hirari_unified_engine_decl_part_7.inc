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
    std::shared_ptr<ProjectLayoutRevision> m_projectLayoutRevision =
        std::make_shared<ProjectLayoutRevision>();
    std::atomic<uint64_t> m_projectControlRevision{0};
    mutable std::mutex m_tracksMutex;

    // The document graph lives in the Rust track table (engine_track_table.rs).
    // Id resolution and every walk below go through it; callers must already hold
    // m_tracksMutex, except findPublishedTrack, which reads the lock-free snapshot.
    Track* findTrackLocked(uint32_t tid) const {
        return reinterpret_cast<Track*>(hirari_engine_track_find(m_trackTable, tid));
    }
    std::shared_ptr<Track> findTrackSharedLocked(uint32_t tid) const {
        return adoptTrackHandle(hirari_engine_track_retain(m_trackTable, tid));
    }
    Track* findPublishedTrack(uint32_t tid) const {
        return tid < kMaxTrackMap ? trackView(m_trackTable).trackMap[tid] : nullptr;
    }

    // Borrowed window onto the graph in layout order, valid while the lock is held.
    struct TrackRange {
        Track* const* items;
        size_t count;
        size_t size() const noexcept { return count; }
        Track* const* begin() const noexcept { return items; }
        Track* const* end() const noexcept { return items + count; }
    };
    TrackRange lockedTracks() const noexcept {
        return { reinterpret_cast<Track* const*>(hirari_engine_track_ptrs(m_trackTable)),
                 hirari_engine_track_count(m_trackTable) };
    }
    Track* lockedTrackAt(size_t index) const noexcept {
        const TrackRange tracks = lockedTracks();
        return index < tracks.count ? tracks.items[index] : nullptr;
    }

    // The table stores heap `std::shared_ptr<Track>` handles as opaque owners, so
    // adopting one is a cast into a unique_ptr and publishing one is a heap copy.
    static std::shared_ptr<Track> adoptTrackHandle(void* owner) {
        if (!owner) return nullptr;
        auto handle = std::unique_ptr<std::shared_ptr<Track>>(
            static_cast<std::shared_ptr<Track>*>(owner));
        return std::move(*handle);
    }
    static void* newTrackHandle(std::shared_ptr<Track> track) {
        return track ? new std::shared_ptr<Track>(std::move(track)) : nullptr;
    }

    // Applies fn to the locked track and reports whether the id resolved, so a
    // control mutator is one call plus the Track method it forwards to.
    template <typename Fn>
    bool mutateLockedTrack(uint32_t tid, const Fn& fn) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        Track* track = findTrackLocked(tid);
        return track != nullptr && fn(*track);
    }

    // Same resolution for the host callbacks, which only hold the opaque engine
    // pointer; each handler keeps its mutation instead of the lock prologue.
    template <typename Fn>
    static std::invoke_result_t<Fn, Track&> withHostTrack(
        void* host, uint32_t tid, std::invoke_result_t<Fn, Track&> missing, const Fn& fn) {
        if (!host) return missing;
        auto* engine = static_cast<HirariUnifiedEngine*>(host);
        std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
        Track* track = engine->findTrackLocked(tid);
        return track != nullptr ? fn(*track) : missing;
    }

    // Same resolution for the read-only host callbacks, with the fallback the
    // caller supplies instead of a repeated scan.
    template <typename Fn>
    std::invoke_result_t<Fn, const Track&> queryLockedTrack(
        uint32_t tid, std::invoke_result_t<Fn, const Track&> missing, const Fn& fn) const {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const Track* track = findTrackLocked(tid);
        return track != nullptr ? fn(*track) : missing;
    }

    // Copies the owning handle out from under the lock so a call that can enter
    // vendor UI code holds the Track without the project topology lock.
    std::shared_ptr<Track> takeTrackHandle(uint32_t tid) const {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        return findTrackSharedLocked(tid);
    }

    // Variable-length host reads answer "how many" when the caller's buffer is
    // missing or too small, and fill it otherwise.
    template <typename T>
    static size_t fillSpan(const T* items, size_t count, T* out, size_t capacity) {
        if (out && capacity >= count && count != 0) std::copy(items, items + count, out);
        return count;
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
