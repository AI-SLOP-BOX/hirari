#pragma once
#include <vector>
#include <cmath>
#include <algorithm>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

namespace Aura::Synthesis {

/**
 * @class SurgeWavetable
 * @brief Contiguous precalculated 3D wavetable memory manager.
 * Builds the Sine -> Triangle -> Sawtooth -> Square morphed frames once on startup.
 */
class SurgeWavetable {
public:
    static constexpr int kNumFrames = 64;
    static constexpr int kTableLength = 2048;

    static const float* getTable() {
        static SurgeWavetable instance;
        return instance.m_data.data();
    }

private:
    std::vector<float> m_data;

    SurgeWavetable() {
        m_data.resize(kNumFrames * kTableLength);
        for (int frame = 0; frame < kNumFrames; ++frame) {
            float morphFrac = static_cast<float>(frame) / static_cast<float>(kNumFrames - 1);
            for (int sample = 0; sample < kTableLength; ++sample) {
                float sampleFrac = static_cast<float>(sample) / static_cast<float>(kTableLength);
                
                float sineVal = std::sin(2.0f * M_PI * sampleFrac);
                float triVal = 1.0f - 4.0f * std::abs(std::round(sampleFrac - 0.25f) - (sampleFrac - 0.25f));
                float sawVal = 2.0f * (sampleFrac - std::floor(sampleFrac + 0.5f));
                float sqVal = (sampleFrac < 0.5f) ? 1.0f : -1.0f;
                
                float outVal = 0.0f;
                if (morphFrac < 0.333f) {
                    float blend = morphFrac / 0.333f;
                    outVal = sineVal + blend * (triVal - sineVal);
                } else if (morphFrac < 0.666f) {
                    float blend = (morphFrac - 0.333f) / 0.333f;
                    outVal = triVal + blend * (sawVal - triVal);
                } else {
                    float blend = (morphFrac - 0.666f) / 0.334f;
                    outVal = sawVal + blend * (sqVal - sawVal);
                }
                m_data[frame * kTableLength + sample] = outVal;
            }
        }
    }
};

/**
 * @class SurgeWavetableOscillator
 * @brief 3D Morphing Wavetable Synthesizer Oscillator.
 * Reads precalculated contiguous frames using 4-point Catmull-Rom spline interpolation.
 * Instantiation is 100% lock-free, RT-safe, and cache-friendly.
 */
class SurgeWavetableOscillator {
public:
    SurgeWavetableOscillator(double sr = 44100.0) 
        : m_sampleRate(sr)
        , m_phase(0.0)
        , m_wavetableData(SurgeWavetable::getTable()) {
    }

    void setFrequency(double freq) {
        m_phaseIncrement = freq / m_sampleRate;
    }

    void setMorphPosition(float pos) {
        m_morphPos = std::clamp(pos, 0.0f, 1.0f);
    }

    float process() {
        float numTables = static_cast<float>(m_numFrames - 1);
        float tableZ = m_morphPos * numTables;
        int tableA = static_cast<int>(tableZ);
        int tableB = std::min(tableA + 1, m_numFrames - 1);
        float blendFrac = tableZ - static_cast<float>(tableA);

        float posInSamples = static_cast<float>(m_phase * m_tableLength);

        float valA = getInterpolatedSample(tableA, posInSamples);
        float valB = getInterpolatedSample(tableB, posInSamples);

        float out = valA + blendFrac * (valB - valA);

        m_phase += m_phaseIncrement;
        if (m_phase >= 1.0) m_phase -= 1.0;

        return out;
    }

private:
    double m_sampleRate;
    double m_phase;
    double m_phaseIncrement = 0.0;
    float m_morphPos = 0.0f;
    
    static constexpr int m_numFrames = SurgeWavetable::kNumFrames;
    static constexpr int m_tableLength = SurgeWavetable::kTableLength;
    const float* m_wavetableData;

    float getInterpolatedSample(int tableIdx, float pos) {
        int p1 = static_cast<int>(pos);
        float frac = pos - p1;

        int p0 = (p1 - 1 + m_tableLength) % m_tableLength;
        int p2 = (p1 + 1) % m_tableLength;
        int p3 = (p1 + 2) % m_tableLength;

        float s0 = m_wavetableData[tableIdx * m_tableLength + p0];
        float s1 = m_wavetableData[tableIdx * m_tableLength + p1];
        float s2 = m_wavetableData[tableIdx * m_tableLength + p2];
        float s3 = m_wavetableData[tableIdx * m_tableLength + p3];

        float a0 = -0.5f * s0 + 1.5f * s1 - 1.5f * s2 + 0.5f * s3;
        float a1 = s0 - 2.5f * s1 + 2.0f * s2 - 0.5f * s3;
        float a2 = -0.5f * s0 + 0.5f * s2;
        float a3 = s1;

        return a0 * (frac * frac * frac) + a1 * (frac * frac) + a2 * frac + a3;
    }
};

} // namespace Aura::Synthesis
