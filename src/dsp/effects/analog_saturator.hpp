#pragma once

#include <cmath>
#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstring>
#include "../math/fast_math.hpp"
#include "oversampler.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief AnalogSaturator: High-fidelity Harmonic Exciter and Soft Clipper.
 */
class AnalogSaturator : public IProcessor {
public:
    enum class Model { Tube, Tape, SoftClip };

    AnalogSaturator(double sr = 44100.0) : m_sampleRate(sr) {}

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
        if (!l || !r || numSamples == 0) return;
        drive = std::clamp(std::isfinite(drive) ? drive : 0.0f, 0.0f, 4.0f);
        warmth = std::clamp(std::isfinite(warmth) ? warmth : 0.5f, 0.0f, 1.0f);
        const float preGain = std::pow(10.0f, drive * 12.0f / 20.0f);
        const float postGain = 1.0f / std::max(1.0f, preGain * 0.35f);
        constexpr float kDcCut = 0.995f;
        for (uint32_t i = 0; i < numSamples; ++i) {
            const float inL = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inR = std::isfinite(r[i]) ? r[i] : 0.0f;
            // Non-linear stages generate harmonics above the host Nyquist
            // frequency.  The oversamplers were previously members that were
            // never used, so every saturator instance aliased at the original
            // sample rate.  Run the model at 2x and decimate through the
            // complementary all-pass branches before the DC servo.
            float upL1 = 0.0f, upL2 = 0.0f, upR1 = 0.0f, upR2 = 0.0f;
            m_oversamplerL.upsample(inL * preGain, upL1, upL2);
            m_oversamplerR.upsample(inR * preGain, upR1, upR2);
            const float shapedL1 = applyModel(upL1, warmth, model) * postGain;
            const float shapedL2 = applyModel(upL2, warmth, model) * postGain;
            const float shapedR1 = applyModel(upR1, warmth, model) * postGain;
            const float shapedR2 = applyModel(upR2, warmth, model) * postGain;
            const float wetL = m_oversamplerL.downsample(shapedL1, shapedL2);
            const float wetR = m_oversamplerR.downsample(shapedR1, shapedR2);
            m_dcL = kDcCut * m_dcL + (1.0f - kDcCut) * wetL;
            m_dcR = kDcCut * m_dcR + (1.0f - kDcCut) * wetR;
            l[i] = std::isfinite(wetL - m_dcL) ? wetL - m_dcL : 0.0f;
            r[i] = std::isfinite(wetR - m_dcR) ? wetR - m_dcR : 0.0f;
        }
    }


    void setSampleRate(double sr) { m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0; }
    void reset() noexcept override {
        m_dcL = 0.0f;
        m_dcR = 0.0f;
        m_oversamplerL.reset();
        m_oversamplerR.reset();
    }
    uint32_t getLatency() const noexcept { return 0; }

private:
    inline float applyModel(float x, float warmth, Model model) {
        switch (model) {
            case Model::Tube: {
                float b = warmth * 0.25f; // Bias
                return (x + b) / (1.0f + std::abs(x + b)) - (b / (1.0f + std::abs(b)));
            }
            case Model::Tape: {
                float xAbs = std::abs(x);
                if (xAbs < 1.0f) return x * (1.5f - 0.5f * x * x);
                return (x > 0 ? 1.0f : -1.0f);
            }
            case Model::SoftClip: return std::tanh(x);
        }
        return x;
    }

    double m_sampleRate;
    std::atomic<float> m_drive{0.2f}, m_warmth{0.5f};
    std::atomic<uint32_t> m_model{0};
    float m_dcL = 0.0f, m_dcR = 0.0f;
    Oversampler2x m_oversamplerL, m_oversamplerR;
    Math::FastMath m_math;
};

} // namespace Aura::DSP::Effects
