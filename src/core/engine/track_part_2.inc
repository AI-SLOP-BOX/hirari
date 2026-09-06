    void setVolume(float v) {
        invalidateFrozenForEdit();
        const float next = std::isfinite(v) ? std::clamp(v, 0.0f, 2.0f) : 1.0f;
        m_volume.store(next,
                       std::memory_order_release);
        m_strip.setGain(v);
    }
    void setPan(float p) {
        invalidateFrozenForEdit();
        const float next = std::isfinite(p) ? std::clamp(p, -1.0f, 1.0f) : 0.0f;
        m_pan.store(next, std::memory_order_release);
        m_strip.setPan(next);
    }
    virtual bool addPlugin(uint32_t pluginType) {
        // The only currently operational insertion is the built-in Aura
        // limiter. External formats must fail explicitly; retaining metadata
        // for an instance that cannot process audio is misleading.
        if (pluginType > 10) return false;

        const char* pluginPath = "Aura/Limiter";
        if (pluginType == 1) pluginPath = "Aura/Compressor";
        else if (pluginType == 2) pluginPath = "Aura/Gate";
        else if (pluginType == 3) pluginPath = "Aura/Saturation";
        else if (pluginType == 4) pluginPath = "Aura/Transient";
        else if (pluginType == 5) pluginPath = "Aura/DeEsser";
        else if (pluginType == 6) pluginPath = "Aura/Delay";
        else if (pluginType == 7) pluginPath = "Aura/Reverb";
        else if (pluginType == 8) pluginPath = "Aura/DynamicEQ";
        else if (pluginType == 9) pluginPath = "Aura/MidSide";
        else if (pluginType == 10) pluginPath = "Aura/Width";
        auto processor = std::make_shared<Aura::Core::Plugin::ExternalPluginHost>(
            pluginPath,
            Aura::Core::Plugin::ExternalPluginHost::Format::Internal);
        if (!processor->isOperational()) return false;

        m_effectChain.addProcessor(std::move(processor));
        invalidateFrozenForEdit();
        m_pluginTypes.push_back(pluginType);
        if (m_isPrepared) {
            m_effectChain.setSampleRate(m_sampleRate, m_blockSize);
        }
        return true;
    }

    struct PluginSnapshot {
        uint32_t type = 0;
        std::shared_ptr<DSP::IProcessor> processor;
        bool bypassed = false;
        bool parallel = false;
        std::string sandboxPath;
    };

    bool removePlugin(uint32_t pluginIndex, PluginSnapshot* snapshot = nullptr) {
        if (pluginIndex >= m_pluginTypes.size()) return false;
        EffectChain::Entry removed{};
        if (!m_effectChain.removeProcessor(pluginIndex, &removed)) return false;
        if (snapshot) {
            snapshot->type = m_pluginTypes[pluginIndex];
            snapshot->processor = removed.processor;
            snapshot->bypassed = removed.bypassed;
            snapshot->parallel = removed.parallel;
            if (snapshot->type == 0xFFFFFFFFu) {
                for (size_t i = 0; i < m_sandboxedPluginIndices.size(); ++i) {
                    if (m_sandboxedPluginIndices[i] == pluginIndex && i < m_sandboxedPluginPaths.size()) {
                        snapshot->sandboxPath = m_sandboxedPluginPaths[i];
                        break;
                    }
                }
            }
        }
        m_pluginTypes.erase(m_pluginTypes.begin() + pluginIndex);
        invalidateFrozenForEdit();
        for (size_t i = 0; i < m_sandboxedPluginIndices.size();) {
            auto& sandboxIndex = m_sandboxedPluginIndices[i];
            if (sandboxIndex == pluginIndex) {
                m_sandboxedPluginIndices.erase(m_sandboxedPluginIndices.begin() + i);
                if (i < m_sandboxedPluginPaths.size())
                    m_sandboxedPluginPaths.erase(m_sandboxedPluginPaths.begin() + i);
                continue;
            }
            if (sandboxIndex > pluginIndex) --sandboxIndex;
            ++i;
        }
        return true;
    }

    bool restorePlugin(uint32_t pluginIndex, const PluginSnapshot& snapshot) {
        if (!snapshot.processor || pluginIndex > m_pluginTypes.size()) return false;
        EffectChain::Entry entry{snapshot.processor, snapshot.bypassed, snapshot.parallel};
        if (!m_effectChain.insertProcessor(pluginIndex, std::move(entry))) return false;
        for (auto& sandboxIndex : m_sandboxedPluginIndices)
            if (sandboxIndex >= pluginIndex) ++sandboxIndex;
        m_pluginTypes.insert(m_pluginTypes.begin() + pluginIndex, snapshot.type);
        if (snapshot.type == 0xFFFFFFFFu) {
            size_t offset = 0;
            while (offset < m_sandboxedPluginIndices.size() &&
                   m_sandboxedPluginIndices[offset] < pluginIndex) ++offset;
            m_sandboxedPluginIndices.insert(m_sandboxedPluginIndices.begin() + offset, pluginIndex);
            m_sandboxedPluginPaths.insert(m_sandboxedPluginPaths.begin() + offset, snapshot.sandboxPath);
        }
        invalidateFrozenForEdit();
        return true;
    }

    bool movePlugin(uint32_t from, uint32_t to) {
        if (from >= m_pluginTypes.size() || to >= m_pluginTypes.size() || from == to)
            return false;
        if (!m_effectChain.moveProcessor(from, to)) return false;
        const uint32_t type = m_pluginTypes[from];
        m_pluginTypes.erase(m_pluginTypes.begin() + from);
        m_pluginTypes.insert(m_pluginTypes.begin() + to, type);

        std::vector<std::pair<uint32_t, std::string>> sandbox;
        sandbox.reserve(m_sandboxedPluginIndices.size());
        for (size_t i = 0; i < m_sandboxedPluginIndices.size(); ++i) {
            if (i >= m_sandboxedPluginPaths.size()) continue;
            uint32_t index = m_sandboxedPluginIndices[i];
            if (index == from) index = to;
            else if (from < to && index > from && index <= to) --index;
            else if (to < from && index >= to && index < from) ++index;
            sandbox.emplace_back(index, m_sandboxedPluginPaths[i]);
        }
        std::sort(sandbox.begin(), sandbox.end(),
                  [](const auto& lhs, const auto& rhs) { return lhs.first < rhs.first; });
        m_sandboxedPluginIndices.clear();
        m_sandboxedPluginPaths.clear();
        for (auto& entry : sandbox) {
            m_sandboxedPluginIndices.push_back(entry.first);
            m_sandboxedPluginPaths.push_back(std::move(entry.second));
        }
        invalidateFrozenForEdit();
        return true;
    }

    bool addSandboxedPlugin(const std::string& pluginPath) {
        if (pluginPath.empty()) {
            m_lastSandboxFailureText = "invalid-plugin-path";
            return false;
        }
        const std::filesystem::path path(pluginPath);
        const bool builtin = pluginPath.rfind("builtin://", 0) == 0;
        if (!builtin) {
            std::error_code ec;
            // macOS CLAP/AU/VST3 entries are directory bundles, not regular
            // files.  Validate existence here and let the isolated worker
            // resolve the bundle's executable image.
            if (!std::filesystem::exists(path, ec) || ec) {
                m_lastSandboxFailureText = "plugin-path-missing";
                return false;
            }
            const auto extension = path.extension().string();
#if defined(__APPLE__)
            if (extension != ".clap" && extension != ".vst3" && extension != ".component") {
                m_lastSandboxFailureText = "plugin-format-unsupported";
                return false;
            }
#else
            if (extension != ".clap" && extension != ".vst3") {
                m_lastSandboxFailureText = "plugin-format-unsupported";
                return false;
            }
#endif
        }
        if (std::find(m_sandboxedPluginPaths.begin(), m_sandboxedPluginPaths.end(), pluginPath) !=
            m_sandboxedPluginPaths.end()) {
            m_lastSandboxFailureText = "duplicate-plugin-path";
            return false;
        }
        auto processor = std::make_shared<Aura::Core::Plugins::ProcessSandboxProcessor>(
            pluginPath, m_sampleRate, std::max<uint32_t>(m_blockSize, 1u));
        bool started = false;
        // Worker creation is a control-thread operation. On macOS, a worker
        // that just exited may leave code-signature validation and IPC
        // resources briefly unavailable to the next launch. Retry only those
        // transport-level failures, and explicitly close the failed host
        // before trying again. Plugin ABI/load/instance failures remain
        // terminal and are never hidden by retrying.
        constexpr int kMaxTransportRetries = 5;
        for (int attempt = 0; attempt < kMaxTransportRetries && !started; ++attempt) {
            started = processor->start();
            if (started || attempt + 1 == kMaxTransportRetries) break;
            const auto failure = processor->failure();
            const bool transient =
                failure == Aura::Core::Plugins::PluginSandboxHost::Failure::SpawnFailed ||
                failure == Aura::Core::Plugins::PluginSandboxHost::Failure::ProcessExited ||
                failure == Aura::Core::Plugins::PluginSandboxHost::Failure::HandshakeTimeout;
            if (!transient) break;
            processor->stop();
            const auto backoff = std::chrono::milliseconds(25u << static_cast<unsigned>(attempt));
            std::this_thread::sleep_for(backoff);
        }
        if (!started) {
            m_lastSandboxFailure.store(processor->failureCode(), std::memory_order_release);
            m_lastSandboxFailureText = processor->failureCode() == 0
                ? "sandbox-start-failed"
                : processor->failureText();
            Aura::Core::Plugins::PluginCacheManager::getInstance().recordScanFailure(
                pluginPath, processor->failureCode());
            return false;
        }
        Aura::Core::Plugins::PluginCacheManager::getInstance().recordScanSuccess(pluginPath);
        m_lastSandboxFailure.store(0, std::memory_order_release);
        m_lastSandboxFailureText = "none";
        const uint32_t processorIndex = static_cast<uint32_t>(m_pluginTypes.size());
        if (m_isPrepared) processor->prepareToPlay(m_sampleRate, m_blockSize);
        m_effectChain.addProcessor(processor);
        m_pluginTypes.push_back(0xFFFFFFFFu);
        m_sandboxedPluginPaths.push_back(pluginPath);
        m_sandboxedPluginIndices.push_back(processorIndex);
        if (m_isPrepared) m_effectChain.setSampleRate(m_sampleRate, m_blockSize);
        return true;
    }

    // Control/test boundary for validating an isolated processor with a
    // caller-owned block. The normal transport still uses Track::process;
    // this method exists so integration tests can prove that shared-memory
    // plugin output is not merely a readiness signal.
    bool processSandboxedPluginBlock(size_t sandboxIndex, float* left, float* right,
                                     uint32_t frames) {
        if (!left || !right || frames == 0 || frames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames ||
            sandboxIndex >= m_sandboxedPluginIndices.size()) return false;
        const uint32_t pluginIndex = m_sandboxedPluginIndices[sandboxIndex];
        auto processor = m_effectChain.getProcessor(pluginIndex);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        if (!sandbox) return false;
        float* channels[2] = {left, right};
        AudioBuffer buffer;
        buffer.wrapChannels(channels, 2, frames);
        MidiBuffer midi;
        return sandbox->processBlock(buffer, midi);
    }
    std::vector<uint8_t> processSandboxedPluginMidiBlock(size_t sandboxIndex,
                                                         uint32_t frames,
                                                         const uint8_t* midiData,
                                                         uint32_t midiSize) {
        if (!midiData || midiSize == 0 || frames == 0 ||
            frames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames ||
            sandboxIndex >= m_sandboxedPluginIndices.size()) return {};
        const uint32_t pluginIndex = m_sandboxedPluginIndices[sandboxIndex];
        auto processor = m_effectChain.getProcessor(pluginIndex);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        if (!sandbox) return {};
        if (midiSize > sizeof(Aura::Core::Plugins::SandboxProtocol::MidiEvent::data)) {
            sandbox->recordInputMidiRejection();
            return {};
        }
        std::vector<float> left(frames, 0.0f), right(frames, 0.0f);
        float* channels[2] = {left.data(), right.data()};
        AudioBuffer buffer;
        buffer.wrapChannels(channels, 2, frames);
        MidiBuffer midi;
        midi.addEvent(0, midiData, midiSize);
        if (!sandbox->processBlock(buffer, midi)) return {};
        if (midi.size() < 2) return {};
        const auto& output = midi.getEvents()[midi.size() - 1];
        return std::vector<uint8_t>(output.data, output.data + output.size);
    }
    void reclaimEffectChainRetiredMemory() {
        m_effectChain.reclaimRetired();
    }
    uint32_t takeWatchdogTrips() {
        return m_effectChain.takeWatchdogTrips();
    }
    uint64_t nonFiniteSampleCount() const noexcept {
        return m_effectChain.nonFiniteSampleCount();
    }
    size_t getPluginCount() const { return m_pluginTypes.size(); }
    const std::vector<uint32_t>& getPluginTypes() const { return m_pluginTypes; }
    std::vector<std::vector<uint8_t>> getPluginStates() const {
        std::vector<std::vector<uint8_t>> states;
        states.reserve(m_pluginTypes.size());
        for (size_t index = 0; index < m_pluginTypes.size(); ++index)
            states.push_back(m_effectChain.getProcessorState(static_cast<uint32_t>(index)));
        return states;
    }
    bool setPluginState(size_t pluginIndex, const std::vector<uint8_t>& state) {
        const bool changed = m_effectChain.setProcessorState(static_cast<uint32_t>(pluginIndex), state);
        if (changed) invalidateFrozenForEdit();
        return changed;
    }
    std::vector<std::vector<uint8_t>> getPluginGuiStates() const {
        std::vector<std::vector<uint8_t>> states;
        states.reserve(m_pluginTypes.size());
        for (size_t index = 0; index < m_pluginTypes.size(); ++index) states.push_back(m_effectChain.getProcessorGuiState(static_cast<uint32_t>(index)));
        return states;
    }
    bool setPluginGuiState(size_t pluginIndex, const std::vector<uint8_t>& state) {
        if (pluginIndex >= m_pluginTypes.size() || state.size() > 1024u * 1024u) return false;
        return m_effectChain.setProcessorGuiState(static_cast<uint32_t>(pluginIndex), state);
    }
    const std::vector<std::string>& getSandboxedPluginPaths() const { return m_sandboxedPluginPaths; }
    std::vector<std::vector<uint8_t>> getSandboxedPluginStates() const {
        std::vector<std::vector<uint8_t>> states;
        states.reserve(m_sandboxedPluginIndices.size());
        for (const uint32_t index : m_sandboxedPluginIndices)
            states.push_back(m_effectChain.getProcessorState(index));
        return states;
    }
    std::vector<uint8_t> getSandboxedPluginState(size_t sandboxIndex) const {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return {};
        return m_effectChain.getProcessorState(m_sandboxedPluginIndices[sandboxIndex]);
    }
    bool setSandboxedPluginState(size_t sandboxIndex, const std::vector<uint8_t>& state) {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return false;
        return m_effectChain.setProcessorState(m_sandboxedPluginIndices[sandboxIndex], state);
    }
    uint8_t sandboxedPluginStateError(size_t sandboxIndex) const noexcept {
        if (sandboxIndex >= m_sandboxedPluginIndices.size())
            return Aura::Core::Plugins::SandboxProtocol::kStateErrorUnavailable;
        auto processor = m_effectChain.getProcessor(m_sandboxedPluginIndices[sandboxIndex]);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        return sandbox ? sandbox->stateErrorCode() : Aura::Core::Plugins::SandboxProtocol::kStateErrorUnavailable;
    }
    const char* sandboxedPluginStateErrorText(size_t sandboxIndex) const noexcept {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return "state-unavailable";
        auto processor = m_effectChain.getProcessor(m_sandboxedPluginIndices[sandboxIndex]);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        return sandbox ? sandbox->stateErrorText() : "state-unavailable";
    }
    bool pollSandboxedPluginHealth(size_t sandboxIndex) noexcept {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return false;
        const uint32_t index = m_sandboxedPluginIndices[sandboxIndex];
        if (index >= m_pluginTypes.size()) return false;
        // The sandbox processor is owned by the effect chain. Health checks
        // are control-thread maintenance and never run from processBlock().
        auto processor = m_effectChain.getProcessor(index);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        return sandbox && sandbox->pollHealth();
    }
    bool restartSandboxedPlugin(size_t sandboxIndex) {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return false;
        const uint32_t index = m_sandboxedPluginIndices[sandboxIndex];
        auto processor = m_effectChain.getProcessor(index);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        // This entry point is an explicit user/control action, not the
        // background maintenance retry.  It must be allowed to recover a
        // quarantined plugin and reset the bounded automatic-restart budget.
        return sandbox && sandbox->restart(true);
    }
    bool resetSandboxedPlugin(size_t sandboxIndex) {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return false;
        const uint32_t index = m_sandboxedPluginIndices[sandboxIndex];
        auto processor = m_effectChain.getProcessor(index);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        if (!sandbox) return false;
        sandbox->reset();
        return sandbox->resetPluginControlPlane();
    }
    bool retrySandboxedPlugin(size_t sandboxIndex) {
        if (sandboxIndex >= m_sandboxedPluginIndices.size()) return false;
        const uint32_t index = m_sandboxedPluginIndices[sandboxIndex];
        auto processor = m_effectChain.getProcessor(index);
        auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        const bool restarted = sandbox && sandbox->restart(true);
        if (restarted && sandboxIndex < m_sandboxedPluginPaths.size()) {
            Aura::Core::Plugins::PluginCacheManager::getInstance().recordScanSuccess(
                m_sandboxedPluginPaths[sandboxIndex]);
        }
        return restarted;
    }
    uint32_t maintainSandboxedPlugins(bool autoRestart) {
        uint32_t recovered = 0;
        for (size_t sandboxIndex = 0; sandboxIndex < m_sandboxedPluginIndices.size(); ++sandboxIndex) {
            const uint32_t index = m_sandboxedPluginIndices[sandboxIndex];
            auto processor = m_effectChain.getProcessor(index);
            auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
            if (!sandbox) continue;
            if (sandbox->pollHealth()) continue;
            if (autoRestart && sandbox->canAutoRetry() && sandbox->autoRestart()) {
                if (sandboxIndex < m_sandboxedPluginPaths.size()) {
                    Aura::Core::Plugins::PluginCacheManager::getInstance().recordScanSuccess(
                        m_sandboxedPluginPaths[sandboxIndex]);
                }
                ++recovered;
            }
        }
        return recovered;
    }
    struct SandboxStatus {
        uint32_t pluginIndex = 0;
        bool alive = false;
        bool canRetry = false;
        uint8_t failure = 0;
        uint32_t droppedOutputMidi = 0;
        uint32_t mailboxOverruns = 0;
        uint32_t inputMidiTruncations = 0;
        uint32_t recoveryMode = 0;
    };
    uint8_t lastSandboxFailure() const noexcept {
        return m_lastSandboxFailure.load(std::memory_order_acquire);
    }
    const char* lastSandboxFailureText() const noexcept {
        for (const uint32_t index : m_sandboxedPluginIndices) {
            auto processor = m_effectChain.getProcessor(index);
            auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
            if (sandbox && sandbox->failureCode() != 0) return sandbox->failureText();
        }
        return m_lastSandboxFailureText.c_str();
    }
    std::vector<SandboxStatus> getSandboxStatuses() {
        std::vector<SandboxStatus> result;
        result.reserve(m_sandboxedPluginIndices.size());
        for (const uint32_t index : m_sandboxedPluginIndices) {
            auto processor = m_effectChain.getProcessor(index);
            auto* sandbox = dynamic_cast<Aura::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
            if (!sandbox) continue;
            result.push_back({index, sandbox->isAlive(), sandbox->canRetry(),
                              sandbox->failureCode(), sandbox->takeDroppedOutputMidi(),
                              sandbox->takeMailboxOverruns(),
                              sandbox->takeInputMidiTruncations(),
                              static_cast<uint32_t>(sandbox->recoveryMode())});
        }
        return result;
    }
    virtual bool setPluginParameter(uint32_t pluginIndex, uint32_t parameterId, float value) {
        const bool changed = m_effectChain.setParameter(pluginIndex, parameterId, value);
        if (changed) invalidateFrozenForEdit();
        return changed;
    }
    virtual float getPluginParameter(uint32_t pluginIndex, uint32_t parameterId) const noexcept {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        return processor ? processor->getParameter(parameterId) : 0.0f;
    }
    virtual uint32_t getPluginParameterCount(uint32_t pluginIndex) const noexcept {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        return processor ? processor->getNumParameters() : 0;
    }
    bool hasPluginNativeEditor(uint32_t pluginIndex) const noexcept {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        return processor && processor->hasNativeEditor();
    }
    uint64_t openPluginNativeEditor(uint32_t pluginIndex, uintptr_t parent) noexcept {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        return processor ? processor->openNativeEditor(parent) : 0;
    }
    bool closePluginNativeEditor(uint32_t pluginIndex, uint64_t session) noexcept {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        return processor && processor->closeNativeEditor(session);
    }
    std::string getPluginParameterName(uint32_t pluginIndex, uint32_t parameterId) const {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        if (!processor || parameterId >= processor->getNumParameters()) return {};
        char name[128]{};
        processor->getParameterName(parameterId, name, sizeof(name));
        return name;
    }
    bool setPluginBypass(uint32_t pluginIndex, bool bypassed) {
        if (pluginIndex >= m_pluginTypes.size()) return false;
        m_effectChain.setBypass(pluginIndex, bypassed);
        invalidateFrozenForEdit();
        return true;
    }
    bool getPluginBypass(uint32_t pluginIndex) const {
        if (pluginIndex >= m_pluginTypes.size()) return false;
        return m_effectChain.getBypass(pluginIndex);
    }
