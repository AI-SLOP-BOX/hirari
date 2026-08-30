#pragma once

// Legacy metadata-oriented API backed by the canonical core WAV decoder.
#include "../core/io/audio_decoder.hpp"

namespace Aura::IO {

class WavReader {
public:
    explicit WavReader(const std::string& path) {
        Core::IO::WavDecoder decoder;
        if (!decoder.open(path)) return;
        Core::AudioBuffer buffer;
        decoder.decodeFull(buffer);
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        m_sampleRate = static_cast<uint32_t>(decoder.getSampleRate());
        m_numChannels = buffer.getNumChannels();
        m_numSamples = buffer.getNumSamples();
        m_data.resize(m_numChannels);
        for (uint32_t channel = 0; channel < m_numChannels; ++channel) {
            const float* source = buffer.getReadPointer(channel);
            m_data[channel].assign(source, source + m_numSamples);
        }
    }

    uint32_t getSampleRate() const { return m_sampleRate; }
    uint32_t getNumChannels() const { return m_numChannels; }
    uint64_t getNumSamples() const { return m_numSamples; }
    const std::vector<float>& getChannelData(uint32_t channel) const {
        static const std::vector<float> empty;
        return channel < m_data.size() ? m_data[channel] : empty;
    }

private:
    uint32_t m_sampleRate = 0;
    uint32_t m_numChannels = 0;
    uint64_t m_numSamples = 0;
    std::vector<std::vector<float>> m_data;
};

} // namespace Aura::IO
