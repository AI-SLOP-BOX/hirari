/**
 * @class ExternalPluginHost
 * @brief Unified Bridge for Internal and External Processors.
 */
class ExternalPluginHost : public DSP::IProcessor {
public:
    enum class Format { Internal, VST3, AU, CLAP };
    enum class LoadState { Unloaded, Operational, Failed, Unsupported };

    ExternalPluginHost(const std::string& path, Format format) : m_path(path), m_format(format) {
        for (auto& v : m_paramValues) v.store(0.0f);
        for (auto& mask : m_dirtyMask) mask.store(0, std::memory_order_relaxed);

        // Instantiate Internal Plugin if path matches
        if (m_format == Format::Internal) {
            if (m_path == "Aura/Limiter") m_internal = std::make_unique<DSP::Effects::ProLimiter>();
            else if (m_path == "Aura/SubBass") m_internal = std::make_unique<DSP::Effects::SubBassGenerator>();
            else if (m_path == "Aura/Compressor") m_internal = std::make_unique<DSP::Effects::DynamicCompressor>();
            else if (m_path == "Aura/Gate") m_internal = std::make_unique<NativeNoiseGate>();
            else if (m_path == "Aura/Saturation") m_internal = std::make_unique<DSP::Effects::TubeSaturation>();
            else if (m_path == "Aura/Transient") m_internal = std::make_unique<NativeTransientShaper>();
            else if (m_path == "Aura/DeEsser") m_internal = std::make_unique<NativeDeEsser>();
            else if (m_path == "Aura/Delay") m_internal = std::make_unique<NativeDelay>();
            else if (m_path == "Aura/Reverb") m_internal = std::make_unique<NativeReverb>();
            else if (m_path == "Aura/DynamicEQ") m_internal = std::make_unique<NativeDynamicEq>();
            else if (m_path == "Aura/MidSide") m_internal = std::make_unique<NativeMidSide>();
            else if (m_path == "Aura/Width") m_internal = std::make_unique<NativeStereoWidth>();
            // NeuralCloner is not part of the available processor interface;
            // leave it failed rather than manufacturing an external bridge.
            m_state = m_internal ? LoadState::Operational : LoadState::Failed;
            if (!m_internal) m_error = "unknown internal plugin: " + m_path;
        } else {
#if defined(AURA_ENABLE_VST3_SDK)
            if (m_format == Format::VST3) {
                m_vst3 = std::make_unique<Plugins::VST3HostProcessor>();
                if (!m_vst3->loadVst3(m_path)) {
                    m_state = LoadState::Failed;
                    m_error = m_vst3->lastError();
                } else {
                    m_state = LoadState::Operational;
                }
            } else
#endif
            {
            // All external formats use the same process boundary.  The
            // worker selects the ABI adapter from the bundle/path, while the
            // host keeps one lifecycle and failure contract for VST3, AU,
            // and CLAP instead of silently treating them as pass-through.
            m_external = std::make_unique<Plugins::ProcessSandboxProcessor>(m_path);
            m_state = m_external ? LoadState::Unloaded : LoadState::Failed;
            if (!m_external) m_error = "failed to allocate external plugin sandbox";
            }
        }
        // The host-side atomics are the only control/audio exchange for
        // parameters.  Seed them once before the processor becomes visible;
        // subsequent updates are applied by the audio thread at a block
        // boundary, avoiding a concurrent direct call into DSP state.
        if (m_internal) {
            for (uint32_t id = 0; id < m_paramValues.size(); ++id) {
                m_paramValues[id].store(m_internal->getParameter(id),
                                        std::memory_order_relaxed);
            }
        }
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        if (m_internal) m_internal->prepareToPlay(sr, bs);
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) m_vst3->prepareToPlay(sr, bs);
#endif
        if (m_external) {
            m_external->prepareToPlay(sr, bs);
            if (!m_external->isAlive() && !m_external->start()) {
                m_state = LoadState::Failed;
            } else {
                m_state = LoadState::Operational;
            }
        }
    }

    uint32_t getLatencySamples() const noexcept override {
        if (m_internal) return m_internal->getLatencySamples();
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->getLatencySamples();
#endif
        return m_external ? m_external->getLatencySamples() : 0u;
    }

    uint32_t getTailSamples() const noexcept override {
        if (m_internal) return m_internal->getTailSamples();
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->getTailSamples();
#endif
        return m_external ? m_external->getTailSamples() : 0u;
    }

    void process(AudioBuffer& b, MidiBuffer& midi, const DSP::ProcessContext& context) noexcept override {
        if (isBypassed()) return;

        // --- INTERNAL PLUG-IN EXECUTION ---
        if (m_internal) {
            // INDUSTRIAL PARAMETER DISPATCH (Bitmask Optimized)
            for (int m = 0; m < 2; ++m) {
                uint64_t dirty = m_dirtyMask[m].exchange(0, std::memory_order_acq_rel);
                if (dirty != 0) {
                    for (int i = 0; i < 64; ++i) {
                        if (dirty & (1ULL << i)) {
                            uint32_t paramId = m * 64 + i;
                            m_internal->setParameter(paramId, m_paramValues[paramId].load(std::memory_order_relaxed));
                        }
                    }
                }
            }
            m_internal->process(b, midi, context);
            return;
        }

#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) {
            m_vst3->process(b, midi, context);
            if (m_vst3->processFailed())
                m_processFailed.store(true, std::memory_order_release);
            return;
        }
