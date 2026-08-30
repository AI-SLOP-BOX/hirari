#pragma once

#include <vector>
#include <array>
#include "../iprocessor.hpp"
#include "../mixing/state_variable_filter.hpp"

namespace Aura::DSP::Effects {

/**
 * @brief AtmosEQ: Professional 12-Channel Immersive Equalizer.
 * Standard for Atmos Mastering and Immersive Tonal Balance (7.1.4 Layout).
 */
class AtmosEQ : public IProcessor {
public:
    struct Band {
        float freq;
        float gain;
        float q;
        enum class Mode { LowPass, BandPass, HighPass } mode;
    };

    AtmosEQ(double sr = 44100.0) : m_sampleRate(sr) {
        // Init 12 instances of filters (one set per channel)
        for (auto& ch : m_filters) {
            for (auto& f : ch) f.setSampleRate(sr);
        }
    }

    /**
     * @brief PROCESS IMMERSIVE: Applies identical EQ to up to 12 channels simultaneously.
     */
    void processImmersive(std::vector<float*>& buffers, uint32_t numSamples, const std::vector<Band>& bands) {
        const uint32_t channels = std::min<uint32_t>(static_cast<uint32_t>(buffers.size()), 12);
        for (uint32_t c = 0; c < channels; ++c) {
            if (!buffers[c]) continue;
            const size_t count = std::min<size_t>(bands.size(), 8);
            for (size_t b = 0; b < count; ++b) {
                const Band& band = bands[b];
                m_filters[c][b].setParameters(
                    std::clamp(std::isfinite(band.freq) ? band.freq : 1000.0f, 5.0f, static_cast<float>(m_sampleRate * 0.49)),
                    std::clamp(std::isfinite(band.q) ? band.q : 0.707f, 0.05f, 4.0f),
                    static_cast<int>(band.mode));
                switch (band.mode) {
                    case Band::Mode::HighPass: m_filters[c][b].processBlockHP(buffers[c], numSamples); break;
                    case Band::Mode::BandPass: m_filters[c][b].processBlockBP(buffers[c], numSamples); break;
                    case Band::Mode::LowPass:
                    default: m_filters[c][b].processBlockLP(buffers[c], numSamples); break;
                }
                if (std::isfinite(band.gain) && std::abs(band.gain) > 0.001f) {
                    const float gain = std::pow(10.0f, std::clamp(band.gain, -24.0f, 24.0f) / 20.0f);
                    for (uint32_t i = 0; i < numSamples; ++i) buffers[c][i] *= gain;
                }
            }
        }
    }


    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (buffer.getNumChannels() == 0) return;
        std::array<float*, 12> buffers{};
        const uint32_t count = std::min<uint32_t>(buffer.getNumChannels(), 12);
        for (uint32_t i = 0; i < count; ++i) buffers[i] = buffer.getWritePointer(i);
        static const std::array<Band, 1> defaultBands{{{1000.0f, 0.0f, 0.707f, Band::Mode::LowPass}}};
        for (uint32_t c = 0; c < count; ++c) {
            if (!buffers[c]) continue;
            for (const Band& band : defaultBands) {
                m_filters[c][0].setParameters(band.freq, band.q, 0);
                m_filters[c][0].processBlockLP(buffers[c], buffer.getNumSamples());
            }
        }
    }

    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        std::array<float*, 2> buffers{l, r};
        static const std::vector<Band> defaultBands = {{1000.0f, 0.0f, 0.707f, Band::Mode::LowPass}};
        std::vector<float*> view{buffers.begin(), buffers.end()};
        processImmersive(view, numSamples, defaultBands);
    }

    void reset() noexcept override { for (auto& channel : m_filters) for (auto& filter : channel) filter.reset(); }

    void setSampleRate(double sr) { if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr; }
    uint32_t getLatency() const { return 0; }

private:
    double m_sampleRate;
    // 12 Channels x 8 Bands (Typical)
    std::array<std::array<Mixing::StateVariableFilter, 8>, 12> m_filters;
};

} // namespace Aura::DSP::Effects
