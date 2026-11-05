public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    static constexpr uint32_t kNoInputMonitorTrack = std::numeric_limits<uint32_t>::max();

    // Every store the graph, the realtime callback and the control surface read
    // is a slot in one Rust bag (engine_session.rs), which owns their creation
    // and teardown order. The engine holds the bag and asks it for a handle; the
    // bag also records this engine's address, so a Rust routine handed the host
    // pointer reaches the same stores without a C++ translation of its own. The
    // tracks mutex travels with it because a Rust graph walk has to be guarded
    // by the same lock the edits in this class still take.
    // Declared before the bag so its address is a valid object when the bag
    // records it: a Rust graph walk locks the very same mutex these edits do.
    mutable std::mutex m_tracksMutex;
    void* m_session = hirari_engine_session_create(this, &m_tracksMutex);
    void* handle(uint32_t kind) const noexcept {
        return hirari_engine_session_handle(m_session, kind);
    }