#endif

        if (m_external) {
            m_external->process(b, midi, context);
            if (m_external->processFailed()) {
                // process() is the realtime boundary: publish only an atomic
                // edge here. State text and lifecycle state are collected by
                // pollExternalHealth() on the control thread.
                m_processFailed.store(true, std::memory_order_release);
            }
            return;
        }
        m_processFailed.store(true, std::memory_order_release);
    }

    void reset() noexcept override {
        if (m_internal) m_internal->reset();
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) m_vst3->reset();
#endif
        // External reset is intentionally a lifecycle no-op: stopping the
        // worker here would make every graph reset an implicit plugin crash.
    }
    std::vector<uint8_t> getState() const override {
        if (m_internal) {
            auto state = m_internal->getState();
            // A few legacy built-ins have no state serializer. Preserve a
            // deterministic control-plane snapshot rather than publishing an
            // empty blob; parameter values are restored independently.
            if (state.empty() && m_internal->getNumParameters() > 0) {
                state.resize(sizeof(float));
                const float value = m_paramValues[0].load(std::memory_order_relaxed);
                std::memcpy(state.data(), &value, sizeof(float));
            }
            return state;
        }
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->getState();
#endif
        return m_external ? m_external->getState() : std::vector<uint8_t>{};
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (m_internal) {
            const bool restored = m_internal->setState(state);
            if (restored) {
                for (uint32_t id = 0; id < m_internal->getNumParameters() && id < 128; ++id)
                    m_paramValues[id].store(m_internal->getParameter(id), std::memory_order_relaxed);
            }
            return restored;
        }
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->setState(state);
#endif
        return m_external && m_external->setState(state);
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        if (m_internal) {
            // Legacy built-in processors expose validated restoration through
            // setState rather than the newer restoreStateChecked hook.
            if (m_internal->restoreStateChecked(state)) {
                for (uint32_t id = 0; id < m_internal->getNumParameters() && id < 128; ++id)
                    m_paramValues[id].store(m_internal->getParameter(id), std::memory_order_relaxed);
                return true;
            }
            return setState(state);
        }
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->restoreStateChecked(state);
#endif
        return m_external && m_external->restoreStateChecked(state);
    }
    
    void setParameter(uint32_t id, float value) noexcept override {
        if (id < 128 && std::isfinite(value)) {
            m_paramValues[id].store(value, std::memory_order_relaxed);
            // Built-in processors are control-thread-owned and expose their
            // state synchronously. Keep their serializer in step with the
            // mirror so preset/project saves immediately capture edits made
            // between audio blocks.
            if (m_internal) m_internal->setParameter(id, value);
            // Keep the control-thread state authoritative for project saves and
            // UI reads. The audio thread is the sole caller that mutates the
            // internal processor, and applies the value at a block boundary.
            // A parameter may legitimately be edited before the isolated
            // worker has started.  ProcessSandboxProcessor retains the value
            // and replays it at the next lifecycle boundary; only a live
            // worker with a genuinely full mailbox is a queue failure.
            if (m_external) {
                const bool queued = m_external->enqueueParameterChange(id, value);
                if (!queued && m_external->isAlive()) {
                    m_parameterQueueFailed.store(true, std::memory_order_release);
                }
            }
            uint32_t maskIdx = id / 64;
            uint32_t bitIdx = id % 64;
            m_dirtyMask[maskIdx].fetch_or(1ULL << bitIdx, std::memory_order_release);
        }
    }

    float getParameter(uint32_t id) const noexcept override {
        // The control-side atomic is authoritative between audio blocks. The
        // internal DSP instance is updated at the next block boundary, so
        // reading it here makes UI/CLI callers observe a stale value.
        if (m_internal) return (id < 128) ? m_paramValues[id].load(std::memory_order_relaxed) : 0.0f;
        return (id < 128) ? m_paramValues[id].load(std::memory_order_relaxed) : 0.0f;
    }

    bool isOperational() const noexcept { return m_state == LoadState::Operational; }
    LoadState loadState() const noexcept { return m_state; }
    bool hasError() const noexcept { return m_state == LoadState::Failed || m_state == LoadState::Unsupported; }
    const std::string& errorMessage() const noexcept { return m_error; }
    bool processFailed() const noexcept {
        return m_processFailed.load(std::memory_order_acquire) ||
               m_parameterQueueFailed.load(std::memory_order_acquire);
    }
    std::string lastError() const {
        if (processFailed() && m_error.empty()) {
            return "external plugin process is unavailable";
        }
        return m_error;
    }
    const std::string& path() const { return m_path; }

    bool hasNativeEditor() const noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->hasNativeEditor();
