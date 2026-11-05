public:
    // One block-size contract shared by the native graph and every bridge
    // entry point. Keeping this in the public engine boundary prevents the
    // FFI from silently rejecting a block size that the configured engine can
    // process, or accepting a size the graph does not bound.
    static constexpr uint32_t kMaxAudioBlockSize = 16'384u;

    HirariUnifiedEngine();
    ~HirariUnifiedEngine();

    void processBlock(void* output, uint32_t offset, uint32_t size);
    void processBlockDirect(float** buffers, uint32_t numChannels, uint32_t numSamples);
    // The hardware input publish and the config transaction are Rust-side now
    // (engine_config.rs): the driver callback stages its buffers through the
    // input-block externs and prepareToPlay is called directly, so no relay
    // method forwards either. The callback-join wait is engine-block state, so
    // the facade holds no in-flight counter of its own.
    void prepareToPlay(double sr, uint32_t sz);
    void syncStructuralChanges();
