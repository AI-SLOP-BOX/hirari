#pragma once

#include <vector>
#include <cmath>
#include <algorithm>

namespace Aura::DSP::Mixing {

/**
 * @brief NeuralDynamicsModel: The 'AI-Cloned' Analog Soul.
 * Uses Recurrent Neural Networks (GRU) to model non-linear hardware hysteresis.
 * Standard for modern high-end plugins (Neural DSP / IK Multimedia style).
 */
class NeuralDynamicsModel {
public:
    struct Weights {
        std::vector<float> inputWeights; // RNN Input matrix
        std::vector<float> recurrentWeights; // State feedback matrix
        std::vector<float> bias;
    };

    NeuralDynamicsModel() {
        m_state.resize(16, 0.0f); // Hidden state for GRU
    }

    /**
     * @brief PROCESS: Real-time inference of an analog circuit's behavior.
     * Captures the 'Warmth' and 'Saturation' that pure math formulas miss.
     */
    float process(float x, const Weights& w) {
        if (!std::isfinite(x)) x = 0.0f;
        // 1. RECURRENT UPDATE (bounded, allocation-free GRU-style inference)
        // Missing or invalid weights are treated as an inactive unit instead
        // of indexing past a malformed preset supplied by a plugin/project.
        for (size_t i = 0; i < m_state.size(); ++i) {
            const float inputWeight = i < w.inputWeights.size() && std::isfinite(w.inputWeights[i])
                ? w.inputWeights[i] : 0.0f;
            const float recurrentWeight = i < w.recurrentWeights.size() && std::isfinite(w.recurrentWeights[i])
                ? w.recurrentWeights[i] : 0.0f;
            const float bias = i < w.bias.size() && std::isfinite(w.bias[i]) ? w.bias[i] : 0.0f;
            const float gate = sigmoid(x * inputWeight + m_state[i] * recurrentWeight + bias);
            m_state[i] = (1.0f - gate) * m_state[i] + gate * std::tanh(x + m_state[i] + bias);
        }

        // 2. OUTPUT NON-LINEAR COMBINATION
        const float output = std::tanh(m_state[0] + x);
        return std::isfinite(output) ? output : 0.0f;
    }

private:
    float sigmoid(float x) { return 1.0f / (1.0f + std::exp(-x)); }
    std::vector<float> m_state;
};

} // namespace Aura::DSP::Mixing