#endif
        return false;
    }
    uint64_t openNativeEditor(uintptr_t parent) noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        return m_vst3 ? m_vst3->openNativeEditor(parent) : 0;
#else
        (void)parent;
        return 0;
#endif
    }
    bool closeNativeEditor(uint64_t session) noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        return m_vst3 && m_vst3->closeNativeEditor(session);
#else
        (void)session;
        return false;
#endif
    }

    bool startExternal() {
        if (!m_external) return false;
        const bool started = m_external->start();
        m_state = started ? LoadState::Operational : LoadState::Failed;
        if (!started) {
            m_error = m_external->failureText();
        } else {
            m_processFailed.store(false, std::memory_order_release);
            m_parameterQueueFailed.store(false, std::memory_order_release);
            m_error.clear();
        }
        return started;
    }
    void stopExternal() noexcept {
        if (m_external) m_external->stop();
        if (m_state == LoadState::Operational) m_state = LoadState::Unloaded;
    }
    bool pollExternalHealth() {
        if (!m_external) return false;
        const bool healthy = m_external->pollHealth();
        if (!healthy) {
            m_state = LoadState::Failed;
            m_error = m_external->failureText();
        }
        return healthy;
    }

private:
    std::string m_path;
    Format m_format;
    std::unique_ptr<DSP::IProcessor> m_internal;
    std::unique_ptr<Plugins::ProcessSandboxProcessor> m_external;
#if defined(AURA_ENABLE_VST3_SDK)
    std::unique_ptr<Plugins::VST3HostProcessor> m_vst3;
#endif
    LoadState m_state = LoadState::Unloaded;
    std::string m_error;

    // Thread-safe parameter state (Industrial Bitmask)
    std::array<std::atomic<float>, 128> m_paramValues;
    std::atomic<uint64_t> m_dirtyMask[2];
    // Set from the RT callback without allocating or mutating diagnostic text.
    std::atomic<bool> m_processFailed{false};
    std::atomic<bool> m_parameterQueueFailed{false};
};

