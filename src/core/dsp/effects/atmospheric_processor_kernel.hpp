#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include "../../composition/harmonic_context_tracker.hpp"
#include "../../mixing/aesthetic_evaluator_kernel.hpp"

namespace Aura::Core::DSP::Effects {

/**
 * @class AtmosphericProcessorKernel
 * @brief Industrial Singularity Engine for Evolving Soundscapes.
 * Implements autonomous parameter morphing and pulsed spatial sovereignty.
 */
class AtmosphericProcessorKernel {
public:
    AtmosphericProcessorKernel(size_t numDelays = 4) : m_numDelays(numDelays) {
        m_delayLines.resize(numDelays, std::vector<float>(32768, 0.0f));
        m_writePos.resize(numDelays, 0);
        m_delayTimes.resize(numDelays, 0.0f);
        m_delayTimes = { 1531.0f, 2307.0f, 3109.0f, 4409.0f };
    }

    /**
     * @brief Processes with SOVEREIGN AESTHETIC ORCHESTRATION.
     */
    void process(float* buffer, uint32_t sz, float decay = 0.5f) {
        const auto& harmony = Composition::HarmonicContextTracker::getInstance().getState();
        const auto& aesthetic = Mixing::AestheticEvaluatorKernel::getInstance().analyze(buffer, buffer, sz);
        
        // --- PHASE 69: AUTONOMOUS PARAMETER MORPHING ---
        // Spatial width and decay are modulated by Grandeur and Tension.
        float width = 0.5f + (aesthetic.transientClarity * 0.5f);
        float adaptiveDecay = decay * (1.0f + harmony.tension * 0.2f);

        for (uint32_t i = 0; i < sz; ++i) {
            float in = buffer[i];
            float feedback[4] = {0,0,0,0};
            
            // --- PHASE 69: PULSED DELAY SOVEREIGNTY ---
            // Delay times exhibit "organic breathing" entrained with the clock.
            float modulation = std::sin(i * 0.0001f) * (harmony.tension * 10.0f);

            for (size_t d = 0; d < m_numDelays; ++d) {
                float pos = static_cast<float>(m_writePos[d]) - (m_delayTimes[d] + modulation);
                if (pos < 0) pos += m_delayLines[d].size();
                feedback[d] = readCubic(m_delayLines[d], pos);
            }
            
            float sum = feedback[0] + feedback[1] + feedback[2] + feedback[3];
            for (size_t d = 0; d < m_numDelays; ++d) {
                float outSample = feedback[d] - 0.5f * sum;
                m_delayLines[d][m_writePos[d]] = in + outSample * adaptiveDecay;
                m_writePos[d] = (m_writePos[d] + 1) % m_delayLines[d].size();
            }
            
            buffer[i] = in * (1.0f - width) + (sum / m_numDelays) * width;
        }
    }

private:
    float readCubic(const std::vector<float>& line, float pos) const {
        int i0 = static_cast<int>(std::floor(pos));
        float fr = pos - i0;
        int i_1 = (i0 - 1 + line.size()) % line.size();
        int i1 = (i0 + 1) % line.size();
        int i2 = (i0 + 2) % line.size();
        float y_1 = line[i_1], y0 = line[i0], y1 = line[i1], y2 = line[i2];
        float a0 = -0.5f * y_1 + 1.5f * y0 - 1.5f * y1 + 0.5f * y2;
        float a1 = y_1 - 2.5f * y0 + 2.0f * y1 - 0.5f * y2;
        float a2 = -0.5f * y_1 + 0.5f * y1;
        float a3 = y0;
        return a0 * fr * fr * fr + a1 * fr * fr + a2 * fr + a3;
    }

    size_t m_numDelays;
    std::vector<std::vector<float>> m_delayLines;
    std::vector<size_t> m_writePos;
    std::vector<float> m_delayTimes;
};

} // namespace Aura::Core::DSP::Effects
