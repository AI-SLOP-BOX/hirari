#pragma once
#include <vector>
#include <cstdint>

namespace Hirari::Core {

/**
 * @class AudioBufferView
 * @brief Zero-copy Sub-fragment of an AudioBuffer.
 * HONEST FIX: Provides a 'View' into a larger buffer without any heap 
 * allocation or copying. Essential for Sub-block Rendering where we 
 * need to process a few samples between MIDI events at zero cost.
 */
class AudioBufferView {
public:
    AudioBufferView(float** data, uint32_t channels, uint32_t offset, uint32_t numSamples)
        : m_data(data), m_channels(channels), m_offset(offset), m_numSamples(numSamples) {}

    float* getReadPointer(uint32_t channel) { return &m_data[channel][m_offset]; }
    float* getWritePointer(uint32_t channel) { return &m_data[channel][m_offset]; }
    
    uint32_t getNumSamples() const { return m_numSamples; }
    uint32_t getNumChannels() const { return m_channels; }

    void clear() {
        for (uint32_t c = 0; c < m_channels; ++c) {
            std::fill(getWritePointer(c), getWritePointer(c) + m_numSamples, 0.0f);
        }
    }

private:
    float** m_data;
    uint32_t m_channels, m_offset, m_numSamples;
};

} // namespace Hirari::Core
