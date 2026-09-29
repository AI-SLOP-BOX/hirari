#pragma once

#include <cmath>
#include <atomic>

namespace Hirari::Core::DSP::Mixing {

/**
 * @brief BitcrusherClassic: Iconic Logic Pro-style lo-fi processor.
 * Features downsampling and bit-depth reduction for creative character.
 */
class BitcrusherClassic {
public:
    void process(float* l, float* r, size_t numFrames) {
        float bitDivider = std::pow(2.0f, m_bitDepth.load() - 1);
        float downsampleFactor = m_downsample.load();
        float drive = std::pow(10.0f, m_driveDB.load() / 20.0f);

        for (size_t i = 0; i < numFrames; ++i) {
            // DOWN SAMPLING: Only update samples every N-th frame
            if (m_sampleCounter >= downsampleFactor) {
                // Apply DRIVE and BIT REDUCTION
                m_lastSampleL = std::round(std::tanh(l[i] * drive) * bitDivider) / bitDivider;
                m_lastSampleR = std::round(std::tanh(r[i] * drive) * bitDivider) / bitDivider;
                m_sampleCounter = 0;
            }
            l[i] = m_lastSampleL;
            r[i] = m_lastSampleR;
            m_sampleCounter += 1.0f;
        }
    }

    void setBitDepth(float bits) { m_bitDepth.store(bits); }
    void setDownsample(float factor) { m_downsample.store(factor); }
    void setDrive(float db) { m_driveDB.store(db); }

private:
    std::atomic<float> m_bitDepth{16.0f}, m_downsample{1.0f}, m_driveDB{0.0f};
    float m_sampleCounter = 0;
    float m_lastSampleL = 0, m_lastSampleR = 0;
};

} // namespace Hirari::Core::DSP::Mixing
