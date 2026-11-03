#pragma once

#include <atomic>
#include <array>
#include <cmath>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <vector>
#include "hirari-core-bridge/src/plugin_sandbox_host.rs.h"
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "plugin_sandbox_protocol.hpp"
#include "../../dsp/iprocessor.hpp"

namespace Hirari::Core::Plugins {

// The process lifecycle, IPC framing, shared-memory mailbox, timeouts and
// restart policy live in the Rust-owned `SandboxHost`
// (hirari-core-bridge/src/plugin_sandbox_host.rs). This processor is the C++
// DSP boundary the engine's virtual chain calls; it holds the Rust host box
// directly and marshals audio/MIDI around it.
class ProcessSandboxProcessor final : public DSP::IProcessor {
public:
    enum class RecoveryMode : uint8_t {
        ClearBlock = SandboxProtocol::kRecoveryClearBlock,
        Quarantined = SandboxProtocol::kRecoveryQuarantined
    };
    // Bit returned by hirari_plugin_sandbox_block_outcome.
    static constexpr std::uint32_t kBlockClearMidi = 2;

    explicit ProcessSandboxProcessor(std::string pluginPath, double sampleRate = 44100.0,
                                     uint32_t maxBlockSize = 512, std::string requestedFormat = "auto")
        : m_host(::Hirari::Core::Bridge::Sandbox::new_sandbox_host(
              ::rust::Str(pluginPath), sampleRate, maxBlockSize,
              SandboxProtocol::kMaxChannels, ::rust::Str(requestedFormat))) {
        // State IPC is generation-protected. A newly created processor must
        // still be able to save/restore before the project manager publishes
        // its first full generation tuple; the lifecycle can replace this
        // unique non-zero bootstrap context later.
        const uint64_t instance = s_generationSeed.fetch_add(1, std::memory_order_relaxed) + 1;
        m_host->set_generation_context(1, instance, 1, 1);
    }

    ~ProcessSandboxProcessor() override { m_host->stop(); }

    bool start() {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host->start();
    }
    void stop() noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        m_host->stop();
    }
    bool isAlive() const noexcept { return m_host->is_alive(); }
    bool isMidiInstrument() const noexcept override { return m_host->is_midi_instrument(); }
    bool pollHealth() noexcept {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host->poll_health() && !m_host->recovery_failed();
    }
    // Parameter replay, the quarantine clear and the failure latch after a
    // restart are all owned by the Rust lifecycle.
    bool restart(bool force = true) {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host->restart(force);
    }
    bool autoRestart() {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        return m_host->auto_restart();
    }
    bool canRetry() const noexcept { return m_host->can_retry(); }
    bool canAutoRetry() const noexcept { return m_host->can_auto_retry(); }

