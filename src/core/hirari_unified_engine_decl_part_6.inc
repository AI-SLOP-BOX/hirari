public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    static constexpr uint32_t kNoInputMonitorTrack = std::numeric_limits<uint32_t>::max();

    // Every store the graph, the realtime callback and the control surface read
    // is a slot in one Rust bag (engine_session.rs), which owns their creation
    // and teardown order. The engine holds the bag and asks it for a handle.
    void* m_session = hirari_engine_session_create();
    void* handle(uint32_t kind) const noexcept {
        return hirari_engine_session_handle(m_session, kind);
    }
