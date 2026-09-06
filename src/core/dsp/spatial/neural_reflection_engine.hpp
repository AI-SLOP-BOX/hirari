#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <random>

namespace Aura::DSP::Spatial {

/**
 * @class NeuralReflectionEngine
 * @brief Procedural acoustic reflection engine using ray-tracing principles.
 * Simulates early reflections and diffusion for holographic soundstages.
 */
class NeuralReflectionEngine {
public:
    NeuralReflectionEngine(size_t numReflections = 32)
        : m_numReflections(std::clamp(numReflections, size_t{1}, size_t{256})) {
        m_reflectionLines.resize(numReflections, std::vector<float>(4096, 0.0f));
        m_writePos.resize(numReflections, 0);
        m_delayTimes.resize(numReflections);
        m_gains.resize(numReflections);
        
        std::mt19937 gen(42);
        std::uniform_real_distribution<float> delayDist(10.0f, 100.0f); // 10-100ms
        std::uniform_real_distribution<float> gainDist(0.1f, 0.5f);
        
        for (size_t i = 0; i < numReflections; ++i) {
            m_delayTimes[i] = (uint32_t)(delayDist(gen) * 44.1f);
            m_gains[i] = gainDist(gen);
        }
    }

    /**
     * @brief Processes reflections based on spatial coordinates.
     * @param x, y, z: Source position.
     */
    void process(float* l, float* r, uint32_t samples, float x, float y, float z) {
        if (!l || !r || samples == 0 || !std::isfinite(x) || !std::isfinite(y) || !std::isfinite(z)) return;
        const float distFactor = 1.0f / (1.0f + std::abs(std::clamp(x, -100.0f, 100.0f))
            + std::abs(std::clamp(y, -100.0f, 100.0f)) + std::abs(std::clamp(z, -100.0f, 100.0f)));
        for (uint32_t s = 0; s < samples; ++s) {
            const float inL = std::isfinite(l[s]) ? std::clamp(l[s], -4.0f, 4.0f) : 0.0f;
            const float inR = std::isfinite(r[s]) ? std::clamp(r[s], -4.0f, 4.0f) : 0.0f;
            const float in = (inL + inR) * 0.5f;
            float reflections = 0.0f;

            for (size_t i = 0; i < m_numReflections; ++i) {
                float outValue = m_reflectionLines[i][(m_writePos[i] + 4096 - m_delayTimes[i]) % 4096];
                
                m_reflectionLines[i][m_writePos[i]] = in + outValue * 0.2f; // Diffusion feedback
                reflections += outValue * m_gains[i] * distFactor;
                
                m_writePos[i] = (m_writePos[i] + 1) % 4096;
            }

            l[s] = std::isfinite(inL + reflections * 0.4f) ? std::clamp(inL + reflections * 0.4f, -4.0f, 4.0f) : 0.0f;
            r[s] = std::isfinite(inR + reflections * 0.4f) ? std::clamp(inR + reflections * 0.4f, -4.0f, 4.0f) : 0.0f;
        }
    }

    void reset() noexcept {
        for (auto& line : m_reflectionLines) std::fill(line.begin(), line.end(), 0.0f);
        std::fill(m_writePos.begin(), m_writePos.end(), 0u);
    }

private:
    size_t m_numReflections;
    std::vector<std::vector<float>> m_reflectionLines;
    std::vector<uint32_t> m_writePos;
    std::vector<uint32_t> m_delayTimes;
    std::vector<float> m_gains;
};

} // namespace Aura::DSP::Spatial
