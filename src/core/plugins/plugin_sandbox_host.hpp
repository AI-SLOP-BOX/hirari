#pragma once
#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <mutex>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <new>
#include <chrono>
#include <limits>
#include <cstdlib>
#include <cctype>
#if !defined(_WIN32)
#include <cerrno>
#include <csignal>
#include <fcntl.h>
#include <poll.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/mman.h>
#include <unistd.h>
#include <spawn.h>
#if defined(__APPLE__)
#include <mach-o/dyld.h>
#endif

#if !defined(_WIN32)
extern char** environ;
#endif
#endif
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "plugin_sandbox_protocol.hpp"

namespace Aura::Core::Plugins {

/**
 * @class PluginSandboxHost
 * @brief Process-lifecycle host for isolated third-party plugins.
 *
 * The helper is a separate OS process. The parent never loads the plugin
 * binary, and it will not expose a live state until the helper acknowledges
 * initialization over the control pipe.
 */
class PluginSandboxHost {
public:
    struct GenerationContext {
        uint64_t project = 0;
        uint64_t plugin = 0;
        uint64_t audio = 0;
        uint64_t state = 0;
        bool valid() const noexcept {
            return project != 0 && plugin != 0 && audio != 0 && state != 0;
        }
    };
    enum class Failure : uint8_t {
        None,
        MissingHelper,
        InvalidPluginPath,
        SpawnFailed,
        HandshakeTimeout,
        PluginLoadFailed,
        PluginAbiInvalid,
        PluginInstanceFailed,
        SecuritySetupFailed,
        PluginFormatUnsupported,
        PluginQuarantined,
        ProcessExited,
        ProcessHung,
        ProcessFailed,
        UnsupportedPlatform,
    };

    PluginSandboxHost(const std::string& pluginPath, double sampleRate = 44100.0,
                      uint32_t maxBlockSize = 512, uint32_t channels = SandboxProtocol::kMaxChannels,
                      std::string requestedFormat = "auto")
        : m_pluginPath(pluginPath),
          m_requestedFormat(std::move(requestedFormat)),
          m_sampleRate(std::isfinite(sampleRate) && sampleRate > 0.0 ? sampleRate : 44100.0),
          m_maxBlockSize(std::clamp(maxBlockSize, 1u, SandboxProtocol::kMaxFrames)),
          m_channels(std::clamp(channels, 1u, SandboxProtocol::kMaxChannels)),
          m_processAlive(false) {}

    ~PluginSandboxHost() { stop(); }

    PluginSandboxHost(const PluginSandboxHost&) = delete;
    PluginSandboxHost& operator=(const PluginSandboxHost&) = delete;

