#pragma once

#include <atomic>
#include <array>
#include <cmath>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <vector>
#include "plugin_sandbox_host.hpp"
#include "../../dsp/iprocessor.hpp"

namespace Aura::Core::Plugins {

class ProcessSandboxProcessor final : public DSP::IProcessor {
public:
    enum class RecoveryMode : uint8_t {
        ClearBlock = SandboxProtocol::kRecoveryClearBlock,
        Quarantined = SandboxProtocol::kRecoveryQuarantined
    };
    static constexpr size_t kMaxStateBytes = SandboxProtocol::kMaxStateBytes;
    static constexpr uint32_t kMaxParameters = 128;
    static constexpr uint32_t kConsecutiveOverrunLimit = 8;

    explicit ProcessSandboxProcessor(std::string pluginPath, double sampleRate = 44100.0,
                                     uint32_t maxBlockSize = 512, std::string requestedFormat = "auto")
        : m_host(std::move(pluginPath), sampleRate, maxBlockSize,
                 SandboxProtocol::kMaxChannels, std::move(requestedFormat)),
          m_preparedSampleRate(sampleRate),
          m_preparedBlockSize(std::min(maxBlockSize, SandboxProtocol::kMaxFrames)) {
        // State IPC is generation-protected. A newly created processor must
        // still be able to save/restore before the project manager publishes
        // its first full generation tuple; the lifecycle can replace this
        // unique non-zero bootstrap context later.
        const uint64_t instance = s_generationSeed.fetch_add(1, std::memory_order_relaxed) + 1;
        m_host.setGenerationContext({1, instance, 1, 1});
    }

