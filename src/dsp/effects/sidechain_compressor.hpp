#pragma once
#include <cmath>
#include <algorithm>
#include <vector>
#include <cstring>
#include <cstdio>
#include "../../core/rust_ffi.hpp"
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

/**
 * @class SidechainCompressor
 * @brief Professional High-performance Ducking Engine using the Sidechain input.
 * HONEST FIX: Uses context.sidechainBuffer to drive the gain reduction (Duck).
 */
class SidechainCompressor : public IProcessor {
public:
    SidechainCompressor()
        : m_rustRuntime(hirari_sidechain_compressor_create(44'100.0)) {}

    ~SidechainCompressor() override { hirari_sidechain_compressor_destroy(m_rustRuntime); }
    SidechainCompressor(const SidechainCompressor&) = delete;
    SidechainCompressor& operator=(const SidechainCompressor&) = delete;

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        hirari_sidechain_compressor_set_sample_rate(m_rustRuntime, sr);
    }

    uint32_t getTailSamples() const noexcept override {
        return hirari_sidechain_compressor_tail(m_rustRuntime);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        const uint32_t samples = buffer.getNumSamples();
        if (samples == 0 || buffer.getNumChannels() == 0) return;
        const float* scL = nullptr;
        const float* scR = nullptr;
        if (context.sidechainBuffer &&
            context.sidechainBuffer->getNumSamples() >= samples &&
            context.sidechainBuffer->getNumChannels() > 0) {
            scL = context.sidechainBuffer->getReadPointer(0);
            scR = context.sidechainBuffer->getNumChannels() > 1
                ? context.sidechainBuffer->getReadPointer(1) : scL;
        }
        const float* mainL = buffer.getReadPointer(0);
        const float* mainR = buffer.getNumChannels() > 1 ? buffer.getReadPointer(1) : mainL;
        float* outL = buffer.getWritePointer(0);
        float* outR = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : outL;
        if (!mainL || !mainR || !outL || !outR) return;
        const double sr = context.sampleRate;
        hirari_sidechain_compressor_process(
            m_rustRuntime, mainL, mainR, outL, outR, scL, scR,
            scL ? samples : 0, samples, sr);
    }


    void reset() noexcept override { hirari_sidechain_compressor_reset(m_rustRuntime); }

    // Parameters
    void setThreshold(float value) noexcept { hirari_sidechain_compressor_set_control(m_rustRuntime, 0, value); }
    void setRatio(float value) noexcept { hirari_sidechain_compressor_set_control(m_rustRuntime, 1, value); }
    void setAttack(float value) noexcept { hirari_sidechain_compressor_set_control(m_rustRuntime, 2, value); }
    void setRelease(float value) noexcept { hirari_sidechain_compressor_set_control(m_rustRuntime, 3, value); }

    std::string getName() const override { return "Sidechain Compressor"; }
    uint32_t getNumParameters() const noexcept override { return 4; }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_sidechain_compressor_set_parameter(m_rustRuntime, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_sidechain_compressor_get_parameter(m_rustRuntime, id);
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id > 3) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        const char* names[] = {"Threshold", "Ratio", "Attack", "Release"};
        std::snprintf(outName, maxSize, "%s", id < 4 ? names[id] : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(32, 0);
        hirari_sidechain_compressor_write_state(
            m_rustRuntime, state.data(), state.size(), isBypassed(), getMix(), getSidechainBus());
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        bool bypassed = false;
        float mix = 0.0f;
        uint32_t sidechain = 0;
        if (!hirari_sidechain_compressor_restore_state(
                m_rustRuntime, state.data(), state.size(), &bypassed, &mix, &sidechain)) return false;
        setBypassed(bypassed);
        setMix(mix);
        setSidechainBus(sidechain);
        return true;
    }

private:
    void* m_rustRuntime;
};

} // namespace Hirari::DSP::Effects
