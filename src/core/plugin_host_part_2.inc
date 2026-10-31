/**
 * @class ExternalPluginHost
 * @brief Processor host for the built-in native catalog.
 *
 * External VST3/AU/CLAP binaries never travel through this class: the engine
 * builds `ProcessSandboxProcessor` directly for them. Only built-in catalog
 * paths are admitted here, so the class is a native processor plus the
 * host-side parameter mirror consumed by the control plane.
 */
class ExternalPluginHost : public DSP::IProcessor {
public:
    enum class Format { Internal, VST3, AU, CLAP };
    enum class LoadState { Unloaded, Operational, Failed };

    ExternalPluginHost(const std::string& path, Format format) {
        for (auto& v : m_paramValues) v.store(0.0f);
        for (auto& mask : m_dirtyMask) mask.store(0, std::memory_order_relaxed);

        if (format == Format::Internal) {
            const int32_t nativeKind = hirari_native_processor_kind_for_path(
                reinterpret_cast<const uint8_t*>(path.data()), path.size());
            if (nativeKind >= 0) m_internal = std::make_unique<NativeProcessor>(nativeKind);
        }
        m_state = m_internal ? LoadState::Operational : LoadState::Failed;
        // The host-side atomics are the only control/audio exchange for
        // parameters. Seed them once before the processor becomes visible;
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
    }

    uint32_t getLatencySamples() const noexcept override {
        return m_internal ? m_internal->getLatencySamples() : 0u;
    }

    uint32_t getTailSamples() const noexcept override {
        return m_internal ? m_internal->getTailSamples() : 0u;
    }

    void process(AudioBuffer& b, MidiBuffer& midi, const DSP::ProcessContext& context) noexcept override {
        if (isBypassed() || !m_internal) return;
        // INDUSTRIAL PARAMETER DISPATCH (Bitmask Optimized)
        for (int m = 0; m < 2; ++m) {
            uint64_t dirty = m_dirtyMask[m].exchange(0, std::memory_order_acq_rel);
            if (dirty != 0) {
                for (int i = 0; i < 64; ++i) {
                    if (dirty & (1ULL << i)) {
                        uint32_t paramId = m * 64 + i;
                        m_internal->setParameter(
                            paramId, m_paramValues[paramId].load(std::memory_order_relaxed));
                    }
                }
            }
        }
        m_internal->process(b, midi, context);
    }

    void reset() noexcept override {
        if (m_internal) m_internal->reset();
    }

    std::vector<uint8_t> getState() const override {
        if (!m_internal) return {};
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

    bool setState(const std::vector<uint8_t>& state) override {
        if (!m_internal) return false;
        const bool restored = m_internal->setState(state);
        if (restored) {
            for (uint32_t id = 0; id < m_internal->getNumParameters() && id < 128; ++id)
                m_paramValues[id].store(m_internal->getParameter(id), std::memory_order_relaxed);
        }
        return restored;
    }

    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        if (!m_internal) return false;
        // Legacy built-in processors expose validated restoration through
        // setState rather than the newer restoreStateChecked hook.
        if (m_internal->restoreStateChecked(state)) {
            for (uint32_t id = 0; id < m_internal->getNumParameters() && id < 128; ++id)
                m_paramValues[id].store(m_internal->getParameter(id), std::memory_order_relaxed);
            return true;
        }
        return setState(state);
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id < 128 && std::isfinite(value)) {
            m_paramValues[id].store(value, std::memory_order_relaxed);
            // Built-in processors are control-thread-owned and expose their
            // state synchronously. Keep their serializer in step with the
            // mirror so preset/project saves immediately capture edits made
            // between audio blocks.
            if (m_internal) m_internal->setParameter(id, value);
            uint32_t maskIdx = id / 64;
            uint32_t bitIdx = id % 64;
            m_dirtyMask[maskIdx].fetch_or(1ULL << bitIdx, std::memory_order_release);
        }
    }

    float getParameter(uint32_t id) const noexcept override {
        // The control-side atomic is authoritative between audio blocks. The
        // internal DSP instance is updated at the next block boundary, so
        // reading it here makes UI/CLI callers observe a stale value.
        return (id < 128) ? m_paramValues[id].load(std::memory_order_relaxed) : 0.0f;
    }

    bool isOperational() const noexcept { return m_state == LoadState::Operational; }

private:
    std::unique_ptr<DSP::IProcessor> m_internal;
    LoadState m_state = LoadState::Unloaded;

    // Thread-safe parameter state (Industrial Bitmask)
    std::array<std::atomic<float>, 128> m_paramValues;
    std::atomic<uint64_t> m_dirtyMask[2];
};

