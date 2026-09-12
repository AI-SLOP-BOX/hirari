    bool setStateUnlocked(const uint8_t* data, size_t size) noexcept {
#if defined(_WIN32)
        (void)data; (void)size;
        return false;
#else
        if (!isAlive() || m_shared == nullptr) {
            m_lastStateError.store(SandboxProtocol::kStateErrorUnavailable, std::memory_order_release);
            return false;
        }
        if (size > SandboxProtocol::kMaxStateBytes || (size != 0 && data == nullptr)) {
            m_lastStateError.store(SandboxProtocol::kStateErrorOversize, std::memory_order_release);
            return false;
        }
        if (!waitForAudioCallsToFinish()) {
            m_lastStateError.store(SandboxProtocol::kStateErrorTimeout, std::memory_order_release);
            return false;
        }
        const uint64_t completed = m_shared->stateCompletedSequence.load(std::memory_order_acquire);
        if (m_shared->stateRequestSequence.load(std::memory_order_acquire) != completed) {
            m_lastStateError.store(SandboxProtocol::kStateErrorBusy, std::memory_order_release);
            return false;
        }
        const GenerationContext context = m_generationContext;
        if (!context.valid()) {
            m_lastStateError.store(SandboxProtocol::kStateErrorVersion, std::memory_order_release);
            return false;
        }
        if (size) std::memcpy(m_shared->state, data, size);
        m_shared->stateSize.store(static_cast<uint32_t>(size), std::memory_order_relaxed);
        m_shared->stateVersion.store(SandboxProtocol::kStateProtocolVersion, std::memory_order_relaxed);
        m_shared->stateChecksum.store(SandboxProtocol::stateChecksum(data, size), std::memory_order_relaxed);
        m_shared->stateProjectGeneration.store(context.project, std::memory_order_relaxed);
        m_shared->statePluginGeneration.store(context.plugin, std::memory_order_relaxed);
        m_shared->stateAudioGeneration.store(context.audio, std::memory_order_relaxed);
        m_shared->stateGeneration.store(context.state, std::memory_order_relaxed);
        m_shared->stateMode.store(2, std::memory_order_relaxed);
        m_shared->stateResult.store(0, std::memory_order_relaxed);
        m_shared->stateError.store(SandboxProtocol::kStateErrorNone, std::memory_order_relaxed);
        const uint64_t sequence = ++m_stateSequence;
        m_shared->stateRequestSequence.store(sequence, std::memory_order_release);
        const bool ok = waitForState(sequence, false);
        const bool generationOk = ok &&
            m_shared->stateProjectGeneration.load(std::memory_order_acquire) == context.project &&
            m_shared->statePluginGeneration.load(std::memory_order_acquire) == context.plugin &&
            m_shared->stateAudioGeneration.load(std::memory_order_acquire) == context.audio &&
            m_shared->stateGeneration.load(std::memory_order_acquire) == context.state;
        const bool checksumOk = generationOk && m_shared->stateError.load(std::memory_order_acquire) ==
            SandboxProtocol::kStateErrorNone && m_shared->stateChecksum.load(std::memory_order_acquire) ==
            SandboxProtocol::stateChecksum(data, size);
        if (checksumOk) m_stateCache.assign(data, data + size);
        if (!checksumOk) {
            const uint8_t workerError = m_shared->stateError.load(std::memory_order_acquire);
            m_lastStateError.store(
                workerError == SandboxProtocol::kStateErrorNone
                    ? SandboxProtocol::kStateErrorTimeout : workerError,
                std::memory_order_release);
        }
        return checksumOk;
#endif
    }

    std::vector<uint8_t> getStateUnlocked() {
#if defined(_WIN32)
        return {};
#else
        if (!isAlive() || m_shared == nullptr) {
            m_lastStateError.store(SandboxProtocol::kStateErrorUnavailable, std::memory_order_release);
            return m_stateCache;
        }
        if (!const_cast<PluginSandboxHost*>(this)->waitForAudioCallsToFinish()) {
            m_lastStateError.store(SandboxProtocol::kStateErrorTimeout, std::memory_order_release);
            return m_stateCache;
        }
        const uint64_t completed = m_shared->stateCompletedSequence.load(std::memory_order_acquire);
        if (m_shared->stateRequestSequence.load(std::memory_order_acquire) != completed) {
            m_lastStateError.store(SandboxProtocol::kStateErrorBusy, std::memory_order_release);
            return m_stateCache;
        }
        m_shared->stateSize.store(0, std::memory_order_relaxed);
        m_shared->stateVersion.store(0, std::memory_order_relaxed);
        m_shared->stateMode.store(1, std::memory_order_relaxed);
        m_shared->stateResult.store(0, std::memory_order_relaxed);
        m_shared->stateError.store(SandboxProtocol::kStateErrorNone, std::memory_order_relaxed);
        const uint64_t sequence = ++m_stateSequence;
        m_shared->stateRequestSequence.store(sequence, std::memory_order_release);
        if (!const_cast<PluginSandboxHost*>(this)->waitForState(sequence, true)) {
            m_lastStateError.store(SandboxProtocol::kStateErrorTimeout, std::memory_order_release);
            return m_stateCache;
        }
        const uint32_t size = m_shared->stateSize.load(std::memory_order_acquire);
        if (size > SandboxProtocol::kMaxStateBytes) {
            m_lastStateError.store(SandboxProtocol::kStateErrorOversize, std::memory_order_release);
            return m_stateCache;
        }
        auto state = std::vector<uint8_t>(m_shared->state, m_shared->state + size);
        const uint8_t workerError = m_shared->stateError.load(std::memory_order_acquire);
        const bool versionOk = SandboxProtocol::isSupportedStateVersion(
            m_shared->stateVersion.load(std::memory_order_acquire));
        const bool checksumOk = m_shared->stateChecksum.load(std::memory_order_acquire) ==
            SandboxProtocol::stateChecksum(state.data(), state.size());
        if (!versionOk || workerError != SandboxProtocol::kStateErrorNone || !checksumOk) {
            const uint8_t error = workerError != SandboxProtocol::kStateErrorNone
                ? workerError
                : (!versionOk ? SandboxProtocol::kStateErrorVersion : SandboxProtocol::kStateErrorChecksum);
            m_lastStateError.store(error, std::memory_order_release);
            return m_stateCache;
        }
        const_cast<PluginSandboxHost*>(this)->m_stateCache = state;
        return state;
#endif
    }