    bool start() {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        const bool started = m_host.start();
        if (!started) return false;
        if (replayParameterSnapshotLocked()) return true;
        m_host.stop();
        return false;
    }
    void stop() noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        m_host.stop();
    }
    bool isAlive() const noexcept { return m_host.isAlive(); }
    bool pollHealth() noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host.pollHealth() && !m_failed.load(std::memory_order_acquire);
    }
    bool restart(bool force = true) {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        const bool restarted = m_host.restart(force);
        if (restarted) {
            const bool parametersRestored = replayParameterSnapshotLocked();
            // A successful explicit restart is the recovery boundary.  Clear
            // any quarantine edge that raced with the old worker shutdown so
            // status snapshots cannot report a healthy worker as quarantined.
            m_host.clearQuarantine();
            m_failed.store(false, std::memory_order_release);
            m_consecutiveOverruns.store(0, std::memory_order_release);
            if (!parametersRestored) {
                m_failed.store(true, std::memory_order_release);
                m_host.stop();
            }
        }
        return restarted && !m_failed.load(std::memory_order_acquire);
    }
    bool setGenerationContext(PluginSandboxHost::GenerationContext context) noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host.setGenerationContext(context);
    }
    PluginSandboxHost::GenerationContext generationContext() const noexcept {
        return m_host.generationContext();
    }
    bool autoRestart() {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        const bool restarted = m_host.autoRestart();
        if (restarted) {
            const bool parametersRestored = replayParameterSnapshotLocked();
            m_failed.store(false, std::memory_order_release);
            m_consecutiveOverruns.store(0, std::memory_order_release);
            if (!parametersRestored) {
                m_failed.store(true, std::memory_order_release);
                m_host.stop();
            }
        }
        return restarted && !m_failed.load(std::memory_order_acquire);
    }
    PluginSandboxHost::Failure failure() const noexcept { return m_host.failure(); }
    bool canRetry() const noexcept { return m_host.canRetry(); }
    bool canAutoRetry() const noexcept { return m_host.canAutoRetry(); }
    bool isQuarantined() const noexcept { return m_host.isQuarantined(); }
    uint32_t restartAttempts() const noexcept { return m_host.restartAttempts(); }
    void clearQuarantine() noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        m_host.clearQuarantine();
        // Clearing the host quarantine is an explicit user/control action.
        // Reset the processor-side recovery latch as well, otherwise the
        // processor would remain permanently silent after a successful host
        // recovery.
        m_failed.store(false, std::memory_order_release);
        m_consecutiveOverruns.store(0, std::memory_order_release);
    }

    void prepareToPlay(double sampleRate, uint32_t blockSize) noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        // A worker block size is not plugin latency.  Until the isolated
        // adapter reports the plugin's actual latency, expose zero so PDC
        // never inserts a fabricated one-block delay.
        m_latencySamples.store(0, std::memory_order_release);
        if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || blockSize == 0) {
            m_failed.store(true, std::memory_order_release);
            return;
        }
        const uint32_t safeBlockSize = std::min(blockSize, SandboxProtocol::kMaxFrames);
        const bool changed = m_preparedSampleRate != sampleRate ||
                             m_preparedBlockSize != safeBlockSize;
        if (!changed) return;
        try {
            if (!m_host.reconfigure(sampleRate, safeBlockSize)) {
                m_failed.store(true, std::memory_order_release);
                return;
            }
        } catch (...) {
            m_failed.store(true, std::memory_order_release);
            return;
        }
        m_preparedSampleRate = sampleRate;
        m_preparedBlockSize = safeBlockSize;
        if (m_host.isAlive() && !replayParameterSnapshotLocked()) {
            m_failed.store(true, std::memory_order_release);
            m_host.stop();
            return;
        }
        m_failed.store(false, std::memory_order_release);
        m_consecutiveOverruns.store(0, std::memory_order_release);
    }
    bool processBlock(Core::AudioBuffer& buffer, Core::MidiBuffer& midi) noexcept {
        const bool produced = m_host.process(buffer, midi);
        const bool deadlineMissed = m_host.takeLastProcessOverrun();
        const bool completedLate = produced && m_host.takeCompletedSequenceOverrun();
        if (deadlineMissed || completedLate) {
            const uint32_t count = m_consecutiveOverruns.fetch_add(1, std::memory_order_acq_rel) + 1;
            if (count >= kConsecutiveOverrunLimit) {
                m_failed.store(true, std::memory_order_release);
            }
        } else if (produced) {
            m_consecutiveOverruns.store(0, std::memory_order_release);
        }
        if (!produced) {
            buffer.clear();
            // A false result is also the normal first half of the mailbox
            // pipeline: the block was submitted and its output is collected
            // on the next call. Only the explicit worker-side process error
            // is a processor failure.
            const auto failure = m_host.failure();
            if (failure == PluginSandboxHost::Failure::ProcessFailed)
                m_failed.store(true, std::memory_order_release);
            // Once the mailbox is no longer a valid source of output, do not
            // let input MIDI survive into a later recovery block. The normal
            // pending-mailbox path deliberately preserves MIDI; this branch
            // is only for an actual failure or quarantine state.
            if (m_failed.load(std::memory_order_acquire) ||
                failure == PluginSandboxHost::Failure::PluginQuarantined ||
                failure == PluginSandboxHost::Failure::ProcessExited ||
                failure == PluginSandboxHost::Failure::ProcessHung ||
                failure == PluginSandboxHost::Failure::ProcessFailed) {
                midi.clear();
            }
        }
        return produced && !m_failed.load(std::memory_order_acquire);
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext&) noexcept override {
        (void)processBlock(buffer, midi);
    }

    bool enqueueParameterChange(uint32_t parameterId, double value,
                                uint32_t sampleOffset = 0) noexcept {
        if (parameterId >= kMaxParameters || !std::isfinite(value)) return false;
        // Keep the desired host state even when the current worker mailbox is
        // full.  The next lifecycle boundary replays this snapshot, so a
        // transient overrun cannot silently reset a plug-in parameter.
        m_parameterSnapshot[parameterId].store(value, std::memory_order_release);
        m_parameterSnapshotValid[parameterId / 64].fetch_or(
            1ULL << (parameterId % 64), std::memory_order_release);
        return m_host.enqueueParameterChange(parameterId, value, sampleOffset);
    }
    void reset() noexcept override {}
    uint32_t getLatencySamples() const noexcept override {
        return m_latencySamples.load(std::memory_order_acquire);
    }
    bool processFailed() const noexcept { return m_failed.load(std::memory_order_acquire); }
    uint32_t consecutiveMailboxOverruns() const noexcept {
        return m_consecutiveOverruns.load(std::memory_order_acquire);
    }
    RecoveryMode recoveryMode() const noexcept {
        return m_failed.load(std::memory_order_acquire)
            ? RecoveryMode::Quarantined : RecoveryMode::ClearBlock;
    }
    uint32_t takeInputMidiTruncations() noexcept {
        return m_host.takeInputMidiTruncations();
    }
    void recordInputMidiRejection() noexcept { m_host.recordInputMidiRejection(); }
    uint32_t takeDroppedOutputMidi() noexcept { return m_host.takeDroppedOutputMidi(); }
    uint32_t takeMailboxOverruns() noexcept { return m_host.takeMailboxOverruns(); }
    uint8_t failureCode() const noexcept { return static_cast<uint8_t>(m_host.failure()); }
    uint8_t stateErrorCode() const noexcept { return m_host.stateErrorCode(); }
    const char* stateErrorText() const noexcept { return m_host.stateErrorText(); }
    const char* failureText() const noexcept {
        switch (m_host.failure()) {
        case PluginSandboxHost::Failure::None: return "none";
        case PluginSandboxHost::Failure::MissingHelper: return "missing-helper";
        case PluginSandboxHost::Failure::InvalidPluginPath: return "invalid-plugin-path";
        case PluginSandboxHost::Failure::SpawnFailed: return "spawn-failed";
        case PluginSandboxHost::Failure::HandshakeTimeout: return "handshake-timeout";
        case PluginSandboxHost::Failure::PluginLoadFailed: return "plugin-load-failed";
        case PluginSandboxHost::Failure::PluginAbiInvalid: return "plugin-abi-invalid";
        case PluginSandboxHost::Failure::PluginInstanceFailed: return "plugin-instance-failed";
        case PluginSandboxHost::Failure::SecuritySetupFailed: return "security-setup-failed";
        case PluginSandboxHost::Failure::PluginFormatUnsupported: return "plugin-format-unsupported";
        case PluginSandboxHost::Failure::PluginQuarantined: return "plugin-quarantined";
        case PluginSandboxHost::Failure::ProcessExited: return "process-exited";
        case PluginSandboxHost::Failure::ProcessHung: return "process-hung";
        case PluginSandboxHost::Failure::ProcessFailed: return "process-failed";
        case PluginSandboxHost::Failure::UnsupportedPlatform: return "unsupported-platform";
        }
        return "unknown-failure";
    }

    // State is exchanged only by the control thread.  The audio callback never
    // touches this vector, so project save/load cannot introduce RT allocation.
    std::vector<uint8_t> getState() const override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host.getState();
    }
    bool setState(const std::vector<uint8_t>& data) override {
        return restoreStateChecked(data);
    }
    bool restoreStateChecked(const std::vector<uint8_t>& data) override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        const bool restored = m_host.setState(data.data(), data.size());
        if (!restored) m_failed.store(true, std::memory_order_release);
        return restored;
    }

