    // DSP State (moved to pointer to decouple header)
    std::unique_ptr<::Hirari::DSP::ProcessContext> m_context;
    // Both DSP objects are Rust-owned; the engine keeps only their handles.
    void* m_meterCore = hirari_master_meter_create(44'100.0);
    void* m_masterLimiter = hirari_master_limiter_create(44'100.0);

    TelemetryData m_telemetryBuffers[2];
    // Per-engine Rust telemetry is allocated on the control thread and reused
    // by the audio callback; it must not lazily initialize on first process.
    void* m_mixerTelemetry = hirari_mixer_telemetry_create();
    std::atomic<uint32_t> m_activeTelemetryIdx{0};
    // Even values are stable snapshots; audio publication brackets writes
    // with odd/even values so control-rate readers can retry instead of
    // observing a buffer while the next audio block is overwriting it.
    std::atomic<uint64_t> m_telemetrySequence{0};
    mutable std::atomic_flag m_telemetryCopyLock = ATOMIC_FLAG_INIT;
    
    std::atomic<bool> m_isShutdown{false};
    // Project hydration mutates tracks, routing and several process-wide
    // managers as one control-plane transaction.  Reject a concurrent load
    // instead of allowing two documents to interleave partial state.
    std::atomic<bool> m_projectHydrationActive{false};
    std::atomic<bool> m_offlineRenderActive{false};
    std::atomic<uint64_t> m_offlineRenderOwner{0};
    std::atomic<bool> m_offlineTargetActive{false};
    std::atomic<uint32_t> m_offlineTargetId{0};
    std::atomic<uint32_t> m_offlineRenderTailMillis{0};
    std::atomic<bool> m_offlineRenderPreFader{false};
    std::atomic<bool> m_offlineRenderIncludeInserts{true};
    std::atomic<bool> m_offlineRenderRangeActive{false};
    std::atomic<uint64_t> m_offlineRenderStartSample{0};
    std::atomic<uint64_t> m_offlineRenderEndSample{0};
    std::atomic<bool> m_bounceRunning{false};
    std::atomic<float> m_bounceProgress{0.0f};
    std::atomic<uint32_t> m_bounceState{static_cast<uint32_t>(BounceState::Idle)};
    std::atomic<bool> m_bounceCancelRequested{false};
    std::atomic<bool> m_bouncePauseRequested{false};
    // Serializes cancellation with the final temporary-file publish so a
    // cancel cannot race a stale render into the destination path.
    mutable std::mutex m_bouncePublishMutex;
    // Serializes render thread retirement with a subsequent start. The
    // worker publishes Failed/Completed before its wrapper flips
    // m_bounceRunning, so callers must be able to join a terminal worker
    // before accepting the next render.
    mutable std::mutex m_bounceLifecycleMutex;
    std::thread m_bounceThread;
    std::shared_ptr<ProjectLayoutRevision> m_projectLayoutRevision =
        std::make_shared<ProjectLayoutRevision>();
    std::atomic<uint64_t> m_projectControlRevision{0};
    std::vector<std::shared_ptr<Track>> m_tracks;
    std::vector<std::shared_ptr<Track>> m_deferredDestructionTracks;
    mutable std::mutex m_tracksMutex;

    // Id resolution shared by the region, routing, control and plugin host
    // callbacks. Callers must already hold m_tracksMutex. The const views reuse
    // the same single traversal rather than carrying a second copy of it.
    Track* findTrackLocked(uint32_t tid);
    std::shared_ptr<Track> findTrackSharedLocked(uint32_t tid);
    const Track* findTrackLocked(uint32_t tid) const {
        return const_cast<HirariUnifiedEngine*>(this)->findTrackLocked(tid);
    }
    std::shared_ptr<Track> findTrackSharedLocked(uint32_t tid) const {
        return const_cast<HirariUnifiedEngine*>(this)->findTrackSharedLocked(tid);
    }
    const Region* findRegionLocked(uint32_t tid, uint32_t rid) const;
    // Track lookup in the published audio snapshot. Used by control callbacks
    // that do not hold m_tracksMutex.
    Track* findPublishedTrack(uint32_t tid) const;

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

    // Reads through the published track snapshot and returns `missing` when the
    // id is gone; each query keeps its own fallback instead of repeating the
    // scan. The not-found messages themselves are decided in Rust.
    template <typename Fn>
    std::invoke_result_t<Fn, const Track&> querySnapshotTrack(
        uint32_t tid, std::invoke_result_t<Fn, const Track&> missing, const Fn& fn) const {
        for (const auto& track : get_tracks_snapshot()) {
            if (track && track->getId() == tid) return fn(*track);
        }
        return missing;
    }

    // Copies the owning handle out from under the lock so a call that can enter
    // vendor UI code holds the Track without the project topology lock.
    std::shared_ptr<Track> takeTrackHandle(uint32_t tid) const {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        return findTrackSharedLocked(tid);
    }

    // Region resolution for the host callbacks, on the same single traversal.
    template <typename Fn>
    static std::invoke_result_t<Fn, const Region&> withHostRegion(
        void* host, uint32_t tid, uint32_t rid,
        std::invoke_result_t<Fn, const Region&> missing, const Fn& fn) {
        if (!host) return missing;
        auto* engine = static_cast<HirariUnifiedEngine*>(host);
        std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
        const Region* region = engine->findRegionLocked(tid, rid);
        return region != nullptr ? fn(*region) : missing;
    }

    // Variable-length host reads answer "how many" when the caller's buffer is
    // missing or too small, and fill it otherwise.
    template <typename T>
    static size_t fillSpan(const T* items, size_t count, T* out, size_t capacity) {
        if (out && capacity >= count && count != 0) std::copy(items, items + count, out);
        return count;
    }

    // Same contract for spans that need a projection before they can be copied.
    template <typename Range, typename T, typename Project>
    static size_t fillProjected(const Range& range, T* out, size_t capacity,
                                const Project& project) {
        size_t count = 0;
        for (const auto& item : range) {
            if (out && count < capacity) out[count] = project(item);
            ++count;
        }
        return count;
    }

    // Sidechain route bookkeeping is project state owned by Rust
    // (sidechain_route_store.rs); the engine keeps only the opaque handle.
    void* m_sidechainRoutes = hirari_sidechain_routes_create();

    // Rust owns the layout snapshot (layout_gather.rs); this only exposes the
    // live-track accessors it reads through.
    const HirariLayoutHostOps* layoutHostOps() const;
    Track* layoutTrackAt(size_t index) const;

    void* m_midiSnapshotStore = nullptr;

    // Zero is reserved as an invalid/unset region identity in serialized
    // projects and FFI calls. All runtime-created regions start at one.
    std::atomic<uint32_t> m_regionIdCounter{1};
    std::atomic<uint32_t> m_trackIdCounter{0};
    std::atomic<uint32_t> m_autoRecordMode{0};
    std::atomic<int32_t> m_rootNote{0}, m_scaleType{0};