#if !defined(_WIN32)
    bool waitForAudioCallsToFinish() noexcept {
        for (int attempt = 0; attempt < 1000; ++attempt) {
            if (m_processCalls.load(std::memory_order_acquire) == 0) return true;
            ::usleep(100);
        }
        return false;
    }

    bool waitForState(uint64_t sequence, bool allowEmpty) noexcept {
        // State serialization is a control-plane operation. Commercial
        // instruments may rebuild wavetable/preset state while loading, so a
        // one-second budget incorrectly turns a healthy worker into a hung
        // plugin. This does not affect the audio callback deadline; it only
        // bounds synchronous save/restore requests on the control thread.
        constexpr int kStateWaitAttempts = 5000;
        for (int attempt = 0; attempt < kStateWaitAttempts; ++attempt) {
            if (!isAlive()) return false;
            if (SandboxProtocol::sequenceReached(
                    m_shared->stateCompletedSequence.load(std::memory_order_acquire), sequence)) {
                const bool result = m_shared->stateResult.load(std::memory_order_acquire) != 0;
                return result && (allowEmpty || m_shared->stateSize.load(std::memory_order_acquire) <= SandboxProtocol::kMaxStateBytes);
            }
            ::usleep(1000);
        }
        // The child may still be blocked inside a vendor state callback.  It
        // must not remain attached to the audio graph, but unmapping shared
        // memory from this wait loop is unsafe because the control operation
        // is still unwinding.  Kill the child and leave reaping/unmapping to
        // the serialized lifecycle cleanup path (stop()/destructor).
        m_failure.store(Failure::ProcessHung, std::memory_order_release);
        m_processAlive.store(false, std::memory_order_release);
        const pid_t child = m_pid.load(std::memory_order_acquire);
        if (child > 0) (void)::kill(child, SIGKILL);
        return false;
    }

    void unmapShared() noexcept {
        if (m_shared != nullptr) {
            ::munmap(m_shared, sizeof(SandboxProtocol::SharedAudioBlock));
            m_shared = nullptr;
        }
    }

    void closeIpcFds() noexcept {
        if (m_controlFd >= 0) { ::close(m_controlFd); m_controlFd = -1; }
        if (m_statusFd >= 0) { ::close(m_statusFd); m_statusFd = -1; }
    }

    bool waitForReady(int timeoutMs) noexcept {
        if (m_statusFd < 0) return false;
        struct pollfd descriptor{m_statusFd, POLLIN | POLLHUP | POLLERR, 0};
        const int result = ::poll(&descriptor, 1, timeoutMs);
        if (result <= 0) return false;
        if (descriptor.revents & (POLLHUP | POLLERR)) {
            int status = 0;
            const pid_t child = m_pid.load(std::memory_order_acquire);
            if (child > 0 && ::waitpid(child, &status, WNOHANG) == child) {
                m_failure.store(Failure::ProcessExited, std::memory_order_release);
            }
            return false;
        }
        if (!(descriptor.revents & POLLIN)) return false;
        uint8_t response = 0;
        if (::read(m_statusFd, &response, sizeof(response)) != 1) return false;
        if (response == SandboxProtocol::kErrorLoad) m_failure.store(Failure::PluginLoadFailed, std::memory_order_release);
        else if (response == SandboxProtocol::kErrorAbi) m_failure.store(Failure::PluginAbiInvalid, std::memory_order_release);
        else if (response == SandboxProtocol::kErrorInstance) m_failure.store(Failure::PluginInstanceFailed, std::memory_order_release);
        else if (response == SandboxProtocol::kErrorSecurity) m_failure.store(Failure::SecuritySetupFailed, std::memory_order_release);
        else if (response == SandboxProtocol::kErrorUnsupported) m_failure.store(Failure::PluginFormatUnsupported, std::memory_order_release);
        else if (response == SandboxProtocol::kError) m_failure.store(Failure::PluginLoadFailed, std::memory_order_release);
        return response == SandboxProtocol::kReady;
    }
#endif
