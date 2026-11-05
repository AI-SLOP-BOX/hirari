    // Project hydration mutates tracks, routing and several process-wide
    // managers as one control-plane transaction.  Reject a concurrent load
    // instead of allowing two documents to interleave partial state.
    std::atomic<bool> m_projectHydrationActive{false};
    mutable std::mutex m_tracksMutex;

    // The document graph lives in the Rust track table (engine_track_table.rs)
    // and its tracks are Rust objects (engine_track.rs). A track pointer is the
    // Arc's own address: it keys the region store and is what every
    // hirari_track_* entry point takes. Callers must already hold m_tracksMutex,
    // except findPublishedTrack, which reads the lock-free snapshot.
    void* findTrackLocked(uint32_t tid) const {
        return hirari_engine_track_find(handle(kHirariHandleTrackTable), tid);
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
        auto* table = handle(kHirariHandleTrackTable);
        return { hirari_engine_track_ptrs(table), hirari_engine_track_count(table) };
    }
    void* lockedTrackAt(size_t index) const noexcept {
        const TrackRange tracks = lockedTracks();
        return index < tracks.count ? tracks.items[index] : nullptr;
    }

    // A copy of the track that outlives the topology lock: an operation that can
    // block on a worker or vendor UI code holds this and releases it when done.
    void* takeTrackOwner(uint32_t tid) const {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        return hirari_engine_track_retain(handle(kHirariHandleTrackTable), tid);
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

    // Rust owns the layout snapshot (layout_gather.rs); this only exposes the
    // live-track accessors it reads through.
    const HirariLayoutHostOps* layoutHostOps() const;
