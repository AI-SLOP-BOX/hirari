    bool setState(const uint8_t* data, size_t size) noexcept {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        std::lock_guard<std::mutex> stateLock(m_stateMutex);
        StateTransitionGuard transition(*this);
        m_lastStateError.store(SandboxProtocol::kStateErrorNone, std::memory_order_release);
        return setStateUnlocked(data, size);
    }

    std::vector<uint8_t> getState() const {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        std::lock_guard<std::mutex> stateLock(m_stateMutex);
        StateTransitionGuard transition(*const_cast<PluginSandboxHost*>(this));
        m_lastStateError.store(SandboxProtocol::kStateErrorNone, std::memory_order_release);
        return const_cast<PluginSandboxHost*>(this)->getStateUnlocked();
    }

    uint8_t stateErrorCode() const noexcept { return m_lastStateError.load(std::memory_order_acquire); }
    const char* stateErrorText() const noexcept {
        switch (stateErrorCode()) {
        case SandboxProtocol::kStateErrorNone: return "none";
        case SandboxProtocol::kStateErrorOversize: return "state-oversize";
        case SandboxProtocol::kStateErrorVersion: return "state-version-unsupported";
        case SandboxProtocol::kStateErrorChecksum: return "state-checksum-mismatch";
        case SandboxProtocol::kStateErrorPlugin: return "state-plugin-rejected";
        case SandboxProtocol::kStateErrorUnavailable: return "state-unavailable";
        case SandboxProtocol::kStateErrorBusy: return "state-busy";
        case SandboxProtocol::kStateErrorTimeout: return "state-timeout";
        default: return "state-unknown-error";
        }
    }

private:
    struct StateTransitionGuard {
        explicit StateTransitionGuard(PluginSandboxHost& host) noexcept : m_host(host) {
            m_host.m_stateTransition.store(true, std::memory_order_release);
        }
        ~StateTransitionGuard() {
            m_host.m_stateTransition.store(false, std::memory_order_release);
        }
        StateTransitionGuard(const StateTransitionGuard&) = delete;
        StateTransitionGuard& operator=(const StateTransitionGuard&) = delete;
        PluginSandboxHost& m_host;
    };