    void prepareToPlay(double sampleRate, uint32_t blockSize) noexcept override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        // A worker block size is not plugin latency.  Until the isolated
        // adapter reports the plugin's actual latency, expose zero so PDC
        // never inserts a fabricated one-block delay.
        m_latencySamples.store(0, std::memory_order_release);
        (void)m_host->prepare(sampleRate, blockSize);
    }

    // Audio-thread boundary: atomics and the preallocated shared mailbox only.
    bool processBlock(Core::AudioBuffer& audio, Core::MidiBuffer& midi) noexcept {
        const uint32_t channels = audio.getNumChannels();
        const uint32_t frames = audio.getNumSamples();
        bool produced = false;
        if (m_host->is_alive()) {
            m_host->reset_staged_midi();
            if (midi.size() > 0) {
                for (size_t index = 0; index < midi.size(); ++index) {
                    const auto& event = midi.getEvents()[index];
                    // The Rust mailbox mirrors the exact slice deadline and
                    // the oversized-payload ring; rejected events increment
                    // the same truncation telemetry the C++ host used.
                    (void)m_host->stage_input_midi(
                        event.sampleOffset, event.articulationId,
                        ::rust::Slice<const uint8_t>(event.data, event.size), frames);
                }
            }
            const float* left = nullptr;
            const float* right = nullptr;
            if (channels > 0) left = audio.getReadPointer(0);
            if (channels > 1) right = audio.getReadPointer(1);
            if (!((channels > 0 && left == nullptr) || (channels > 1 && right == nullptr))) {
                const uint8_t flags = m_host->process(
                    ::rust::Slice<float>(const_cast<float*>(left), channels > 0 ? frames : 0),
                    ::rust::Slice<float>(const_cast<float*>(right), channels > 1 ? frames : 0),
                    channels, frames);
                if ((flags & 2) != 0) {
                    midi.clear();
                } else if ((flags & 1) != 0) {
                    produced = true;
                    // Collect the plugin's output MIDI into the caller's
                    // buffer. The input events are already present, so one
                    // sort at the boundary restores monotonic offsets.
                    const uint32_t outputCount = m_host->output_midi_count();
                    uint8_t payload[SandboxProtocol::kMaxMidiPayloadBytes]{};
                    for (uint32_t index = 0; index < outputCount; ++index) {
                        uint64_t sampleOffset = 0;
                        uint8_t articulationId = 0;
                        const uint32_t size = m_host->copy_output_midi(
                            index, sampleOffset, articulationId,
                            ::rust::Slice<uint8_t>(payload, sizeof(payload)));
                        if (size > 0 && sampleOffset < frames) {
                            midi.addEvent(static_cast<uint32_t>(sampleOffset), payload, size,
                                          articulationId);
                        }
                    }
                    midi.sort();
                }
            }
        }
        // Overrun streak, failure latch and MIDI invalidation are classified in
        // Rust; only the resulting flags cross back.
        const bool deadlineMissed = m_host->take_last_process_overrun();
        const bool completedLate = m_host->take_completed_sequence_overrun();
        const uint32_t outcome = m_host->observe_block(deadlineMissed, completedLate, produced);
        if (!produced) audio.clear();
        if (!produced && (outcome & kBlockClearMidi) != 0) midi.clear();
        return produced && !m_host->recovery_failed();
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const DSP::ProcessContext&) noexcept override {
        (void)processBlock(buffer, midi);
    }

    bool enqueueParameterChange(uint32_t parameterId, double value,
                                uint32_t sampleOffset = 0) noexcept {
        if (!std::isfinite(value)) return false;
        // Native plugin parameter IDs are not bounded by the size of the
        // host's small restart snapshot. CLAP IDs are arbitrary uint32 values
        // and VST3 IDs are likewise not parameter-array indices. Send every
        // valid ID through the realtime mailbox and retain up to the mailbox's
        // full distinct-ID capacity for lifecycle replay. The sparse cache is
        // keyed by native ID instead of treating the ID as an array index.
        (void)m_host->remember_parameter(parameterId, value);
        return m_host->enqueue_parameter_change(parameterId, value, sampleOffset);
    }
    // EffectChain and UI automation use the base IProcessor parameter
    // contract.  Previously a sandboxed third-party plugin inherited the
    // no-op implementation, so parameter edits appeared to succeed in the
    // host while never reaching the isolated worker. Keep the normalized
    // snapshot even when the bounded realtime queue is temporarily full;
    // lifecycle recovery replays it before audio resumes.
    void setParameter(uint32_t parameterId, float value) noexcept override {
        (void)enqueueParameterChange(parameterId, static_cast<double>(value), 0);
    }
    void setParameterAtSample(uint32_t parameterId, float value,
                              uint32_t sampleOffset) noexcept override {
        (void)enqueueParameterChange(parameterId, static_cast<double>(value), sampleOffset);
    }
    float getParameter(uint32_t parameterId) const noexcept override {
        double value = 0.0;
        return m_host->parameter_value(parameterId, value) && std::isfinite(value)
            ? static_cast<float>(std::clamp(value, 0.0, 1.0))
            : 0.0f;
    }
    void reset() noexcept override {
        // Reset is part of the realtime processor contract, so it must not
        // stop/restart the child process or take the lifecycle mutex.  The
        // Rust host clears the recovery latch, the overrun streak and the
        // quarantine edge without touching the worker's DSP state.
        m_host->reset_recovery();
    }
    uint32_t getLatencySamples() const noexcept override {
        return m_latencySamples.load(std::memory_order_acquire);
    }
    RecoveryMode recoveryMode() const noexcept {
        return m_host->recovery_failed()
            ? RecoveryMode::Quarantined : RecoveryMode::ClearBlock;
    }
    uint32_t takeInputMidiTruncations() noexcept {
        return m_host->take_input_midi_truncations();
    }
    void recordInputMidiRejection() noexcept { m_host->record_input_midi_rejection(); }
    uint32_t takeDroppedOutputMidi() noexcept { return m_host->take_dropped_output_midi(); }
    uint32_t takeMailboxOverruns() noexcept { return m_host->take_mailbox_overruns(); }
    uint8_t failureCode() const noexcept { return m_host->failure(); }
    uint8_t stateErrorCode() const noexcept { return m_host->state_error_code(); }
    const char* stateErrorText() const noexcept {
        return hirari_plugin_sandbox_state_error_name(stateErrorCode());
    }
    const char* failureText() const noexcept {
        return hirari_plugin_sandbox_failure_name(failureCode());
    }

    // State is exchanged only by the control thread.  The audio callback never
    // touches this vector, so project save/load cannot introduce RT allocation.
    std::vector<uint8_t> getState() const override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        const auto bytes = m_host->get_state();
        return std::vector<uint8_t>(bytes.begin(), bytes.end());
    }
    bool setState(const std::vector<uint8_t>& data) override {
        return restoreStateChecked(data);
    }
    bool restoreStateChecked(const std::vector<uint8_t>& data) override {
        std::lock_guard<std::mutex> lock(m_lifecycleMutex);
        const bool restored = m_host->set_state(::rust::Slice<const uint8_t>(data.data(), data.size()));
        if (!restored) m_host->latch_recovery_failure();
        return restored;
    }

private:
    ::rust::Box<::Hirari::Core::Bridge::Sandbox::SandboxHost> m_host;
    // Control-plane lifecycle serialization. processBlock() deliberately does
    // not take this mutex: audio callbacks must never wait for worker control.
    mutable std::mutex m_lifecycleMutex;
    std::atomic<uint32_t> m_latencySamples{0};
    inline static std::atomic<uint64_t> s_generationSeed{0};
};

} // namespace Hirari::Core::Plugins
