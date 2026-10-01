#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <functional>
#include <memory>
#include <vector>
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
        : m_processSize(processSize == 0 ? 1u : processSize),
          m_hopSize(hopSize == 0 || hopSize > processSize ? processSize : hopSize) {
        m_inputBuffer.assign(m_processSize, 0.0f);
        m_outputBuffer.assign(m_processSize, 0.0f);
        m_workspace.assign(m_processSize, 0.0f);
        m_windowedWorkspace.assign(m_processSize, 0.0f);

        // Periodic Hann. A symmetric Hann (denominator processSize - 1) does not
        // satisfy COLA at any integer hop, so the periodic form is required for
        // the analysis/synthesis pair below to sum to a constant.
        m_window.assign(m_processSize, 0.0f);
        for (uint32_t i = 0; i < m_processSize; ++i) {
            m_window[i] = 0.5f * (1.0f - std::cos(2.0f * static_cast<float>(M_PI) *
                                                 static_cast<float>(i) /
                                                 static_cast<float>(m_processSize)));
        }

        // Analysis and synthesis apply the same window, so the overlap-add sum
        // gains w^2. Normalise by the steady-state sum of w^2 over the frames
        // that overlap a given sample, which restores unity gain for any
        // integer hop instead of a hop-dependent error.
        m_colaGain = 1.0f;
        const uint32_t overlap = m_processSize / m_hopSize;
        if (overlap > 1) {
            // Each output sample receives a contribution from every frame that
            // overlaps it, each weighted by w^2. Averaged over a period the sum
            // is overlap * mean(w^2) (every shifted window has the same sum of
            // squares). For a periodic Hann this is 0.75 at 50% overlap, which
            // is exactly the -2.5 dB loss that went uncorrected before.
            double sumSquares = 0.0;
            for (float w : m_window) {
                sumSquares += static_cast<double>(w) * w;
            }
            const double steadyState =
                (sumSquares / static_cast<double>(m_processSize)) * static_cast<double>(overlap);
            if (steadyState > 1e-9) {
                m_colaGain = static_cast<float>(1.0 / steadyState);
            }
        }

        m_outputQueue.assign(m_hopSize, 0.0f);
        m_writePos = 0;
        m_outputPos = 0;
        m_hasOutput = false;
    }

    /**
     * @brief PUMP: Feeds samples into the STFT stream and replaces them with
     * the framed output.
     *
     * RT-safety: every buffer is preallocated in the constructor and the
     * callback is invoked as a const lvalue reference, so no allocation occurs
     * on the audio thread. The spectral callback must not allocate either.
     */
    void process(float* data, uint32_t numSamples,
                 const std::function<void(const std::vector<float>&, std::vector<float>&)>& callback) {
        if (data == nullptr || !callback) {
            return;
        }
        for (uint32_t i = 0; i < numSamples; ++i) {
            m_inputBuffer[m_writePos++] = data[i];

            if (m_writePos >= m_processSize) {
                // 1. APPLY WINDOW (RT-Safe)
                for (uint32_t j = 0; j < m_processSize; ++j) {
                    m_windowedWorkspace[j] = m_inputBuffer[j] * m_window[j];
                }

                std::fill(m_workspace.begin(), m_workspace.end(), 0.0f);
                callback(m_windowedWorkspace, m_workspace);

                // 2. OVERLAP-ADD WITH WINDOW AND COLA NORMALISATION
                for (uint32_t j = 0; j < m_processSize; ++j) {
                    m_outputBuffer[j] += m_workspace[j] * m_window[j] * m_colaGain;
                }

                // The first hop of the accumulator is now complete: every frame
                // that overlaps those samples has been added. Drain exactly one
                // hop per frame, then shift. Reading through a cursor that is
                // advanced independently of the frame cadence is what consumed
                // each frame's tail before it was fully accumulated.
                for (uint32_t k = 0; k < m_hopSize; ++k) {
                    m_outputQueue[k] = m_outputBuffer[k];
                }
                m_outputPos = 0;
                m_hasOutput = true;

                // SHIFT the overlap-add accumulator by one hop. Without this the
                // accumulator keeps growing and every later frame is summed on
                // top of the ones before it, which is what made the previous
                // implementation's output scale with the number of frames.
                std::copy(m_outputBuffer.begin() + m_hopSize, m_outputBuffer.end(), m_outputBuffer.begin());
                std::fill(m_outputBuffer.begin() + (m_processSize - m_hopSize), m_outputBuffer.end(), 0.0f);

                // SHIFT the analysis input by one hop.
                std::copy(m_inputBuffer.begin() + m_hopSize, m_inputBuffer.end(), m_inputBuffer.begin());
                std::fill(m_inputBuffer.begin() + (m_processSize - m_hopSize), m_inputBuffer.end(), 0.0f);
                m_writePos -= m_hopSize;
            }

            if (m_hasOutput) {
                data[i] = m_outputQueue[m_outputPos++];
            } else {
                // Priming: the first complete frame has not been produced yet.
                data[i] = 0.0f;
            }
        }
    }

    /**
     * @brief LATENCY REPORT: Crucial for PDC (Plugin Delay Compensation).
     *
     * The first output sample is emitted once one full frame of input has been
     * collected and one hop has been drained from the accumulator, so the real
     * algorithmic delay is processSize - hopSize. Reporting processSize
     * over-stated the delay by one hop and misaligned the whole mix against
     * every plugin with genuine latency.
     */
    uint32_t getLatencySamples() const { return m_processSize - m_hopSize; }

private:
    uint32_t m_processSize, m_hopSize;
    float m_colaGain = 1.0f;
    std::vector<float> m_inputBuffer, m_outputBuffer, m_workspace, m_window, m_windowedWorkspace;
    std::vector<float> m_outputQueue;
    uint32_t m_writePos, m_outputPos;
    bool m_hasOutput;
};

} // namespace Hirari::DSP
