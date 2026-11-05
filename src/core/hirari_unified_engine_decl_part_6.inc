public:
    static constexpr uint32_t kMaxTracks = 256;
    static constexpr uint32_t kMaxTrackMap = 2048;

private:
    static constexpr uint32_t kNoInputMonitorTrack = std::numeric_limits<uint32_t>::max();

    // Every store the graph, the realtime callback and the control surface read
    // is a slot in one Rust bag (engine_session.rs), which owns their creation
    // and teardown order. The engine holds the bag and asks it for a handle; the
    // bag also records this engine's address, so a Rust routine handed the host
    // pointer reaches the same stores without a C++ translation of its own.
    void* m_session = hirari_engine_session_create(this);
    void* handle(uint32_t kind) const noexcept {
        return hirari_engine_session_handle(m_session, kind);
    }
