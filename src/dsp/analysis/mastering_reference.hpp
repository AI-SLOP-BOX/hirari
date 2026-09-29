#pragma once

#include <vector>
#include <memory>
#include <string>
#include <algorithm>
#include "loudness_meter.hpp"

namespace Hirari::DSP::Analysis {

/**
 * @brief MasteringReference: Side-by-side comparison with professional tracks.
 * Essential for the final 'Mastering' stage to ensure tonal balance.
 */
class MasteringReference {
public:
    MasteringReference(double sr = 44100.0) : m_sampleRate(sr), m_meter(sr) {}

    /**
     * @brief Loads a reference audio file and analyzes its metadata.
     */
    void loadReference(std::shared_ptr<std::vector<std::vector<float>>> data) {
        m_refData = data;
        m_refLoudness = analyzeLoudness(*data);
    }

    /**
     * @brief LOUDNESS MATCHING (Pro Level): Returns the gain needed to match the reference.
     * Prevents the 'louder is better' bias by aligning LUFS levels.
     */
    float getMatchGain(float currentLUFS) const {
        float diff = m_refLoudness - currentLUFS;
        return std::pow(10.0f, diff / 20.0f);
    }

    /**
     * @brief Gets the reference sample at a given position.
     */
    float getSample(uint32_t chan, uint64_t pos) const {
        if (!m_refData || chan >= m_refData->size()) return 0.0f;
        const auto& cData = (*m_refData)[chan];
        if (pos >= cData.size()) return 0.0f;
        return cData[pos];
    }

private:
    float analyzeLoudness(const std::vector<std::vector<float>>& data) {
        m_meter.reset();
        uint32_t step = 1024;
        for (uint64_t i = 0; i < data[0].size(); i += step) {
            uint32_t len = std::min(step, static_cast<uint32_t>(data[0].size() - i));
            m_meter.process(data[0].data() + i, data[1].data() + i, len);
        }
        return m_meter.getMetrics().integratedLUFS;
    }

    double m_sampleRate;
    std::shared_ptr<std::vector<std::vector<float>>> m_refData;
    float m_refLoudness = -14.0f;
    LoudnessMeter m_meter;
};

} // namespace Hirari::DSP::Analysis