    // Control-thread only. The helper is explicitly supplied so a missing
    // helper cannot be mistaken for a live sandbox.
    bool start() {
        std::lock_guard<std::recursive_mutex> lifecycleLock(m_lifecycleMutex);
        m_failure.store(Failure::None, std::memory_order_release);
        if (m_pluginPath.empty() || isAlive()) {
            m_failure.store(Failure::InvalidPluginPath, std::memory_order_release);
            return false;
        }
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
        if (!waitForReady(5000)) {
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
    bool process(AudioBuffer& audio, MidiBuffer& midi) noexcept {
        if (!isAlive()) return false;
        m_lastProcessOverrun.store(false, std::memory_order_release);
        m_processCalls.fetch_add(1, std::memory_order_acq_rel);
        struct ProcessGuard {
            std::atomic<uint32_t>& calls;
            ~ProcessGuard() { calls.fetch_sub(1, std::memory_order_release); }
        } guard{m_processCalls};
        // A state/reconfigure transaction may begin immediately after the
        // initial liveness check.  Count this call before checking the gate so
        // the control thread can wait for it deterministically; never submit
        // audio while state IPC is mutating the shared mailbox.
        if (m_stateTransition.load(std::memory_order_acquire) || !isAlive()) {
            audio.clear();
            midi.clear();
            return false;
        }
        // The shared mailbox has a fixed channel layout. Never silently
        // process only the first channels of a wider caller buffer: that
        // would produce plausible-looking but channel-corrupted audio for
        // surround or multichannel routes. A graph adapter must negotiate a
        // matching layout before this worker is used.
        if (m_shared == nullptr || audio.getNumChannels() == 0 ||
            audio.getNumChannels() > m_channels ||
            audio.getNumChannels() > SandboxProtocol::kMaxChannels ||
            audio.getNumSamples() == 0 || audio.getNumSamples() > SandboxProtocol::kMaxFrames) {
            audio.clear();
            midi.clear();
            return false;
        }

        const uint32_t currentChannels = std::min<uint32_t>(audio.getNumChannels(), SandboxProtocol::kMaxChannels);
        const uint32_t currentFrames = std::min<uint32_t>(audio.getNumSamples(), SandboxProtocol::kMaxFrames);
        const uint64_t completed = m_shared->completedSequence.load(std::memory_order_acquire);
        const bool requestInFlight =
            m_lastSubmitted != 0 &&
            m_shared->requestSequence.load(std::memory_order_acquire) != completed;
        // A completed mailbox block is copied back into the caller's buffer.
        // Preserve this call's input first so collecting the previous block
        // cannot accidentally feed the previous output back into the worker.
        size_t inputMidiCount = 0;
        uint32_t parameterChangeCount = 0;
        if (!requestInFlight) {
            for (uint32_t channel = 0; channel < currentChannels; ++channel) {
                const float* source = audio.getReadPointer(channel);
                if (!source) { audio.clear(); return false; }
                std::memcpy(m_pendingInput[channel], source,
                            static_cast<size_t>(currentFrames) * sizeof(float));
            }
            m_pendingChannels = currentChannels;
            m_pendingFrames = currentFrames;
            for (size_t sourceIndex = 0;
                 sourceIndex < midi.size() && inputMidiCount < SandboxProtocol::kMaxMidiEvents;
                 ++sourceIndex) {
                const auto& event = midi.getEvents()[sourceIndex];
                if (event.size > sizeof(SandboxProtocol::MidiEvent::data)) {
                    // Keep large SysEx/MIDI 2.0 payloads intact in the
                    // bounded extended ring instead of truncating them into
                    // the legacy event mailbox.
                    if (!m_shared->extendedMidi.push(event.sampleOffset,
                                                     event.articulationId,
                                                     event.data, event.size)) {
                        m_shared->inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
                    }
                    continue;
                }
                auto& pending = m_pendingMidi[inputMidiCount++];
                pending.sampleOffset = event.sampleOffset;
                pending.size = event.size;
                pending.articulationId = event.articulationId;
                std::memcpy(pending.data, event.data, pending.size);
            }
            if (midi.size() > SandboxProtocol::kMaxMidiEvents) {
                m_shared->inputMidiTruncations.fetch_add(
                    static_cast<uint32_t>(midi.size() - SandboxProtocol::kMaxMidiEvents),
                    std::memory_order_relaxed);
            }
            const uint64_t parameterWrite = m_parameterWrite.load(std::memory_order_acquire);
            uint64_t parameterRead = m_parameterRead.load(std::memory_order_relaxed);
            const uint64_t available = parameterWrite - parameterRead;
            parameterChangeCount = static_cast<uint32_t>(std::min<uint64_t>(
                available, SandboxProtocol::kMaxParameterChanges));
            for (uint32_t index = 0; index < parameterChangeCount; ++index) {
                m_shared->parameterChange[index] = m_pendingParameterChanges[
                    (parameterRead + index) % SandboxProtocol::kMaxParameterChanges];
            }
            m_parameterRead.store(parameterRead + parameterChangeCount,
                                   std::memory_order_release);
        }
        bool produced = false;
        bool submitPreservedInput = false;
        // Audio mailbox ownership is one-block exact: accepting a later
        // sequence here would copy a newer block into the wrong caller.
        if (m_lastSubmitted != 0 && completed == m_lastSubmitted) {
            const uint32_t channels = std::min<uint32_t>(audio.getNumChannels(), SandboxProtocol::kMaxChannels);
            const uint32_t frames = std::min<uint32_t>(audio.getNumSamples(), SandboxProtocol::kMaxFrames);
            for (uint32_t channel = 0; channel < channels; ++channel) {
                std::memcpy(audio.getWritePointer(channel), m_shared->output[channel],
                            static_cast<size_t>(frames) * sizeof(float));
            }
            const uint32_t outputMidiCount = std::min<uint32_t>(
                m_shared->outputMidiEvents.load(std::memory_order_acquire),
                SandboxProtocol::kMaxMidiEvents);
            for (uint32_t index = 0; index < outputMidiCount; ++index) {
                const auto& event = m_shared->outputMidi[index];
                if (event.sampleOffset < frames && event.size > 0 && event.size <= sizeof(event.data)) {
                    midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
                }
            }
            // The plugin's output is appended after the input events. Sort
            // once at the boundary so downstream processors see monotonic
            // sample offsets without any allocation.
            midi.sort();
            produced = true;
            submitPreservedInput = true;
            m_completedSequenceHadOverrun = m_sequenceHadOverrun;
            m_sequenceHadOverrun = false;
            m_lastSubmitted = 0;
            m_pendingAgeFrames = 0;
            m_overrunReportedForSequence = false;
            if (m_shared->processErrors.exchange(0, std::memory_order_acq_rel) > 0) {
                m_failure.store(Failure::ProcessFailed, std::memory_order_release);
                audio.clear();
                return false;
            }
        }

        // Never overwrite an in-flight block. The caller receives a bounded
        // silent fallback for this cycle instead of blocking the audio thread.
        if (m_shared->requestSequence.load(std::memory_order_acquire) != completed) {
            // A caller may poll the one-block mailbox several times before
            // the worker is scheduled.  That is normal pending state, not an
            // overrun.  Only flag a deadline miss after the bounded grace
            // period; otherwise a healthy worker is quarantined by a tight
            // test/UI polling loop.
            // Audio-thread deadline accounting is expressed in frames rather
            // than wall-clock calls. Wall-clock hang detection remains on the
            // control thread in pollHealth().
            const double deadlineFrames = std::max(1.0, m_sampleRate * 0.005);
            const uint64_t ageIncrement = currentFrames;
            m_pendingAgeFrames = m_pendingAgeFrames >
                    std::numeric_limits<uint64_t>::max() - ageIncrement
                ? std::numeric_limits<uint64_t>::max()
                : m_pendingAgeFrames + ageIncrement;
            if (!m_overrunReportedForSequence &&
                static_cast<double>(m_pendingAgeFrames) >= deadlineFrames) {
                m_shared->mailboxOverruns.fetch_add(1, std::memory_order_relaxed);
                m_lastProcessOverrun.store(true, std::memory_order_release);
                m_sequenceHadOverrun = true;
                // Count one deadline miss per submitted sequence. Repeated
                // polling of the same in-flight block must not turn a single
                // slow plugin call into eight independent overruns and an
                // unjust quarantine.
                m_overrunReportedForSequence = true;
            }
            if (!produced) audio.clear();
            return produced;
        }

        const uint32_t channels = submitPreservedInput ? m_pendingChannels : currentChannels;
        const uint32_t frames = submitPreservedInput ? m_pendingFrames : currentFrames;
        m_shared->channels.store(channels, std::memory_order_relaxed);
        m_shared->frames.store(frames, std::memory_order_relaxed);
        for (uint32_t channel = 0; channel < channels; ++channel) {
            const float* source = submitPreservedInput ? m_pendingInput[channel] : audio.getReadPointer(channel);
            if (source == nullptr) { audio.clear(); return produced; }
            std::memcpy(m_shared->input[channel], source, static_cast<size_t>(frames) * sizeof(float));
        }
        for (size_t index = 0; index < inputMidiCount; ++index) {
            const auto& event = m_pendingMidi[index];
            auto& destination = m_shared->midi[index];
            destination.sampleOffset = event.sampleOffset;
            destination.size = event.size;
            destination.articulationId = event.articulationId;
            std::memcpy(destination.data, event.data, destination.size);
        }
        m_shared->midiEvents.store(static_cast<uint32_t>(inputMidiCount), std::memory_order_relaxed);
        m_shared->parameterChanges.store(parameterChangeCount, std::memory_order_release);
        m_shared->outputMidiEvents.store(0, std::memory_order_relaxed);
        m_shared->outputMidiDropped.store(0, std::memory_order_relaxed);
        const uint64_t sequence = ++m_sequence;
        m_lastSubmitted = sequence;
        m_pendingAgeFrames = 0;
        m_overrunReportedForSequence = false;
        m_shared->requestSequence.store(sequence, std::memory_order_release);
        if (!produced) audio.clear();
        return produced;
    }

    uint64_t getHeartbeat() const {
#if !defined(_WIN32)
        return m_shared != nullptr ? m_shared->heartbeat.load(std::memory_order_acquire) : 0;
#else
        return m_heartbeat.load(std::memory_order_acquire);
#endif
    }

    uint32_t takeDroppedOutputMidi() noexcept {
#if defined(_WIN32)
        return 0;
#else
        return m_shared ? m_shared->outputMidiDropped.exchange(0, std::memory_order_acq_rel) : 0;
#endif
    }

    uint32_t takeMailboxOverruns() noexcept {
        return m_shared ? m_shared->mailboxOverruns.exchange(0, std::memory_order_acq_rel) : 0;
    }

    uint32_t takeInputMidiTruncations() noexcept {
        return m_shared ? m_shared->inputMidiTruncations.exchange(0, std::memory_order_acq_rel) : 0;
    }

    void recordInputMidiRejection() noexcept {
        if (m_shared) m_shared->inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
    }

    bool takeLastProcessOverrun() noexcept {
        return m_lastProcessOverrun.exchange(false, std::memory_order_acq_rel);
    }

    // Audio-thread only. The completion call must be able to tell the
    // processor that the block it just collected had previously missed its
    // deadline; otherwise a slow block would increment the consecutive
    // overrun counter and immediately reset it on completion.
    bool takeCompletedSequenceOverrun() noexcept {
        const bool value = m_completedSequenceHadOverrun;
        m_completedSequenceHadOverrun = false;
        return value;
    }

    uint32_t activeSampleRate() const noexcept {
        return m_shared ? m_shared->activeSampleRate.load(std::memory_order_acquire) : 0;
    }

    uint32_t activeChannels() const noexcept {
        return m_shared ? m_shared->activeChannels.load(std::memory_order_acquire) : 0;
    }

    uint32_t takeProcessErrors() noexcept {
#if defined(_WIN32)
        return 0;
#else
        return m_shared ? m_shared->processErrors.exchange(0, std::memory_order_acq_rel) : 0;
#endif
    }

    // Control-thread only. State IPC is bounded and has a finite timeout;
    // it is never used by the real-time process() path.
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

    std::string m_pluginPath;
    std::string m_requestedFormat = "auto";
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 512;
    uint32_t m_channels = SandboxProtocol::kMaxChannels;
    std::atomic<bool> m_processAlive;
    std::atomic<uint32_t> m_processCalls{0};
    // Audio calls observe this gate without taking the state mutex.  The
    // control thread sets it before waiting for in-flight calls, closing the
    // race between the wait and the first state mailbox write.
    std::atomic<bool> m_stateTransition{false};
    std::atomic<Failure> m_failure{Failure::None};
    std::atomic<uint64_t> m_heartbeat{0};
#if !defined(_WIN32)
    std::atomic<pid_t> m_pid{-1};
    int m_controlFd = -1;
    int m_statusFd = -1;
    int m_sharedFd = -1;
    std::string m_sharedName;
    SandboxProtocol::SharedAudioBlock* m_shared = nullptr;
    inline static std::atomic<uint64_t> s_sharedNameCounter{0};
#endif
    uint64_t m_sequence = 0;
    mutable std::mutex m_stateMutex;
    // Lifecycle operations are serialized as one control-plane transaction.
    // recursive_mutex is intentional: restart/reconfigure call start/stop
    // internally while holding this lock. The audio callback never takes it.
    mutable std::recursive_mutex m_lifecycleMutex;
    mutable uint64_t m_stateSequence = 0;
    GenerationContext m_generationContext{};
    mutable std::atomic<uint8_t> m_lastStateError{SandboxProtocol::kStateErrorNone};
    std::vector<uint8_t> m_stateCache;
    uint32_t m_restartAttempts = 0;
    std::chrono::steady_clock::time_point m_nextRestartTime{};
    uint64_t m_lastSubmitted = 0;
    uint64_t m_pendingAgeFrames = 0;
    // Audio-thread-owned; it is reset whenever a new mailbox sequence is
    // submitted or the previous sequence is collected.
    bool m_overrunReportedForSequence = false;
    bool m_sequenceHadOverrun = false;
    bool m_completedSequenceHadOverrun = false;
    alignas(64) float m_pendingInput[SandboxProtocol::kMaxChannels][SandboxProtocol::kMaxFrames]{};
    SandboxProtocol::MidiEvent m_pendingMidi[SandboxProtocol::kMaxMidiEvents]{};
    std::array<SandboxProtocol::ParameterChange,
               SandboxProtocol::kMaxParameterChanges> m_pendingParameterChanges{};
    std::atomic<uint64_t> m_parameterWrite{0};
    std::atomic<uint64_t> m_parameterRead{0};
    uint32_t m_pendingChannels = 0;
    uint32_t m_pendingFrames = 0;
    std::atomic<bool> m_lastProcessOverrun{false};
    uint64_t m_lastHeartbeat = 0;
    uint64_t m_lastCompletedSequence = 0;
#if !defined(_WIN32)
    std::chrono::steady_clock::time_point m_lastHeartbeatTime{};
    std::chrono::steady_clock::time_point m_lastProgressTime{};
#endif
};

} // namespace Aura::Core::Plugins
