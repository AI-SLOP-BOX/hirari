#pragma once
#include <stdint.h>
#include <stddef.h>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <string>
#include <vector>
#include "hirari-core-bridge/src/plugin_sandbox_host.rs.h"
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "plugin_sandbox_protocol.hpp"

namespace Hirari::Core::Plugins {

/**
 * @class PluginSandboxHost
 * @brief Thin C++ facade over the Rust-owned plugin process supervisor.
 *
 * The process lifecycle, IPC framing, shared-memory mailbox, timeouts and
 * restart policy live in `hirari-core-bridge/src/plugin_sandbox_host.rs`. This
 * header only preserves the C++ type and its engine-facing signatures; the
 * worker process remains the sole component that loads VST3/AU/CLAP binaries.
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
        : m_host(::Hirari::Core::Bridge::Sandbox::new_sandbox_host(
              ::rust::Str(pluginPath), sampleRate, maxBlockSize, channels,
              ::rust::Str(requestedFormat))) {}

    ~PluginSandboxHost() { stop(); }

    PluginSandboxHost(const PluginSandboxHost&) = delete;
    PluginSandboxHost& operator=(const PluginSandboxHost&) = delete;

    bool start() { return m_host->start(); }
    void stop() noexcept { m_host->stop(); }
    bool requestPluginReset() noexcept { return m_host->request_plugin_reset(); }
    bool isAlive() const noexcept { return m_host->is_alive(); }
    bool pollHealth() noexcept { return m_host->poll_health(); }
    bool restart(bool force = true) { return m_host->restart(force); }
    bool autoRestart() { return m_host->auto_restart(); }
    bool reconfigure(double sampleRate, uint32_t maxBlockSize) {
        return m_host->reconfigure(sampleRate, maxBlockSize);
    }
    Failure failure() const noexcept {
        return static_cast<Failure>(m_host->failure());
    }
    bool isQuarantined() const noexcept { return m_host->is_quarantined(); }
    uint32_t restartAttempts() const noexcept { return m_host->restart_attempts(); }
    void clearQuarantine() noexcept { m_host->clear_quarantine(); }
    bool canRetry() const noexcept { return m_host->can_retry(); }
    bool canAutoRetry() const noexcept { return m_host->can_auto_retry(); }

    bool setGenerationContext(GenerationContext context) noexcept {
        if (!context.valid()) return false;
        return m_host->set_generation_context(context.project, context.plugin,
                                              context.audio, context.state);
    }
    GenerationContext generationContext() const noexcept {
        const auto context = m_host->generation_context();
        return {context.project, context.plugin, context.audio, context.state};
    }

    bool enqueueExtendedMidi(uint64_t sampleOffset, uint8_t articulationId,
                             const uint8_t* bytes, size_t size) noexcept {
        if (bytes == nullptr) size = 0;
        return m_host->enqueue_extended_midi(
            sampleOffset, articulationId,
            ::rust::Slice<const uint8_t>(bytes, size));
    }

    bool enqueueParameterChange(uint32_t parameterId, double value,
                                uint32_t sampleOffset = 0) noexcept {
        return m_host->enqueue_parameter_change(parameterId, value, sampleOffset);
    }

    bool process(AudioBuffer& audio, MidiBuffer& midi) noexcept {
        if (m_host->is_alive()) {
            const uint32_t channels = audio.getNumChannels();
            const uint32_t frames = audio.getNumSamples();
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
            if ((channels > 0 && left == nullptr) || (channels > 1 && right == nullptr)) {
                audio.clear();
                midi.clear();
                return false;
            }
            const uint8_t flags = m_host->process(
                ::rust::Slice<float>(const_cast<float*>(left), channels > 0 ? frames : 0),
                ::rust::Slice<float>(const_cast<float*>(right), channels > 1 ? frames : 0),
                channels, frames);
            if ((flags & 2) != 0) {
                audio.clear();
                midi.clear();
                return false;
            }
            if ((flags & 1) == 0) {
                audio.clear();
                return false;
            }
            // Collect the plugin's output MIDI into the caller's buffer. The
            // input events are already present, so one sort at the boundary
            // restores monotonic sample offsets.
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
            return true;
        }
        audio.clear();
        midi.clear();
        return false;
    }

    uint64_t getHeartbeat() const { return m_host->heartbeat(); }
    uint32_t takeDroppedOutputMidi() noexcept { return m_host->take_dropped_output_midi(); }
    uint32_t takeMailboxOverruns() noexcept { return m_host->take_mailbox_overruns(); }
    uint32_t takeInputMidiTruncations() noexcept { return m_host->take_input_midi_truncations(); }
    void recordInputMidiRejection() noexcept { m_host->record_input_midi_rejection(); }
    bool takeLastProcessOverrun() noexcept { return m_host->take_last_process_overrun(); }
    bool takeCompletedSequenceOverrun() noexcept {
        return m_host->take_completed_sequence_overrun();
    }
    uint32_t activeSampleRate() const noexcept { return m_host->active_sample_rate(); }
    uint32_t activeChannels() const noexcept { return m_host->active_channels(); }
    bool isMidiInstrument() const noexcept { return m_host->is_midi_instrument(); }
    uint32_t takeProcessErrors() noexcept { return m_host->take_process_errors(); }

    bool setState(const uint8_t* data, size_t size) noexcept {
        if (data == nullptr && size != 0) return false;
        return m_host->set_state(::rust::Slice<const uint8_t>(data, size));
    }
    std::vector<uint8_t> getState() const {
        const auto bytes = m_host->get_state();
        return std::vector<uint8_t>(bytes.begin(), bytes.end());
    }
    uint8_t stateErrorCode() const noexcept { return m_host->state_error_code(); }
    const char* stateErrorText() const noexcept {
        return hirari_plugin_sandbox_state_error_name(stateErrorCode());
    }

private:
    ::rust::Box<::Hirari::Core::Bridge::Sandbox::SandboxHost> m_host;
};

} // namespace Hirari::Core::Plugins
