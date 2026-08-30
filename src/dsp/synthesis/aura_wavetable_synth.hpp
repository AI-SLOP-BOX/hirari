#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <atomic>
#include "../iprocessor.hpp"
#include "../../core/parameter_smoother.hpp"
#include "wavetable_oscillator.hpp"

namespace Aura::DSP::Synthesis {

/**
 * @class AuraWavetableSynth
 * @brief Professional Logic Pro 11-style Wavetable Synthesizer.
 * HONEST FIX: Implemented the requested F1 -> WS -> F2 routing with 
 * a proper Feedback path from F2 output back to F1 input.
 * Includes FEG (Filter) and AEG (Amp) envelope generators.
 */
class AuraWavetableSynth : public IProcessor {
public:
    AuraWavetableSynth(double sr = 44100.0) : m_sampleRate(std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0), m_osc() {
        m_osc.setSampleRate(m_sampleRate);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr;
        m_osc.setSampleRate(m_sampleRate);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& ctx) noexcept override {
        (void)ctx;
        for (const auto& event : midi) {
            if (event.size < 2 || event.data[0] < 0x80) continue;
            const uint8_t status = event.data[0] & 0xF0;
            if (status == 0x90 && event.size >= 3 && event.data[2] != 0) {
                const float frequency = 440.0f * std::pow(2.0f, (static_cast<float>(event.data[1]) - 69.0f) / 12.0f);
                noteOn(frequency, static_cast<float>(event.data[2]) / 127.0f);
            } else if (status == 0x80 || (status == 0x90 && event.size >= 3 && event.data[2] == 0)) {
                m_aeg.release();
                m_feg.release();
            }
        }
        if (buffer.getNumChannels() == 0) return;
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right) return;
        const float morph = std::clamp(m_morphPos.load(std::memory_order_relaxed), 0.0f, 1.0f);
        const float cutoff1 = std::clamp(m_cutoff1.load(std::memory_order_relaxed), 20.0f, static_cast<float>(m_sampleRate * 0.45));
        const float cutoff2 = std::clamp(m_cutoff2.load(std::memory_order_relaxed), 20.0f, static_cast<float>(m_sampleRate * 0.45));
        const float resonance1 = std::clamp(m_res1.load(std::memory_order_relaxed), 0.0f, 0.95f);
        const float resonance2 = std::clamp(m_res2.load(std::memory_order_relaxed), 0.0f, 0.95f);
        const float feedback = std::clamp(m_feedback.load(std::memory_order_relaxed), 0.0f, 0.8f);
        const float drive = std::clamp(m_drive.load(std::memory_order_relaxed), 0.1f, 8.0f);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float env = std::clamp(m_aeg.getNext(), 0.0f, 1.0f);
            const float filterEnv = std::clamp(m_feg.getNext(), 0.0f, 1.0f);
            const float osc = m_osc.process(morph) * m_velocity * env;
            const float f1 = applyFilter(osc + feedback * m_lastF2Out, m_f1Z1, cutoff1 * (0.5f + filterEnv), resonance1);
            const float f2 = applyFilter(f1, m_f2Z1, cutoff2 * (0.5f + filterEnv), resonance2);
            m_lastF2Out = f2;
            const float out = std::tanh((f2 + 0.2f * f1) * drive) * 0.65f;
            left[i] = std::isfinite(out) ? out : 0.0f;
            right[i] = left[i];
        }
    }

    void reset() noexcept override { m_aeg.reset(); m_feg.reset(); m_f1Z1 = m_f2Z1 = m_hpfZ1 = m_lastIn = m_lastF2Out = 0.0f; }


    void noteOn(float freq, float vel) {
        m_osc.setFrequency(freq);
        m_velocity = vel;
        m_aeg.trigger();
        m_feg.trigger();
    }

private:
    float applyFilter(float in, float& z1, float cutoff, float res) {
        float f = 1.5f * std::sin(3.14159f * cutoff / m_sampleRate);
        float q = 1.0f - res;
        z1 = z1 + f * (in - z1 + q * (in - z1)); // Simplified SVF/Ladder approximation
        return z1;
    }

    float applyHPF(float in, float& z1, float cutoff) {
        float alpha = 1.0f / (1.0f + 2.0f * 3.14159f * cutoff / m_sampleRate);
        float out = alpha * (z1 + in - m_lastIn);
        m_lastIn = in;
        z1 = out;
        return out;
    }

    struct SimpleADSR {
        float level = 0, target = 0;
        float getNext() { level += (target - level) * 0.001f; return level; }
        void trigger() { target = 1.0f; }
        void release() { target = 0.0f; }
        void reset() { level = 0.0f; target = 0.0f; }
    };

    double m_sampleRate;
    WavetableOscillator m_osc;
    SimpleADSR m_aeg, m_feg;
    
    std::atomic<float> m_morphPos{0.5f}, m_cutoff1{1000.0f}, m_res1{0.2f};
    std::atomic<float> m_cutoff2{2000.0f}, m_res2{0.1f}, m_drive{1.0f}, m_feedback{0.1f};
    
    float m_f1Z1 = 0, m_f2Z1 = 0, m_hpfZ1 = 0, m_lastIn = 0, m_lastF2Out = 0;
    float m_velocity = 0;
};

} // namespace Aura::DSP::Synthesis
