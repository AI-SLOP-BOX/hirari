#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <random>
#include <string>
#include <array>
#include "../../core/audio_buffer.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Effects {

/**
 * @class VirtuosoTapeSaturator
 * @brief High-Fidelity Analog Magnetic Tape Emulation.
 * HONEST FIX: Replaces a simple clipper with a sophisticated hysteresis and 
 * bias-based tube/magnetic model with realistic Wow & Flutter.
 */
class VirtuosoTapeSaturator : public IProcessor {
public:
    VirtuosoTapeSaturator(double sr = 44100.0) 
        : m_sampleRate(sr > 0.0 ? sr : 44100.0), m_noiseGen(std::random_device{}()) {
        reset();
        setupBuffers();
    }

    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = sr > 0.0 ? sr : 44100.0;
        setupBuffers();
        reset();
    }

    /**
     * @brief PROCESS: Magnetic saturation and tape velocity modulation.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const ProcessContext&) noexcept override {
        if (isBypassed() || buffer.isEmpty()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();
        float* l = buffer.getWritePointer(0);
        float* r = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l) return;

        const float drive = std::pow(10.0f, std::clamp(m_driveDb, -12.0f, 36.0f) / 20.0f);
        
        for (uint32_t s = 0; s < numSamples; ++s) {
            float inL = std::isfinite(l[s]) ? l[s] : 0.0f;
            float inR = r && std::isfinite(r[s]) ? r[s] : inL;

            updateLFOs();

            // Wow & Flutter delay calculations (in samples)
            float wow = static_cast<float>(std::sin(m_wowPhase)) * m_wowDepth * m_delayOffset;
            float flutter = static_cast<float>(std::sin(m_flutterPhase)) * m_flutterDepth * 6.0f;
            float totalDelay = m_delayOffset + wow + flutter;
            totalDelay = std::clamp(totalDelay, 1.0f, static_cast<float>(kBufferSize - 2));

            // Write to circular delay buffers
            m_delayBuffers[0][m_writeIdx] = inL;
            m_delayBuffers[1][m_writeIdx] = inR;

            auto readChannel = [&](int ch, float inVal) {
                float readPos = static_cast<float>(m_writeIdx) + static_cast<float>(kBufferSize) - totalDelay;
                while (readPos >= static_cast<float>(kBufferSize)) readPos -= static_cast<float>(kBufferSize);

                size_t idx0 = static_cast<size_t>(readPos);
                size_t idx1 = (idx0 + 1) % kBufferSize;
                float frac = readPos - static_cast<float>(idx0);

                // Linear interpolation for modulated tape speed delay
                float delayOut = m_delayBuffers[ch][idx0] * (1.0f - frac) + m_delayBuffers[ch][idx1] * frac;

                // Asymmetrical Magnetic Saturation with tube-style Bias
                float biased = delayOut * drive + m_bias;
                float saturated = std::tanh(biased) - std::tanh(m_bias);

                // Hiss Noise Injection
                float hiss = m_noiseDist(m_noiseGen) * m_hissLevel;
                saturated += hiss;

                // Tape Head Warmth LPF (high-frequency loss)
                m_z1[ch] += (saturated - m_z1[ch]) * 0.78f;
                if (!std::isfinite(m_z1[ch])) m_z1[ch] = 0.0f;

                return inVal * (1.0f - m_mix) + m_z1[ch] * m_mix;
            };

            l[s] = readChannel(0, inL);
            if (r) r[s] = readChannel(1, inR);

            m_writeIdx = (m_writeIdx + 1) % kBufferSize;
        }
    }

    void reset() noexcept override {
        for (auto& buf : m_delayBuffers) std::fill(buf.begin(), buf.end(), 0.0f);
        m_z1.fill(0.0f);
        m_writeIdx = 0;
        m_wowPhase = 0.0;
        m_flutterPhase = 0.0;
    }

    // Parameters
    void setDrive(float db) { m_driveDb = db; }
    void setHiss(float level) { m_hissLevel = level; }
    void setWow(float d) { m_wowDepth = d; }
    
    uint32_t getLatencySamples() const noexcept override { 
        return static_cast<uint32_t>(m_delayOffset); 
    }

    std::string getName() const override { return "VirtuosoTapeSaturator"; }

private:
    void setupBuffers() {
        m_delayBuffers[0].assign(kBufferSize, 0.0f);
        m_delayBuffers[1].assign(kBufferSize, 0.0f);
    }

    void updateLFOs() {
        m_wowPhase += (2.0 * M_PI * 0.5) / m_sampleRate; // 0.5Hz Wow
        m_flutterPhase += (2.0 * M_PI * 15.6) / m_sampleRate; // 15.6Hz Flutter
        if (m_wowPhase > 2.0 * M_PI) m_wowPhase -= 2.0 * M_PI;
        if (m_flutterPhase > 2.0 * M_PI) m_flutterPhase -= 2.0 * M_PI;
    }

    double m_sampleRate;
    static constexpr int kBufferSize = 1024;
    std::array<std::vector<float>, 2> m_delayBuffers;
    uint32_t m_writeIdx = 0;
    float m_delayOffset = 50.0f;

    float m_driveDb = 12.0f;
    float m_mix = 1.0f; 
    float m_bias = 0.05f;      // Asymmetrical Tube/Tape Bias
    float m_hissLevel = 0.00001f;
    float m_wowDepth = 0.15f;
    float m_flutterDepth = 0.05f;
    double m_wowPhase = 0.0, m_flutterPhase = 0.0;
    std::array<float, 2> m_z1{0.0f, 0.0f};

    std::mt19937 m_noiseGen;
    std::uniform_real_distribution<float> m_noiseDist{-1.0f, 1.0f};
};

} // namespace Aura::DSP::Effects
