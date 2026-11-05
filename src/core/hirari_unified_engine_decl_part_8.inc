    static constexpr size_t kMaxMacros = 128;

public:
    // Region edit host callbacks. The Rust region store owns the table, so
    // these only expose the engine surface it cannot reach: the track
    // registry, media decode and the pitch-analysis gate. Decoded-buffer
    // access lives on Track as the region storage host.
    static size_t regionHostListTracks(void* host, uint32_t* out, size_t capacity);
    static void* regionHostTempoState(void* host);
    static void* regionHostUndoState(void* host);
    static double regionHostSampleRate(void* host);
    static size_t regionHostTrackIdentity(void* host, uint32_t trackId);
    static void* regionHostRetainTrack(void* host, uint32_t trackId);
    static void regionHostReleaseTrack(void* handle);
    static bool regionHostPitchBegin(void* trackHandle);
    static void regionHostPitchFinish(void* trackHandle);
    static void* regionHostNoteSnapshot(void* trackHandle, uint32_t regionId);
};
