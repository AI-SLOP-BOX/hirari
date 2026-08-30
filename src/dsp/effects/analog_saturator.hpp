#pragma once

#include <cmath>
#include <algorithm>
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

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        setSampleRate(sr);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext& context) noexcept override {
        (void)midi;
        (void)context;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        processWithSettings(left, right, buffer.getNumSamples(), 0.2f, 0.5f, Model::Tube);
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
    float m_dcL = 0.0f, m_dcR = 0.0f;
    Oversampler2x m_oversamplerL, m_oversamplerR;
    Math::FastMath m_math;
};

} // namespace Aura::DSP::Effects
