    bool start() {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        m_failure.store(Failure::None, std::memory_order_release);
        if (m_pluginPath.empty()) {
            m_failure.store(Failure::InvalidPluginPath, std::memory_order_release);
            return false;
        }
        // Keep direct sandbox users subject to the same admission boundary as
        // the registry. Built-in protocol URIs are handled by the worker and
        // are intentionally exempt from filesystem admission.
        if (m_pluginPath.rfind("builtin://", 0) != 0) {
            std::string expectedFormat;
            if (m_requestedFormat == "vst3" || m_requestedFormat == "VST3") expectedFormat = "VST3";
            else if (m_requestedFormat == "au" || m_requestedFormat == "AU") expectedFormat = "AU";
            else if (m_requestedFormat == "clap" || m_requestedFormat == "CLAP") expectedFormat = "CLAP";
            if (!PluginAdmission::isSafeCandidate(std::filesystem::path(m_pluginPath), expectedFormat)) {
                m_failure.store(Failure::InvalidPluginPath, std::memory_order_release);
                return false;
            }
        }
        // A live worker is not a path error.  Keep the existing lifecycle
        // state intact so callers can distinguish an idempotency mistake
        // from a genuinely invalid plugin path and recover deterministically.
        if (isAlive()) return false;
        std::string helperPath;
        if (const char* configured = std::getenv("AURA_PLUGIN_HOST_BIN"); configured && *configured != '\0') {
            helperPath = configured;
        }
#if defined(__APPLE__)
        if (helperPath.empty() || ::access(helperPath.c_str(), X_OK) != 0) {
            uint32_t bufferSize = 0;
            (void)_NSGetExecutablePath(nullptr, &bufferSize);
            if (bufferSize > 0) {
                std::vector<char> buffer(static_cast<size_t>(bufferSize) + 1, '\0');
                if (_NSGetExecutablePath(buffer.data(), &bufferSize) == 0) {
                    std::string executable(buffer.data());
                    const auto slash = executable.rfind('/');
                    if (slash != std::string::npos) {
                        const std::string bundled = executable.substr(0, slash + 1) + "aura-plugin-host-worker";
                        if (::access(bundled.c_str(), X_OK) == 0) helperPath = bundled;
                    }
                }
            }
        }
#endif
        if (helperPath.empty() || ::access(helperPath.c_str(), X_OK) != 0) {
            m_failure.store(Failure::MissingHelper, std::memory_order_release);
            return false;
        }
#if defined(_WIN32)
        // Windows requires CreateProcessW plus a Job Object. Do not fall back
        // to in-process loading until that lifecycle exists.
        m_failure.store(Failure::UnsupportedPlatform, std::memory_order_release);
        return false;
#else
        m_sharedName = "/aura_plugin_" + std::to_string(static_cast<unsigned long long>(::getpid())) +
                       "_" + std::to_string(++s_sharedNameCounter);
        m_sharedFd = ::shm_open(m_sharedName.c_str(), O_CREAT | O_EXCL | O_RDWR, 0600);
        if (m_sharedFd < 0 || ::ftruncate(m_sharedFd, sizeof(SandboxProtocol::SharedAudioBlock)) != 0) {
            if (m_sharedFd >= 0) ::close(m_sharedFd);
            m_sharedFd = -1;
            ::shm_unlink(m_sharedName.c_str());
            m_failure.store(Failure::SpawnFailed, std::memory_order_release);
            return false;
        }
        const int fdFlags = ::fcntl(m_sharedFd, F_GETFD, 0);
        if (fdFlags >= 0) ::fcntl(m_sharedFd, F_SETFD, fdFlags & ~FD_CLOEXEC);
        m_shared = static_cast<SandboxProtocol::SharedAudioBlock*>(::mmap(
            nullptr, sizeof(SandboxProtocol::SharedAudioBlock), PROT_READ | PROT_WRITE,
            MAP_SHARED, m_sharedFd, 0));
        if (m_shared == MAP_FAILED) {
            m_shared = nullptr;
            unmapShared();
            if (m_sharedFd >= 0) {
                ::close(m_sharedFd);
                m_sharedFd = -1;
            }
            if (!m_sharedName.empty()) {
                ::shm_unlink(m_sharedName.c_str());
                m_sharedName.clear();
            }
            m_failure.store(Failure::SpawnFailed, std::memory_order_release);
            return false;
        }
        new (m_shared) SandboxProtocol::SharedAudioBlock{};
        m_shared->protocolVersion.store(
            SandboxProtocol::kAudioBlockProtocolVersion, std::memory_order_release);

        int parentToChild[2] = {-1, -1};
        int childToParent[2] = {-1, -1};
        if (::pipe(parentToChild) != 0 || ::pipe(childToParent) != 0) {
            if (parentToChild[0] >= 0) { ::close(parentToChild[0]); ::close(parentToChild[1]); }
            if (childToParent[0] >= 0) { ::close(childToParent[0]); ::close(childToParent[1]); }
            unmapShared();
            if (m_sharedFd >= 0) { ::close(m_sharedFd); m_sharedFd = -1; }
            ::shm_unlink(m_sharedName.c_str());
            m_sharedName.clear();
            m_failure.store(Failure::SpawnFailed, std::memory_order_release);
            return false;
        }

        pid_t child = -1;
        // The child protocol uses stable descriptor numbers, but the source
        // descriptors allocated by the parent can eventually be 10, 11, or
        // 12. Move child-bound sources to a high CLOEXEC range first so a
        // dup2 action cannot overwrite a later source descriptor.
        const auto duplicateSpawnSource = [](int descriptor) {
#if defined(F_DUPFD_CLOEXEC)
            return ::fcntl(descriptor, F_DUPFD_CLOEXEC, 100);
#else
            const int duplicate = ::fcntl(descriptor, F_DUPFD, 100);
            if (duplicate >= 0) {
                const int flags = ::fcntl(duplicate, F_GETFD, 0);
                if (flags >= 0) ::fcntl(duplicate, F_SETFD, flags | FD_CLOEXEC);
            }
            return duplicate;
#endif
        };
        const int spawnControlFd = duplicateSpawnSource(parentToChild[0]);
        const int spawnStatusFd = duplicateSpawnSource(childToParent[1]);
        const int spawnSharedFd = duplicateSpawnSource(m_sharedFd);
        if (spawnControlFd < 0 || spawnStatusFd < 0 || spawnSharedFd < 0) {
            if (spawnControlFd >= 0) ::close(spawnControlFd);
            if (spawnStatusFd >= 0) ::close(spawnStatusFd);
            if (spawnSharedFd >= 0) ::close(spawnSharedFd);
            ::close(parentToChild[0]); ::close(parentToChild[1]);
            ::close(childToParent[0]); ::close(childToParent[1]);
            unmapShared();
            if (m_sharedFd >= 0) { ::close(m_sharedFd); m_sharedFd = -1; }
            ::shm_unlink(m_sharedName.c_str());
            m_sharedName.clear();
            m_failure.store(Failure::SpawnFailed, std::memory_order_release);
            return false;
        }
        posix_spawn_file_actions_t actions;
        ::posix_spawn_file_actions_init(&actions);
        constexpr int kControlFd = 10;
        constexpr int kStatusFd = 11;
        constexpr int kSharedFd = 12;
        const bool actionsOk =
            ::posix_spawn_file_actions_adddup2(&actions, spawnControlFd, kControlFd) == 0 &&
            ::posix_spawn_file_actions_adddup2(&actions, spawnStatusFd, kStatusFd) == 0 &&
            ::posix_spawn_file_actions_adddup2(&actions, spawnSharedFd, kSharedFd) == 0;
        std::string controlFdString = std::to_string(kControlFd);
        std::string statusFdString = std::to_string(kStatusFd);
        std::string sharedFdString = std::to_string(kSharedFd);
        std::string sampleRateString = std::to_string(m_sampleRate);
        std::string minFramesString = "1";
        std::string maxFramesString = std::to_string(m_maxBlockSize);
        std::string channelsString = std::to_string(m_channels);
        const auto formatForPath = [](const std::string& path) {
            const auto lower = [](std::string value) {
                for (char& character : value)
                    character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
                return value;
            };
            const std::string normalized = lower(path);
            if (normalized.rfind("builtin://", 0) == 0) return std::string("builtin");
            if (normalized.size() >= 5 && normalized.compare(normalized.size() - 5, 5, ".clap") == 0)
                return std::string("clap");
            if (normalized.size() >= 5 && normalized.compare(normalized.size() - 5, 5, ".vst3") == 0)
                return std::string("vst3");
            if (normalized.size() >= 10 && normalized.compare(normalized.size() - 10, 10, ".component") == 0)
                return std::string("au");
            return std::string("auto");
        };
        std::string formatString = m_requestedFormat.empty() || m_requestedFormat == "auto"
            ? formatForPath(m_pluginPath) : m_requestedFormat;
        char* const childArgv[] = {
            const_cast<char*>(helperPath.c_str()),
            const_cast<char*>("--plugin"), const_cast<char*>(m_pluginPath.c_str()),
            const_cast<char*>("--format"), const_cast<char*>(formatString.c_str()),
            const_cast<char*>("--control-fd"), const_cast<char*>(controlFdString.c_str()),
            const_cast<char*>("--status-fd"), const_cast<char*>(statusFdString.c_str()),
            const_cast<char*>("--shared-fd"), const_cast<char*>(sharedFdString.c_str()),
            const_cast<char*>("--shared-name"), const_cast<char*>(m_sharedName.c_str()),
            const_cast<char*>("--sample-rate"), const_cast<char*>(sampleRateString.c_str()),
            const_cast<char*>("--min-frames"), const_cast<char*>(minFramesString.c_str()),
            const_cast<char*>("--max-frames"), const_cast<char*>(maxFramesString.c_str()),
            const_cast<char*>("--channels"), const_cast<char*>(channelsString.c_str()),
            nullptr};
        const int spawnResult = actionsOk
            ? ::posix_spawn(&child, helperPath.c_str(), &actions, nullptr, childArgv, environ)
            : EINVAL;
        ::posix_spawn_file_actions_destroy(&actions);
        ::close(spawnControlFd);
        ::close(spawnStatusFd);
        ::close(spawnSharedFd);
        if (spawnResult != 0) {
            ::close(parentToChild[0]); ::close(parentToChild[1]);
            ::close(childToParent[0]); ::close(childToParent[1]);
            unmapShared();
            if (m_sharedFd >= 0) { ::close(m_sharedFd); m_sharedFd = -1; }
            ::shm_unlink(m_sharedName.c_str());
            m_sharedName.clear();
            m_failure.store(Failure::SpawnFailed, std::memory_order_release);
            return false;
        }
        ::close(parentToChild[0]);
        ::close(childToParent[1]);
        m_controlFd = parentToChild[1];
        m_statusFd = childToParent[0];
        const int flags = ::fcntl(m_statusFd, F_GETFL, 0);
        if (flags >= 0) ::fcntl(m_statusFd, F_SETFL, flags | O_NONBLOCK);
        m_pid.store(child, std::memory_order_release);
        // The first launch of a large plugin may fault in its code signature
        // pages and initialize worker-side state.  A one-second deadline made
        // a healthy plugin look dead when the DAW was under startup load.
        // Vendor bundles may perform signature validation and one-time
        // preset/shader discovery before sending the ready byte.  Keep the
        // handshake bounded, but give real VST3 hosts the same cold-start
        // budget as the Rust lifecycle tests (30s).
        int readyTimeoutMs = 30000;
        if (const char* configured = std::getenv("AURA_PLUGIN_READY_TIMEOUT_MS")) {
            char* end = nullptr;
            const long value = std::strtol(configured, &end, 10);
            if (end != configured && *end == '\0') {
                readyTimeoutMs = static_cast<int>(std::clamp(value, 1000L, 120000L));
            }
        }
        if (!waitForReady(readyTimeoutMs)) {
            stop();
            if (m_failure.load(std::memory_order_acquire) == Failure::None) {
                m_failure.store(Failure::HandshakeTimeout, std::memory_order_release);
            }
            return false;
        }
        m_processAlive.store(true, std::memory_order_release);
        // A new worker gets a new mailbox lifetime. Never allow local
        // pending audio/MIDI or a sequence from the dead worker to be
        // interpreted as belonging to this session.
        m_sequence = 0;
        m_lastSubmitted = 0;
        m_pendingChannels = 0;
        m_pendingFrames = 0;
        std::memset(m_pendingMidi, 0, sizeof(m_pendingMidi));
        m_pendingAgeFrames = 0;
        m_overrunReportedForSequence = false;
        m_sequenceHadOverrun = false;
        m_completedSequenceHadOverrun = false;
        m_lastHeartbeat = 0;
        m_lastCompletedSequence = 0;
        m_lastHeartbeatTime = std::chrono::steady_clock::now();
        m_lastProgressTime = m_lastHeartbeatTime;
        return true;
#endif
    }

    // Control-thread only. Never call this from process().