private:
    bool replayParameterSnapshotLocked() noexcept {
        bool ok = true;
        for (uint32_t parameterId = 0; parameterId < kMaxParameters; ++parameterId) {
            const uint64_t valid = m_parameterSnapshotValid[parameterId / 64].load(
                std::memory_order_acquire);
            if ((valid & (1ULL << (parameterId % 64))) == 0) continue;
            const double value = m_parameterSnapshot[parameterId].load(std::memory_order_acquire);
            if (!std::isfinite(value)) {
                ok = false;
                continue;
            }
            // Zero is a valid parameter value.  Replaying all slots is
            // intentional: the sandbox has no portable query for which
            // parameters were serialized by a third-party format.
            if (!m_host.enqueueParameterChange(parameterId, value, 0)) ok = false;
        }
        return ok;
    }

    PluginSandboxHost m_host;
    // Control-plane lifecycle serialization. processBlock() deliberately does
    // not take this mutex: audio callbacks must never wait for worker control.
    mutable std::mutex m_lifecycleMutex;
    std::atomic<bool> m_failed{false};
    std::atomic<uint32_t> m_consecutiveOverruns{0};
    std::atomic<uint32_t> m_latencySamples{0};
    double m_preparedSampleRate = 0.0;
    uint32_t m_preparedBlockSize = 0;
    std::array<std::atomic<double>, kMaxParameters> m_parameterSnapshot{};
    std::array<std::atomic<uint64_t>, 2> m_parameterSnapshotValid{};
    inline static std::atomic<uint64_t> s_generationSeed{0};
};

} // namespace Aura::Core::Plugins
