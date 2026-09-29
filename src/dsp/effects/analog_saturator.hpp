#pragma once

#include <cmath>
#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstring>
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Effects {

/**
 * @brief AnalogSaturator: High-fidelity Harmonic Exciter and Soft Clipper.
 */
class AnalogSaturator : public IProcessor {
public:
    enum class Model { Tube, Tape, SoftClip };

    AnalogSaturator(double sr = 44100.0) : m_state(hirari_analog_saturator_create(sr)) {}
    ~AnalogSaturator() override { hirari_analog_saturator_destroy(m_state); }
    AnalogSaturator(const AnalogSaturator&) = delete;
    AnalogSaturator& operator=(const AnalogSaturator&) = delete;

    std::string getName() const override { return "Analog Saturator"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getNumParameters() const noexcept override { return 3; }
    void setParameter(uint32_t id, float value) noexcept override {
        if (!std::isfinite(value)) return;
        if (id == 0) m_drive.store(std::clamp(value, 0.0f, 1.0f), std::memory_order_relaxed);
        else if (id == 1) m_warmth.store(std::clamp(value, 0.0f, 1.0f), std::memory_order_relaxed);
        else if (id == 2) m_model.store(static_cast<uint32_t>(std::clamp(std::lround(value), 0l, 2l)), std::memory_order_relaxed);
    }
    float getParameter(uint32_t id) const noexcept override {
        if (id == 0) return m_drive.load(std::memory_order_relaxed);
        if (id == 1) return m_warmth.load(std::memory_order_relaxed);
        if (id == 2) return static_cast<float>(m_model.load(std::memory_order_relaxed));
        return 0.0f;
    }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id >= 3) return false; out = {0.0f, id == 2 ? 2.0f : 1.0f, id == 2}; return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (outName && maxSize > 0) std::snprintf(outName, maxSize, "%s", id == 0 ? "Drive" : (id == 1 ? "Warmth" : (id == 2 ? "Model" : "")));
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(28, 0); const uint32_t magic = 0x41555241u; const uint16_t version = 1; const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2); std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_mix, 4); std::memcpy(state.data()+12, &m_sidechainBusId, 4);
        const float values[3] = {getParameter(0), getParameter(1), getParameter(2)}; std::memcpy(state.data()+16, values, sizeof(values)); return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 28) return false; uint32_t magic=0, sidechain=0; uint16_t version=0, flags=0; float mix=0.0f, values[3]{};
        std::memcpy(&magic,state.data(),4); std::memcpy(&version,state.data()+4,2); std::memcpy(&flags,state.data()+6,2); std::memcpy(&mix,state.data()+8,4); std::memcpy(&sidechain,state.data()+12,4); std::memcpy(values,state.data()+16,sizeof(values));
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f) return false;
        for (float value : values) if (!std::isfinite(value) || value < 0.0f || value > 2.0f) return false;
        m_bypassed=(flags&1u)!=0; m_mix=mix; m_sidechainBusId=sidechain; for (uint32_t i=0;i<3;++i) setParameter(i,values[i]); return true;
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        setSampleRate(sr);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        processWithSettings(left, right, buffer.getNumSamples(), m_drive.load(std::memory_order_relaxed) * 4.0f,
                            m_warmth.load(std::memory_order_relaxed), static_cast<Model>(m_model.load(std::memory_order_relaxed)));
    }

    void process(float* l, float* r, uint32_t numSamples) {
        // Default processing with neutral settings if not configured
        processWithSettings(l, r, numSamples, 0.2f, 0.5f, Model::Tube);
    }

    /**
     * @brief Processes a block of samples with specific settings.
     */
    void processWithSettings(float* l, float* r, uint32_t numSamples, float drive, float warmth, Model model = Model::Tube) {
        hirari_analog_saturator_process(
            m_state, l, r, numSamples, drive, warmth, static_cast<uint32_t>(model));
    }


    void setSampleRate(double sr) { hirari_analog_saturator_set_sample_rate(m_state, sr); }
    void reset() noexcept override { hirari_analog_saturator_reset(m_state); }
    uint32_t getLatency() const noexcept { return 0; }

private:
    void* m_state = nullptr;
    std::atomic<float> m_drive{0.2f}, m_warmth{0.5f};
    std::atomic<uint32_t> m_model{0};
};

} // namespace Hirari::DSP::Effects
