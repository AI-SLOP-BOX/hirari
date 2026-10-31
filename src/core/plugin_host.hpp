#pragma once
#include <string>
#include <vector>
#include <cstdint>
#include "../dsp/iprocessor.hpp"

namespace Hirari::Core::Plugin {

/**
 * @class ExternalPluginHost
 * @brief Processor host for the built-in native catalog.
 *
 * External VST3/AU/CLAP binaries never travel through this class: the engine
 * builds `ProcessSandboxProcessor` directly for them. Only built-in catalog
 * paths are admitted here; parameter mirroring, dirty dispatch and the state
 * marker policy live in Rust (`native_plugin_host.rs`), so this adapter only
 * forwards the `DSP::IProcessor` virtuals.
 */
class ExternalPluginHost final : public DSP::IProcessor {
public:
    enum class Format { Internal, VST3, AU, CLAP };

    ExternalPluginHost(const std::string& path, Format format)
        : m_host(format == Format::Internal
                     ? hirari_native_plugin_host_create(
                           reinterpret_cast<const uint8_t*>(path.data()), path.size())
                     : nullptr) {}

    ~ExternalPluginHost() override { hirari_native_plugin_host_destroy(m_host); }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        hirari_native_plugin_host_prepare(m_host, sr, 0);
    }

    void process(AudioBuffer& b, MidiBuffer&, const DSP::ProcessContext& context) noexcept override {
        if (isBypassed() || !m_host) return;
        hirari_native_plugin_host_process(m_host, b.getArrayOfWritePointers(),
                                          b.getNumChannels(), b.getNumSamples(), context.bpm);
    }

    void reset() noexcept override { hirari_native_plugin_host_reset(m_host); }

    uint32_t getLatencySamples() const noexcept override {
        return hirari_native_plugin_host_latency_samples(m_host);
    }

    uint32_t getTailSamples() const noexcept override {
        return hirari_native_plugin_host_tail_samples(m_host);
    }

    uint32_t getNumParameters() const noexcept override {
        return hirari_native_plugin_host_parameter_count(m_host);
    }

    void setParameter(uint32_t id, float value) noexcept override {
        hirari_native_plugin_host_set_parameter(m_host, id, value);
    }

    float getParameter(uint32_t id) const noexcept override {
        return hirari_native_plugin_host_parameter(m_host, id);
    }

    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        const size_t size = hirari_native_plugin_host_state_save(
            m_host, m_bypassed ? 1u : 0u, m_mix, m_sidechainBusId,
            state.data(), state.size());
        // Kinds without their own serializer fall back to the shared host
        // envelope, which stays authoritative for the whole catalog.
        if (size == 0) return IProcessor::getState();
        state.resize(size);
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        return restoreKindState(state) || IProcessor::setState(state);
    }

    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        return restoreKindState(state) || IProcessor::setState(state);
    }

    bool isOperational() const noexcept { return m_host != nullptr; }

private:
    bool restoreKindState(const std::vector<uint8_t>& state) {
        uint8_t bypassed = 0;
        float mix = 1.0f;
        uint32_t sidechain = 0;
        if (!hirari_native_plugin_host_state_restore(m_host, state.data(), state.size(),
                                                     &bypassed, &mix, &sidechain)) {
            return false;
        }
        m_bypassed = bypassed != 0;
        m_mix = mix;
        m_sidechainBusId = sidechain;
        return true;
    }

    void* m_host = nullptr;
};

} // namespace Hirari::Core::Plugin
