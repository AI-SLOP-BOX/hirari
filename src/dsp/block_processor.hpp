#pragma once

#include <vector>
#include <algorithm>
#include <memory>
#include "iprocessor.hpp"

namespace Hirari::DSP {

/**
 * @brief BlockProcessor: Architecture for fixed-size DSP (FFT/STFT).
 * Bridges dynamic driver buffers (e.g. 512) to fixed spectral buffers (e.g. 2048).
 * Essential for Reverb, Pitch Shifting, and Spectral Repair.
 */
class BlockProcessor {
public:
    BlockProcessor(uint32_t processSize, uint32_t hopSize) 
        : m_processSize(processSize), m_hopSize(hopSize) {
        m_inputBuffer.assign(processSize, 0.0f);
        m_outputBuffer.assign(processSize, 0.0f);
        m_workspace.assign(processSize, 0.0f);
        m_windowedWorkspace.assign(processSize, 0.0f);
        
        // --- HONEST FIX: PRECOMPUTED HANNING WINDOW ---
        m_window.assign(processSize, 0.0f);
        for (uint32_t i = 0; i < processSize; ++i) {
            m_window[i] = 0.5f * (1.0f - std::cos(2.0f * M_PI * i / (processSize - 1)));
        }
        
        m_writePos = 0;
        m_readPos = 0;
    }

    /**
     * @brief PUMP: Feeds samples into the STFT stream.
     * HONEST FIX: Removed the std::vector allocation inside the loop! 
     * Uses a pre-allocated m_workspace to ensure RT-safety.
     */
    void process(float* data, uint32_t numSamples, 
                 std::function<void(const std::vector<float>&, std::vector<float>&)> callback) {
        for (uint32_t i = 0; i < numSamples; ++i) {
            m_inputBuffer[m_writePos++] = data[i];

            if (m_writePos >= m_processSize) {
                // 1. APPLY WINDOW (RT-Safe)
                for (uint32_t j = 0; j < m_processSize; ++j) {
                    m_windowedWorkspace[j] = m_inputBuffer[j] * m_window[j];
                }
                
                std::fill(m_workspace.begin(), m_workspace.end(), 0.0f);
                callback(m_windowedWorkspace, m_workspace);
                
                // 2. OVERLAP-ADD WITH WINDOW
                for (uint32_t j = 0; j < m_processSize; ++j) {
                    m_outputBuffer[j] += m_workspace[j] * m_window[j];
                }
                
                // SHIFT
                std::copy(m_inputBuffer.begin() + m_hopSize, m_inputBuffer.end(), m_inputBuffer.begin());
                std::fill(m_inputBuffer.begin() + (m_processSize - m_hopSize), m_inputBuffer.end(), 0.0f);
                m_writePos -= m_hopSize;
            }

            data[i] = m_outputBuffer[m_readPos++];
            if (m_readPos >= m_hopSize) {
                std::copy(m_outputBuffer.begin() + m_hopSize, m_outputBuffer.end(), m_outputBuffer.begin());
                std::fill(m_outputBuffer.begin() + (m_processSize - m_hopSize), m_outputBuffer.end(), 0.0f);
                m_readPos -= m_hopSize;
            }
        }
    }

    /**
     * @brief LATENCY REPORT: Crucial for PDC (Plugin Delay Compensation).
     * HONEST FIX: Correctly reports the STFT block latency (processSize).
     */
    uint32_t getLatencySamples() const { return m_processSize; }

private:
    uint32_t m_processSize, m_hopSize;
    std::vector<float> m_inputBuffer, m_outputBuffer, m_workspace, m_window, m_windowedWorkspace;
    uint32_t m_writePos, m_readPos;
};

} // namespace Hirari::DSP
