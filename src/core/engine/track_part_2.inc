    size_t sandboxSlotCount() const noexcept { return hirari_sandbox_slots_len(m_sandboxSlots); }
    uint32_t sandboxSlotIndex(size_t slot) const noexcept {
        return hirari_sandbox_slots_index(m_sandboxSlots, slot);
    }
    std::string sandboxSlotPath(size_t slot) const {
        const size_t length = hirari_sandbox_slots_path_size(m_sandboxSlots, slot);
        std::string path(length, '\0');
        if (length == 0) return path;
        if (hirari_sandbox_slots_path_copy(m_sandboxSlots, slot,
                reinterpret_cast<uint8_t*>(path.data()), length) != length) return {};
        return path;
    }

    // A control edit follows one protocol: drop the frozen render, mutate, then
    // bump the layout revision. A helper keeps the three steps from being
    // re-ordered site by site.
    template <typename Fn>
    void editControl(const Fn& fn) {
        invalidateFrozenForEdit();
        fn();
        markProjectLayoutChanged();
    }

    // A successful edit that only needs the layout revision bumped; the frozen
    // render stays valid because nothing audible changed.
    bool commitLayoutChange(bool changed) {
        if (changed) markProjectLayoutChanged();
        return changed;
    }

    // Same protocol for edits that only count when the chain actually changed.
    bool commitEditChange(bool changed) {
        if (changed) {
            invalidateFrozenForEdit();
            markProjectLayoutChanged();
        }
        return changed;
    }

    // Reads through the chain processor at an index; an empty slot answers with
    // the caller's fallback instead of repeating the shared_ptr check.
    template <typename Fn>
    std::invoke_result_t<Fn, ::Hirari::DSP::IProcessor&> withProcessor(
        uint32_t pluginIndex, std::invoke_result_t<Fn, ::Hirari::DSP::IProcessor&> missing,
        const Fn& fn) const {
        const auto processor = m_effectChain.getProcessor(pluginIndex);
        return processor ? fn(*processor) : missing;
    }

    // Reads through the sandbox bound to a slot; a missing slot or a
    // non-sandbox processor answers with the caller's fallback.
    template <typename Fn>
    std::invoke_result_t<Fn, Hirari::Core::Plugins::ProcessSandboxProcessor&> withSandbox(
        size_t sandboxIndex,
        std::invoke_result_t<Fn, Hirari::Core::Plugins::ProcessSandboxProcessor&> missing,
        const Fn& fn) const {
        if (sandboxIndex >= sandboxSlotCount()) return missing;
        const auto processor = m_effectChain.getProcessor(sandboxSlotIndex(sandboxIndex));
        auto* sandbox =
            dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        return sandbox ? fn(*sandbox) : missing;
    }

    void setVolume(float v) {
        editControl([&] {
            const float next = std::isfinite(v) ? std::clamp(v, 0.0f, 2.0f) : 1.0f;
            m_volume.store(next, std::memory_order_release);
            hirari_channel_strip_set_gain(m_strip, v);
        });
    }
    void setPan(float p) {
        editControl([&] {
            const float next = std::isfinite(p) ? std::clamp(p, -1.0f, 1.0f) : 0.0f;
            m_pan.store(next, std::memory_order_release);
            hirari_channel_strip_set_pan(m_strip, next);
        });
    }
    virtual bool addPlugin(uint32_t pluginType) {
        // The built-in catalog (type <-> path) is Rust-owned. Unknown types
        // must fail explicitly; retaining metadata for an instance that
        // cannot process audio is misleading.
        const char* pluginPath = hirari_internal_plugin_path(pluginType, false);
        if (!pluginPath) return false;
        auto processor = std::make_shared<Hirari::Core::Plugin::ExternalPluginHost>(
            pluginPath,
            Hirari::Core::Plugin::ExternalPluginHost::Format::Internal);
        if (!processor->isOperational()) return false;

        m_effectChain.addProcessor(std::move(processor));
        m_effectChain.setNodeKind(m_effectChain.nodeCount() - 1, pluginType);
        invalidateFrozenForEdit();
        if (m_isPrepared) {
            m_effectChain.setSampleRate(m_sampleRate, m_blockSize);
        }
        markProjectLayoutChanged();
        return true;
    }

    struct PluginSnapshot {
        uint32_t type = 0;
        std::shared_ptr<DSP::IProcessor> processor;
        bool bypassed = false;
        bool parallel = false;
        bool midiInstrument = false;
        std::string sandboxPath;
    };

    bool removePlugin(uint32_t pluginIndex, PluginSnapshot* snapshot = nullptr) {
        if (pluginIndex >= m_effectChain.nodeCount()) return false;
        EffectChain::Entry removed{};
        if (!m_effectChain.removeProcessor(pluginIndex, &removed)) return false;
        const bool removedMidiInstrument = removed.processor &&
            removed.processor->isMidiInstrument();
        if (snapshot) {
            snapshot->type = removed.kind;
            snapshot->processor = removed.processor;
            snapshot->bypassed = removed.bypassed;
            snapshot->parallel = removed.parallel;
            snapshot->midiInstrument = removedMidiInstrument;
            if (snapshot->type == 0xFFFFFFFFu) {
                const size_t slot = hirari_sandbox_slots_find(m_sandboxSlots, pluginIndex);
                if (slot != SIZE_MAX) snapshot->sandboxPath = sandboxSlotPath(slot);
            }
        }
        if (removedMidiInstrument)
            m_midiInstrumentPluginCount.fetch_sub(1, std::memory_order_release);
        invalidateFrozenForEdit();
        // The slot shift and its path permutation are Rust-owned so removal and
        // reordering cannot disagree about the chain order.
        (void)hirari_sandbox_slots_remove(m_sandboxSlots, pluginIndex);
        markProjectLayoutChanged();
        return true;
    }

    bool restorePlugin(uint32_t pluginIndex, const PluginSnapshot& snapshot) {
        if (!snapshot.processor || pluginIndex > m_effectChain.nodeCount()) return false;
        EffectChain::Entry entry{snapshot.processor, snapshot.bypassed, snapshot.parallel,
                                 snapshot.type};
        if (!m_effectChain.insertProcessor(pluginIndex, std::move(entry))) return false;
        if (snapshot.midiInstrument)
            m_midiInstrumentPluginCount.fetch_add(1, std::memory_order_release);
        if (snapshot.type == 0xFFFFFFFFu) {
            (void)hirari_sandbox_slots_insert(
                m_sandboxSlots, pluginIndex,
                reinterpret_cast<const uint8_t*>(snapshot.sandboxPath.data()),
                snapshot.sandboxPath.size());
        }
        invalidateFrozenForEdit();
        markProjectLayoutChanged();
        return true;
    }

    bool movePlugin(uint32_t from, uint32_t to) {
        if (from >= m_effectChain.nodeCount() || to >= m_effectChain.nodeCount() ||
            from == to) return false;
        if (!m_effectChain.moveProcessor(from, to)) return false;

        // Same Rust-owned permutation as removal, now with the from/to remap.
        hirari_sandbox_slots_move(m_sandboxSlots, from, to);
        invalidateFrozenForEdit();
        markProjectLayoutChanged();
        return true;
    }

    bool addSandboxedPlugin(const std::string& pluginPath) {
        switch (hirari_plugin_admission_error(pluginPath.c_str())) {
            case 1: m_lastSandboxFailureText = "invalid-plugin-path"; return false;
            case 2: m_lastSandboxFailureText = "plugin-path-missing"; return false;
            case 3: m_lastSandboxFailureText = "plugin-format-unsupported"; return false;
            default: break;
        }
        if (hirari_sandbox_slots_contains_path(
                m_sandboxSlots, reinterpret_cast<const uint8_t*>(pluginPath.data()),
                pluginPath.size())) {
            m_lastSandboxFailureText = "duplicate-plugin-path";
            return false;
        }
        auto processor = std::make_shared<Hirari::Core::Plugins::ProcessSandboxProcessor>(
            pluginPath, m_sampleRate, std::max<uint32_t>(m_blockSize, 1u));
        bool started = false;
        // The transport-retry policy (which failures are transient and the
        // backoff) lives in Rust; close the failed host before a retry.
        constexpr uint32_t kMaxTransportRetries = 5;
        for (uint32_t attempt = 0; attempt < kMaxTransportRetries && !started; ++attempt) {
            started = processor->start();
            if (started || attempt + 1 == kMaxTransportRetries) break;
            const uint32_t delay =
                hirari_plugin_sandbox_transport_retry_delay(attempt, processor->failureCode());
            if (delay == 0) break;
            processor->stop();
            std::this_thread::sleep_for(std::chrono::milliseconds(delay));
        }
        if (!started) {
            m_lastSandboxFailure.store(processor->failureCode(), std::memory_order_release);
            m_lastSandboxFailureText = processor->failureCode() == 0
                ? "sandbox-start-failed"
                : processor->failureText();
            (void)hirari_plugin_scan_cache_record_failure(
                reinterpret_cast<const uint8_t*>(pluginPath.data()), pluginPath.size(),
                processor->failureCode());
            return false;
        }
        (void)hirari_plugin_scan_cache_record_success(
            reinterpret_cast<const uint8_t*>(pluginPath.data()), pluginPath.size());
        m_lastSandboxFailure.store(0, std::memory_order_release);
        m_lastSandboxFailureText = "none";
        const bool midiInstrument = processor->isMidiInstrument();
        const uint32_t processorIndex = m_effectChain.nodeCount();
        if (m_isPrepared) processor->prepareToPlay(m_sampleRate, m_blockSize);
        m_effectChain.addProcessor(processor);
        m_effectChain.setNodeKind(processorIndex, 0xFFFFFFFFu);
        if (midiInstrument)
            m_midiInstrumentPluginCount.fetch_add(1, std::memory_order_release);
        (void)hirari_sandbox_slots_push(
            m_sandboxSlots, processorIndex,
            reinterpret_cast<const uint8_t*>(pluginPath.data()), pluginPath.size());
        if (m_isPrepared) m_effectChain.setSampleRate(m_sampleRate, m_blockSize);
        invalidateFrozenForEdit();
        markProjectLayoutChanged();
        return true;
    }

    // Control/test boundary for validating an isolated processor with a
    // caller-owned block. The normal transport still uses Track::process;
    // this method exists so integration tests can prove that shared-memory
    // plugin output is not merely a readiness signal.
    bool processSandboxedPluginBlock(size_t sandboxIndex, float* left, float* right,
                                     uint32_t frames) {
        if (!left || !right || frames == 0 || frames > Hirari::Core::Plugins::SandboxProtocol::kMaxFrames ||
            sandboxIndex >= sandboxSlotCount()) return false;
        const uint32_t pluginIndex = sandboxSlotIndex(sandboxIndex);
        auto processor = m_effectChain.getProcessor(pluginIndex);
        auto* sandbox = dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
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
            frames > Hirari::Core::Plugins::SandboxProtocol::kMaxFrames ||
            sandboxIndex >= sandboxSlotCount()) return {};
        const uint32_t pluginIndex = sandboxSlotIndex(sandboxIndex);
        auto processor = m_effectChain.getProcessor(pluginIndex);
        auto* sandbox = dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        if (!sandbox) return {};
        if (midiSize > sizeof(Hirari::Core::Plugins::SandboxProtocol::MidiEvent::data)) {
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
    size_t getPluginCount() const { return m_effectChain.nodeCount(); }
    std::vector<uint32_t> getPluginTypes() const {
        std::vector<uint32_t> types(m_effectChain.nodeCount());
        for (uint32_t index = 0; index < types.size(); ++index)
            types[index] = m_effectChain.nodeKind(index);
        return types;
    }
    std::vector<std::vector<uint8_t>> getPluginStates() const {
        const uint32_t count = m_effectChain.nodeCount();
        std::vector<std::vector<uint8_t>> states;
        states.reserve(count);
        for (uint32_t index = 0; index < count; ++index)
            states.push_back(m_effectChain.getProcessorState(index));
        return states;
    }
    bool setPluginState(size_t pluginIndex, const std::vector<uint8_t>& state) {
        return commitEditChange(
            m_effectChain.setProcessorState(static_cast<uint32_t>(pluginIndex), state));
    }
    std::vector<std::vector<uint8_t>> getPluginGuiStates() const {
        const uint32_t count = m_effectChain.nodeCount();
        std::vector<std::vector<uint8_t>> states;
        states.reserve(count);
        for (uint32_t index = 0; index < count; ++index)
            states.push_back(m_effectChain.getProcessorGuiState(index));
        return states;
    }
    bool setPluginGuiState(size_t pluginIndex, const std::vector<uint8_t>& state) {
        if (pluginIndex >= m_effectChain.nodeCount() || state.size() > 1024u * 1024u) return false;
        return commitLayoutChange(m_effectChain.setProcessorGuiState(
            static_cast<uint32_t>(pluginIndex), state));
    }
    std::vector<std::string> getSandboxedPluginPaths() const {
        std::vector<std::string> paths;
        paths.reserve(sandboxSlotCount());
        for (size_t slot = 0; slot < sandboxSlotCount(); ++slot) paths.push_back(sandboxSlotPath(slot));
        return paths;
    }
    std::vector<std::vector<uint8_t>> getSandboxedPluginStates() const {
        std::vector<std::vector<uint8_t>> states;
        states.reserve(sandboxSlotCount());
        for (size_t slot = 0; slot < sandboxSlotCount(); ++slot)
            states.push_back(m_effectChain.getProcessorState(sandboxSlotIndex(slot)));
        return states;
    }
    std::vector<uint8_t> getSandboxedPluginState(size_t sandboxIndex) const {
        if (sandboxIndex >= sandboxSlotCount()) return {};
        return m_effectChain.getProcessorState(sandboxSlotIndex(sandboxIndex));
    }
    bool setSandboxedPluginState(size_t sandboxIndex, const std::vector<uint8_t>& state) {
        if (sandboxIndex >= sandboxSlotCount()) return false;
        return commitEditChange(m_effectChain.setProcessorState(
            sandboxSlotIndex(sandboxIndex), state));
    }
    uint8_t sandboxedPluginStateError(size_t sandboxIndex) const noexcept {
        return withSandbox(sandboxIndex,
                           Hirari::Core::Plugins::SandboxProtocol::kStateErrorUnavailable,
                           [](auto& sandbox) { return sandbox.stateErrorCode(); });
    }
    const char* sandboxedPluginStateErrorText(size_t sandboxIndex) const noexcept {
        return withSandbox(sandboxIndex, "state-unavailable",
                           [](auto& sandbox) { return sandbox.stateErrorText(); });
    }
    bool restartSandboxedPlugin(size_t sandboxIndex) {
        // This entry point is an explicit user/control action, not the
        // background maintenance retry.  It must be allowed to recover a
        // quarantined plugin and reset the bounded automatic-restart budget.
        return commitLayoutChange(withSandbox(
            sandboxIndex, false, [](auto& sandbox) { return sandbox.restart(true); }));
    }
    bool retrySandboxedPlugin(size_t sandboxIndex) {
        if (sandboxIndex >= sandboxSlotCount()) return false;
        const uint32_t index = sandboxSlotIndex(sandboxIndex);
        auto processor = m_effectChain.getProcessor(index);
        auto* sandbox = dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
        const bool restarted = sandbox && sandbox->restart(true);
        if (restarted) markProjectLayoutChanged();
        if (restarted && sandboxIndex < sandboxSlotCount()) {
            const std::string path = sandboxSlotPath(sandboxIndex);
            (void)hirari_plugin_scan_cache_record_success(
                reinterpret_cast<const uint8_t*>(path.data()), path.size());
        }
        return restarted;
    }
    uint32_t maintainSandboxedPlugins(bool autoRestart) {
        uint32_t recovered = 0;
        for (size_t sandboxIndex = 0; sandboxIndex < sandboxSlotCount(); ++sandboxIndex) {
            const uint32_t index = sandboxSlotIndex(sandboxIndex);
            auto processor = m_effectChain.getProcessor(index);
            auto* sandbox = dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
            if (!sandbox) continue;
            if (sandbox->pollHealth()) continue;
            if (autoRestart && sandbox->canAutoRetry() && sandbox->autoRestart()) {
                if (sandboxIndex < sandboxSlotCount()) {
                    const std::string path = sandboxSlotPath(sandboxIndex);
                    (void)hirari_plugin_scan_cache_record_success(
                        reinterpret_cast<const uint8_t*>(path.data()), path.size());
                }
                ++recovered;
            }
        }
        if (recovered > 0) markProjectLayoutChanged();
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
        for (size_t slot = 0; slot < sandboxSlotCount(); ++slot) {
            auto processor = m_effectChain.getProcessor(sandboxSlotIndex(slot));
            auto* sandbox = dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
            if (sandbox && sandbox->failureCode() != 0) return sandbox->failureText();
        }
        return m_lastSandboxFailureText.c_str();
    }
    std::vector<SandboxStatus> getSandboxStatuses() {
        std::vector<SandboxStatus> result;
        result.reserve(sandboxSlotCount());
        for (size_t slot = 0; slot < sandboxSlotCount(); ++slot) {
            const uint32_t index = sandboxSlotIndex(slot);
            auto processor = m_effectChain.getProcessor(index);
            auto* sandbox = dynamic_cast<Hirari::Core::Plugins::ProcessSandboxProcessor*>(processor.get());
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
        return commitEditChange(m_effectChain.setParameter(pluginIndex, parameterId, value));
    }
    virtual float getPluginParameter(uint32_t pluginIndex, uint32_t parameterId) const noexcept {
        return withProcessor(pluginIndex, 0.0f,
                             [&](auto& p) { return p.getParameter(parameterId); });
    }
    virtual uint32_t getPluginParameterCount(uint32_t pluginIndex) const noexcept {
        return withProcessor(pluginIndex, 0u, [](auto& p) { return p.getNumParameters(); });
    }
    bool hasPluginNativeEditor(uint32_t pluginIndex) const noexcept {
        return withProcessor(pluginIndex, false, [](auto& p) { return p.hasNativeEditor(); });
    }
    uint64_t openPluginNativeEditor(uint32_t pluginIndex, uintptr_t parent) noexcept {
        return withProcessor(pluginIndex, uint64_t{0},
                             [&](auto& p) { return p.openNativeEditor(parent); });
    }
    bool closePluginNativeEditor(uint32_t pluginIndex, uint64_t session) noexcept {
        return withProcessor(pluginIndex, false,
                             [&](auto& p) { return p.closeNativeEditor(session); });
    }
    std::string getPluginParameterName(uint32_t pluginIndex, uint32_t parameterId) const {
        return withProcessor(pluginIndex, std::string{}, [&](auto& processor) -> std::string {
            if (parameterId >= processor.getNumParameters()) return {};
            char name[128]{};
            processor.getParameterName(parameterId, name, sizeof(name));
            return name;
        });
    }
    bool setPluginBypass(uint32_t pluginIndex, bool bypassed) {
        if (pluginIndex >= m_effectChain.nodeCount()) return false;
        editControl([&] { m_effectChain.setBypass(pluginIndex, bypassed); });
        return true;
    }
    bool getPluginBypass(uint32_t pluginIndex) const {
        if (pluginIndex >= m_effectChain.nodeCount()) return false;
        return m_effectChain.getBypass(pluginIndex);
    }
