    void stop() noexcept {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
#if defined(_WIN32)
        m_processAlive.store(false, std::memory_order_release);
#else
        // Publish the stop before touching any shared mapping. The audio
        // callback observes this flag and leaves without dereferencing the
        // mapping; the control thread then waits for already-running calls.
        m_processAlive.store(false, std::memory_order_release);
        for (int attempt = 0; attempt < 1000 &&
             m_processCalls.load(std::memory_order_acquire) != 0; ++attempt) {
            ::usleep(100);
        }
        if (m_controlFd >= 0) {
            const uint8_t command = SandboxProtocol::kShutdown;
            (void)::write(m_controlFd, &command, sizeof(command));
            ::close(m_controlFd);
            m_controlFd = -1;
        }
        const pid_t child = m_pid.load(std::memory_order_acquire);
        if (child <= 0) {
            m_processAlive.store(false, std::memory_order_release);
            if (m_sharedFd >= 0) { ::close(m_sharedFd); m_sharedFd = -1; }
            if (!m_sharedName.empty()) { ::shm_unlink(m_sharedName.c_str()); m_sharedName.clear(); }
            unmapShared();
            return;
        }
        if (::kill(child, SIGTERM) == -1 && errno != ESRCH) {
            // waitpid below still reaps an already-exited child when possible.
        }
        int status = 0;
        bool reaped = false;
        for (int attempt = 0; attempt < 100; ++attempt) {
            const pid_t result = ::waitpid(child, &status, WNOHANG);
            if (result == child) { reaped = true; break; }
            if (result < 0 && errno != EINTR && errno != ECHILD) break;
            ::usleep(5000);
        }
        if (!reaped) {
            (void)::kill(child, SIGKILL);
            // Never turn a failed plugin shutdown into an unbounded control
            // thread hang.  A child that ignores SIGKILL is already outside
            // the normal lifecycle; retain the failure state and finish
            // releasing this host's IPC resources after a bounded reap wait.
            for (int attempt = 0; attempt < 100; ++attempt) {
                const pid_t result = ::waitpid(child, &status, WNOHANG);
                if (result == child) { reaped = true; break; }
                if (result < 0 && errno != EINTR && errno != ECHILD) break;
                ::usleep(5000);
            }
        }
        if (!reaped && m_failure.load(std::memory_order_acquire) == Failure::None)
            m_failure.store(Failure::ProcessExited, std::memory_order_release);
        m_pid.store(-1, std::memory_order_release);
        m_processAlive.store(false, std::memory_order_release);
        if (m_statusFd >= 0) {
            ::close(m_statusFd);
            m_statusFd = -1;
        }
        if (m_sharedFd >= 0) {
            ::close(m_sharedFd);
            m_sharedFd = -1;
        }
        if (!m_sharedName.empty()) {
            ::shm_unlink(m_sharedName.c_str());
            m_sharedName.clear();
        }
        unmapShared();
#endif
    }

    // Control-plane reset for the isolated worker. This is intentionally
    // separate from DSP::IProcessor::reset(), which may be called on the
    // realtime thread and therefore cannot write to the lifecycle pipe.
    bool requestPluginReset() noexcept {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
#if defined(_WIN32)
        return false;
#else
        if (!m_processAlive.load(std::memory_order_acquire) || m_controlFd < 0) return false;
        const uint8_t command = SandboxProtocol::kReset;
        return ::write(m_controlFd, &command, sizeof(command)) == 1;
#endif
    }

    // Audio-thread safe status read. It performs no syscall and no lock.
    bool isAlive() const noexcept {
        return m_processAlive.load(std::memory_order_acquire);
    }

    // Control-thread only health poll. This is where waitpid is allowed.
    bool pollHealth() noexcept {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
#if defined(_WIN32)
        return false;
#else
        const pid_t child = m_pid.load(std::memory_order_acquire);
        if (!m_processAlive.load(std::memory_order_acquire) || child <= 0) return false;
        int status = 0;
        const pid_t result = ::waitpid(child, &status, WNOHANG);
        if (result == 0) {
            if (m_shared != nullptr) {
                const uint64_t heartbeat = m_shared->heartbeat.load(std::memory_order_acquire);
                if (heartbeat != m_lastHeartbeat) {
                    m_lastHeartbeat = heartbeat;
                    m_lastHeartbeatTime = std::chrono::steady_clock::now();
                }
                const uint64_t requested = m_shared->requestSequence.load(std::memory_order_acquire);
                const uint64_t completed = m_shared->completedSequence.load(std::memory_order_acquire);
                if (completed != m_lastCompletedSequence) {
                    m_lastCompletedSequence = completed;
                    m_lastProgressTime = std::chrono::steady_clock::now();
                } else if (requested != completed &&
                           std::chrono::steady_clock::now() - m_lastProgressTime >
                               std::chrono::milliseconds(250)) {
                    m_failure.store(Failure::ProcessHung, std::memory_order_release);
                    stop();
                    return false;
                }
            }
            return true;
        }
        m_failure.store(Failure::ProcessExited, std::memory_order_release);
        m_pid.store(-1, std::memory_order_release);
        m_processAlive.store(false, std::memory_order_release);
        closeIpcFds();
        if (m_sharedFd >= 0) { ::close(m_sharedFd); m_sharedFd = -1; }
        if (!m_sharedName.empty()) { ::shm_unlink(m_sharedName.c_str()); m_sharedName.clear(); }
        unmapShared();
        return false;
#endif
    }

    // Control-thread only. Recreates the helper after a crash or hang and
    // restores the last available plugin state. A failed restart leaves the
    // host stopped and preserves the failure reason for diagnostics.
    bool restart(bool force = true) {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        // State capture, process replacement, and state restore are one
        // control-thread transaction.  This prevents a concurrent save/load
        // operation from observing the stopped helper and losing the cache.
        std::lock_guard<std::mutex> stateLock(m_stateMutex);
        StateTransitionGuard transition(*this);
        if (force && failure() == Failure::PluginQuarantined) {
            m_restartAttempts = 0;
            m_failure.store(Failure::None, std::memory_order_release);
        }
        if (!force && std::chrono::steady_clock::now() < m_nextRestartTime) return false;
        if (isAlive() && !pollHealth()) return false;
        const std::vector<uint8_t> state = getStateUnlocked();
        stop();
        ++m_restartAttempts;
        if (!start()) {
            const uint32_t shift = std::min<uint32_t>(m_restartAttempts - 1, 8);
            m_nextRestartTime = std::chrono::steady_clock::now() +
                std::chrono::milliseconds(100u * (1u << shift));
            return false;
        }
        if (!state.empty() && !setStateUnlocked(state.data(), state.size())) {
            stop();
            m_failure.store(Failure::PluginInstanceFailed, std::memory_order_release);
            m_nextRestartTime = std::chrono::steady_clock::now() + std::chrono::seconds(30);
            return false;
        }
        m_nextRestartTime = std::chrono::steady_clock::time_point{};
        return true;
    }

    // Control-thread only. Reconfigure the isolated worker when the host
    // audio format changes. The plugin state is captured before the worker is
    // replaced and restored only after the new format has acknowledged its
    // handshake, so a failed reconfiguration cannot expose a half-initialized
    // worker to the audio graph.
    bool reconfigure(double sampleRate, uint32_t maxBlockSize) {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0 ||
            maxBlockSize == 0 || maxBlockSize > SandboxProtocol::kMaxFrames) {
            m_failure.store(Failure::ProcessFailed, std::memory_order_release);
            return false;
        }
        std::lock_guard<std::mutex> stateLock(m_stateMutex);
        StateTransitionGuard transition(*this);
        const std::vector<uint8_t> state = getStateUnlocked();
        const bool wasAlive = isAlive();
        const double previousSampleRate = m_sampleRate;
        const uint32_t previousBlockSize = m_maxBlockSize;
        if (wasAlive) stop();
        m_sampleRate = sampleRate;
        m_maxBlockSize = maxBlockSize;
        if (!wasAlive) return true;
        if (!start()) {
            // A rejected format must not strand an otherwise healthy plugin
            // in a stopped state. Restore the prior worker configuration and
            // state when possible; the caller still receives false for the
            // requested transition, but the audio graph remains recoverable.
            m_sampleRate = previousSampleRate;
            m_maxBlockSize = previousBlockSize;
            if (start() && !state.empty() &&
                !setStateUnlocked(state.data(), state.size())) {
                stop();
            }
            return false;
        }
        if (!state.empty() && !setStateUnlocked(state.data(), state.size())) {
            stop();
            m_failure.store(Failure::PluginInstanceFailed, std::memory_order_release);
            m_sampleRate = previousSampleRate;
            m_maxBlockSize = previousBlockSize;
            // Best-effort rollback. Keep the failure/quarantine state if the
            // old configuration itself can no longer be started.
            if (start() && !state.empty() &&
                !setStateUnlocked(state.data(), state.size())) {
                stop();
            }
            return false;
        }
        return true;
    }

    // Control-thread only. All state IPC must carry a complete generation
    // tuple; zero is intentionally rejected so legacy callers cannot bypass
    // stale-state protection.
    bool setGenerationContext(GenerationContext context) noexcept {
        if (!context.valid()) return false;
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        std::lock_guard<std::mutex> stateLock(m_stateMutex);
        m_generationContext = context;
        return true;
    }

    GenerationContext generationContext() const noexcept {
        std::lock_guard<std::mutex> stateLock(m_stateMutex);
        return m_generationContext;
    }

    bool autoRestart() {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        static constexpr uint32_t kMaxAutomaticRestarts = 5;
        if (m_restartAttempts >= kMaxAutomaticRestarts) {
            m_failure.store(Failure::PluginQuarantined, std::memory_order_release);
            return false;
        }
        return restart(false);
    }

    Failure failure() const noexcept { return m_failure.load(std::memory_order_acquire); }

    bool isQuarantined() const noexcept { return failure() == Failure::PluginQuarantined; }
    uint32_t restartAttempts() const noexcept { return m_restartAttempts; }
    void clearQuarantine() noexcept {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        if (isQuarantined()) {
            m_restartAttempts = 0;
            m_failure.store(Failure::None, std::memory_order_release);
        }
    }

    bool canRetry() const noexcept {
        const Failure state = failure();
        // Manual retry is an explicit user action and may clear quarantine.
        if (state == Failure::PluginQuarantined) return true;
        return state == Failure::ProcessExited || state == Failure::ProcessHung ||
               state == Failure::ProcessFailed ||
               state == Failure::PluginLoadFailed || state == Failure::PluginAbiInvalid ||
               state == Failure::PluginInstanceFailed;
    }

    bool canAutoRetry() const noexcept {
        const Failure state = failure();
        return state != Failure::None && state != Failure::PluginQuarantined && canRetry();
    }

    /// Enqueue an extended MIDI payload for the next submitted audio block.
    /// This is also the boundary used by the host-side SysEx/MIDI 2.0 adapter;
    /// it never allocates and fails closed when the bounded ring is full.
    bool enqueueExtendedMidi(uint64_t sampleOffset, uint8_t articulationId,
                             const uint8_t* bytes, size_t size) noexcept {
        if (!m_shared || !isAlive()) return false;
        if (!m_shared->extendedMidi.push(sampleOffset, articulationId, bytes, size)) {
            m_shared->inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
            return false;
        }
        return true;
    }

    // Control-plane producer for sample-accurate parameter automation. The
    // SPSC queue is bounded and the audio callback only drains it into the
    // current mailbox block; it never takes a lock or allocates.
    bool enqueueParameterChange(uint32_t parameterId, double value,
                                uint32_t sampleOffset = 0) noexcept {
        if (!std::isfinite(value) || sampleOffset > SandboxProtocol::kMaxFrames)
            return false;
        const uint64_t write = m_parameterWrite.load(std::memory_order_relaxed);
        const uint64_t read = m_parameterRead.load(std::memory_order_acquire);
        if (write - read >= SandboxProtocol::kMaxParameterChanges) return false;
        auto& change = m_pendingParameterChanges[
            write % SandboxProtocol::kMaxParameterChanges];
        change.parameterId = parameterId;
        change.sampleOffset = sampleOffset;
        change.value = value;
        m_parameterWrite.store(write + 1, std::memory_order_release);
        return true;
    }

    /**
     * @brief Process audio/MIDI with ZERO-LOCK IPC.
     * INDUSTRIAL: Sovereignty protected from child process stalls.
     */
