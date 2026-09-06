#pragma once

#include <cmath>
#include <vector>
#include <algorithm>
#include <atomic>
#include "../iprocessor.hpp"
#include "../../core/parameter_smoother.hpp"

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::DSP::Effects {

/**
 * @class VintagePassiveEQ
 * @brief Passive-style Equalizer modeling (Pultec EQP-1A logic) with RT-safety and smoothing.
 */
class VintagePassiveEQ : public IProcessor {
public:
    VintagePassiveEQ() {
        m_lowBoost.store(2.0f, std::memory_order_relaxed);
        m_lowAtten.store(1.0f, std::memory_order_relaxed);
        m_highBoost.store(3.0f, std::memory_order_relaxed);
        reset();
    }

    std::string getName() const override { return "Vintage Passive EQ"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        // ParameterSmoother exposes smoothing time rather than a raw sample-rate setter.
        m_smoothLowBoost.setSmoothingTime(20.0f, static_cast<float>(sr));
        m_smoothLowAtten.setSmoothingTime(20.0f, static_cast<float>(sr));
        m_smoothHighBoost.setSmoothingTime(20.0f, static_cast<float>(sr));
        
        m_smoothLowBoost.setTarget(m_lowBoost.load(std::memory_order_relaxed));
        m_smoothLowAtten.setTarget(m_lowAtten.load(std::memory_order_relaxed));
        m_smoothHighBoost.setTarget(m_highBoost.load(std::memory_order_relaxed));

        updateCoefficients(
            m_lowBoost.load(std::memory_order_relaxed),
            m_lowAtten.load(std::memory_order_relaxed),
            m_highBoost.load(std::memory_order_relaxed)
        );
    }

    /**
     * @brief Applies passive EQ curves channel-by-channel with parameter smoothing.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (m_bypassed) return;
        uint32_t channels = buffer.getNumChannels();
        uint32_t samples = buffer.getNumSamples();
        
        // 1. Get smoothed parameters for this block
        m_smoothLowBoost.setTarget(m_lowBoost.load(std::memory_order_relaxed));
        m_smoothLowAtten.setTarget(m_lowAtten.load(std::memory_order_relaxed));
        m_smoothHighBoost.setTarget(m_highBoost.load(std::memory_order_relaxed));

        float smoothLB = m_smoothLowBoost.getNextValue();
        float smoothLA = m_smoothLowAtten.getNextValue();
        float smoothHB = m_smoothHighBoost.getNextValue();

        // 2. Update coefficients on the audio thread safely
        updateCoefficients(smoothLB, smoothLA, smoothHB);

        // 3. Process signal
        for (uint32_t c = 0; c < channels && c < 2; ++c) {
            float* p = buffer.getWritePointer(c);
            if (!p) continue;
            for (uint32_t i = 0; i < samples; ++i) {
                float val = p[i];
                val = m_lowFilter[c].process(val);
                val = m_highFilter[c].process(val);
                
                // Safety guard against NaN/inf explosions
                if (std::isnan(val) || std::isinf(val)) {
                    val = 0.0f;
                    m_lowFilter[c].reset();
                    m_highFilter[c].reset();
                }
                p[i] = val;
            }
        }
    }

    void reset() noexcept override {
        for (auto& f : m_lowFilter) f.reset();
        for (auto& f : m_highFilter) f.reset();
    }

    // Parameters (UI thread writes to atomic variables without blocking)
    void setLowBoost(float b) { m_lowBoost.store(b, std::memory_order_relaxed); }
    void setLowAtten(float a) { m_lowAtten.store(a, std::memory_order_relaxed); }
    void setHighBoost(float b) { m_highBoost.store(b, std::memory_order_relaxed); }

private:
    struct FilterState {
        float x1=0, x2=0, y1=0, y2=0;
        float b0=1, b1=0, b2=0, a1=0, a2=0;
        
        float process(float in) {
            float out = b0*in + b1*x1 + b2*x2 - a1*y1 - a2*y2;
            x2=x1; x1=in; y2=y1; y1=out;
            return out;
        }
        void reset() { x1=x2=y1=y2=0; }
    };

    void updateCoefficients(float lowBoost, float lowAtten, float highBoost) {
        // Low shelf boost/cut at 60 Hz
        float lowFreq = 60.0f;
        float lowGainDb = (lowBoost * 3.0f) - (lowAtten * 2.5f); // Pultec Trick
        float lowA = std::pow(10.0f, lowGainDb / 40.0f);
        float w0_low = static_cast<float>(2.0 * M_PI * lowFreq / m_sampleRate);
        float cos_w0_low = std::cos(w0_low);
        float sin_w0_low = std::sin(w0_low);
        float beta_low = std::sqrt(lowA) * 2.0f; 
        
        float b0_low = lowA * ((lowA + 1.0f) - (lowA - 1.0f) * cos_w0_low + beta_low * sin_w0_low);
        float b1_low = 2.0f * lowA * ((lowA - 1.0f) - (lowA + 1.0f) * cos_w0_low);
        float b2_low = lowA * ((lowA + 1.0f) - (lowA - 1.0f) * cos_w0_low - beta_low * sin_w0_low);
        float a0_low = (lowA + 1.0f) + (lowA - 1.0f) * cos_w0_low + beta_low * sin_w0_low;
        float a1_low = -2.0f * ((lowA - 1.0f) + (lowA + 1.0f) * cos_w0_low);
        float a2_low = (lowA + 1.0f) + (lowA - 1.0f) * cos_w0_low - beta_low * sin_w0_low;

        for (int c = 0; c < 2; ++c) {
            m_lowFilter[c].b0 = b0_low / a0_low;
            m_lowFilter[c].b1 = b1_low / a0_low;
            m_lowFilter[c].b2 = b2_low / a0_low;
            m_lowFilter[c].a1 = a1_low / a0_low;
            m_lowFilter[c].a2 = a2_low / a0_low;
        }

        // High shelf boost at 8 kHz
        float highFreq = 8000.0f;
        float highGainDb = highBoost * 3.5f;
        float highA = std::pow(10.0f, highGainDb / 40.0f);
        float w0_high = static_cast<float>(2.0 * M_PI * highFreq / m_sampleRate);
        float cos_w0_high = std::cos(w0_high);
        float sin_w0_high = std::sin(w0_high);
        float beta_high = std::sqrt(highA) * 2.0f;

        float b0_high = highA * ((highA + 1.0f) + (highA - 1.0f) * cos_w0_high + beta_high * sin_w0_high);
        float b1_high = -2.0f * highA * ((highA - 1.0f) + (highA + 1.0f) * cos_w0_high);
        float b2_high = highA * ((highA + 1.0f) + (highA - 1.0f) * cos_w0_high - beta_high * sin_w0_high);
        float a0_high = (highA + 1.0f) - (highA - 1.0f) * cos_w0_high + beta_high * sin_w0_high;
        float a1_high = 2.0f * ((highA - 1.0f) - (highA + 1.0f) * cos_w0_high);
        float a2_high = (highA + 1.0f) - (highA - 1.0f) * cos_w0_high - beta_high * sin_w0_high;

        for (int c = 0; c < 2; ++c) {
            m_highFilter[c].b0 = b0_high / a0_high;
            m_highFilter[c].b1 = b1_high / a0_high;
            m_highFilter[c].b2 = b2_high / a0_high;
            m_highFilter[c].a1 = a1_high / a0_high;
            m_highFilter[c].a2 = a2_high / a0_high;
        }
    }

    double m_sampleRate = 44100.0;
    std::atomic<float> m_lowBoost;
    std::atomic<float> m_lowAtten;
    std::atomic<float> m_highBoost;

    // Thread-safe parameter smoothers
    Core::ParameterSmoother m_smoothLowBoost;
    Core::ParameterSmoother m_smoothLowAtten;
    Core::ParameterSmoother m_smoothHighBoost;

    FilterState m_lowFilter[2];
    FilterState m_highFilter[2];
};

} // namespace Aura::DSP::Effects
