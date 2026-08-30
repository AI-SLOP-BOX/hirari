#pragma once

#include "../iprocessor.hpp"
#include "../analysis/spectral_editor.hpp"
#include "../../core/concurrency/status_queue.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SpectralRestorationProcessor
 * @brief Industrial Surgical Repair Processor (Aura Studio Pro).
 * Integrates the SpectralEditor kernel into the real-time processing chain.
 */
class SpectralRestorationProcessor : public IProcessor {
public:
    SpectralRestorationProcessor(double sr = 44100.0) 
        : m_sampleRate(sr), m_editor(2048) {
        reset();
    }

    std::string getName() const override { return "Spectral Restoration"; }

    void prepareToPlay(double sr, uint32_t /*blockSize*/) noexcept override {
        m_sampleRate = sr;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (isBypassed()) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const uint32_t numChannels = buffer.getNumChannels();

        for (uint32_t ch = 0; ch < numChannels; ++ch) {
            float* data = buffer.getWritePointer(ch);
            // In a real industrial implementation, we'd have one editor per channel 
            // to avoid phase-blurring or use a linked stereo editor.
            // For now, we process mono-summed or assume mono for surgery.
            m_editor.process(data, data, numSamples);
        }
    }

    void setLearnMode(bool active) { m_editor.setLearnMode(active); }
    void setRestorationActive(bool active) { m_editor.setRestorationActive(active); }
    void setDenoiseThreshold(float t) { m_editor.setDenoiseThreshold(t); }
    
    void eraseHarmonics(float fund, float bw) {
        m_editor.requestEraseHarmonics(fund, (float)m_sampleRate, bw);
    }

    void reset() noexcept override {
        m_editor.reset();
    }

private:
    double m_sampleRate;
    Analysis::SpectralEditor m_editor;
};

} // namespace Aura::DSP::Effects
