#pragma once

#include <string>
#include <vector>
#include <memory>
#include "../../dsp/iprocessor.hpp"
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../plugins/process_sandbox_processor.hpp"

// --- PROFESSIONAL CLAP SDK ABSTRACTION ---
struct clap_plugin_t;
struct clap_host_t;

struct clap_event_header_t {
    uint32_t size;
    uint32_t time;
    uint16_t space_id;
    uint16_t type;
    uint32_t flags;
};

struct clap_event_note_t {
    clap_event_header_t header;
    int32_t note_id;
    int16_t port_index;
    int16_t channel;
    int16_t key;
    double velocity;
};

struct clap_process_t {
    uint32_t frames_count;
    uint32_t steady_time; // Audio engine sample position
    float* const* audio_inputs;
    uint32_t audio_inputs_count;
    float* const* audio_outputs;
    uint32_t audio_outputs_count;
    const void* events_in;  // Pointer to event list
    void* events_out; // Pointer to event list
};

namespace Aura::Core::PluginHost {

/**
 * @class ClapHostInterface
 * @brief Next-generation open standard "CLAP (CLever Audio Plugin)" host module.
 * Integrates sample-accurate parameter modulations and MIDI-to-CLAP events.
 */
class ClapHostInterface : public ::Aura::DSP::IProcessor {
public:
    explicit ClapHostInterface(const std::string& clapPath) : m_binaryPath(clapPath) {
        if (!clapPath.empty()) {
            m_processor = std::make_unique<::Aura::Core::Plugins::ProcessSandboxProcessor>(clapPath);
        }
    }

    ~ClapHostInterface() override = default;

    // --- IProcessor Implementation ---
    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        m_blockSize = bs;
        if (m_processor) {
            m_processor->prepareToPlay(sr, bs);
            // reconfigure() intentionally leaves a never-started processor
            // stopped. The legacy CLAP adapter must still honor the normal
            // IProcessor prepare contract and bring its isolated worker up.
            if (!m_processor->isAlive()) (void)m_processor->start();
        }
    }

    void process(AudioBuffer& b, MidiBuffer& midi, const ::Aura::DSP::ProcessContext& context) noexcept override {
        (void)context;
        if (!m_processor || !m_processor->isAlive() || m_processor->processFailed()) {
            b.clear();
            midi.clear();
            return;
        }
        (void)m_processor->processBlock(b, midi);
    }

    void reset() noexcept override { if (m_processor) m_processor->reset(); }

    uint32_t getLatencySamples() const noexcept override {
        return m_processor ? m_processor->getLatencySamples() : 0;
    }

    // --- PARAMETER CONTROL ---
    void setParameter(uint32_t id, float value) noexcept override {
        if (m_processor) (void)m_processor->enqueueParameterChange(id, value, 0);
    }

    bool restoreStateChecked(const std::vector<uint8_t>& state) noexcept override {
        return m_processor && m_processor->restoreStateChecked(state);
    }

    std::vector<uint8_t> getState() const override {
        return m_processor ? m_processor->getState() : std::vector<uint8_t>{};
    }

    bool isOperational() const noexcept {
        return m_processor && m_processor->isAlive() && !m_processor->processFailed();
    }

    // Lifecycle is deliberately exposed through the compatibility adapter so
    // older callers cannot accidentally bypass the worker's serialized
    // start/stop/restart protocol.
    bool start() { return m_processor && m_processor->start(); }
    void stop() noexcept { if (m_processor) m_processor->stop(); }
    bool restart(bool force = true) { return m_processor && m_processor->restart(force); }
    bool pollHealth() noexcept { return m_processor && m_processor->pollHealth(); }
    uint8_t failureCode() const noexcept {
        return m_processor ? m_processor->failureCode() : 0;
    }
    const char* failureText() const noexcept {
        return m_processor ? m_processor->failureText() : "no-processor";
    }

    // --- CLAP GUI EXTENSION ---
    void openWindow() {
        // GUI ownership remains outside the realtime worker boundary. The
        // adapter intentionally exposes no fake window handle.
    }

private:
    std::string m_binaryPath;
    std::unique_ptr<::Aura::Core::Plugins::ProcessSandboxProcessor> m_processor;
    double m_sampleRate = 44100.0;
    uint32_t m_blockSize = 512;
};

} // namespace Aura::Core::PluginHost
