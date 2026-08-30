#pragma once

#include <vector>
#include <array>
#include <algorithm>
#include <cmath>
#include "../iprocessor.hpp"

namespace Aura::DSP::Mixing {

/**
 * @brief AtmosCompressor: Professional 12-Channel Immersive Dynamics.
 * Standard for Atmos Mastering and Immersive mixing (7.1.4 Layout).
 */
class AtmosCompressor : public IProcessor {
public:
    struct Config {
        float threshold = -20.0f;
        float ratio = 4.0f;
        float attackMs = 10.0f;
        float releaseMs = 100.0f;
        bool linkAll = true; // Essential for spatial phase coherence
    };

    AtmosCompressor(double sr = 44100.0) : m_sampleRate(sr) {}

    /**
     * @brief PROCESS IMMERSIVE: Applies linked compression to up to 12 channels.
     */
    void processImmersive(std::vector<float*>& buffers, uint32_t numSamples, Config cfg) {
        processImmersivePointers(buffers.data(),
            std::min<uint32_t>(static_cast<uint32_t>(buffers.size()), 12), numSamples, cfg);
    }

private:
    void processImmersivePointers(float* const* buffers, uint32_t channels,
                                  uint32_t numSamples, Config cfg) noexcept {
        if (channels == 0 || numSamples == 0) return;
        cfg.threshold = std::clamp(std::isfinite(cfg.threshold) ? cfg.threshold : -20.0f, -60.0f, 0.0f);
        cfg.ratio = std::clamp(std::isfinite(cfg.ratio) ? cfg.ratio : 4.0f, 1.0f, 20.0f);
        const float threshold = std::pow(10.0f, cfg.threshold / 20.0f);
        float gain = 1.0f;
        for (uint32_t i = 0; i < numSamples; ++i) {
            float peak = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) if (buffers[c]) peak = std::max(peak, std::abs(std::isfinite(buffers[c][i]) ? buffers[c][i] : 0.0f));
            const float target = peak > threshold ? std::pow(threshold / peak, 1.0f - 1.0f / cfg.ratio) : 1.0f;
            const float coeff = target < gain ? std::exp(-1.0f / (std::max(0.1f, cfg.attackMs) * 0.001f * m_sampleRate)) : std::exp(-1.0f / (std::max(1.0f, cfg.releaseMs) * 0.001f * m_sampleRate));
            gain = coeff * gain + (1.0f - coeff) * target;
            for (uint32_t c = 0; c < channels; ++c) if (buffers[c]) buffers[c][i] = std::isfinite(buffers[c][i] * gain) ? buffers[c][i] * gain : 0.0f;
        }
    }

public:
    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        const std::array<float*, 2> buffers{l, r};
        processImmersivePointers(buffers.data(), 2, numSamples, Config{});
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        if (buffer.getNumChannels() == 0) return;
        std::array<float*, 12> buffers{};
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 12);
        for (uint32_t c = 0; c < channels; ++c) buffers[c] = buffer.getWritePointer(c);
        processImmersivePointers(buffers.data(), channels, buffer.getNumSamples(), Config{});
    }
    void reset() noexcept override { m_gain = 1.0f; }

    void setSampleRate(double sr) { if (std::isfinite(sr) && sr > 1000.0) m_sampleRate = sr; }
    uint32_t getLatency() const { return 0; }

private:
    float calculateReduction(float peak, const Config& cfg) {
        // (Conceptual RMS/Peak detection with attack/release smoothing)
        const float threshold = std::pow(10.0f, cfg.threshold / 20.0f);
        return peak > threshold ? std::pow(threshold / peak, 1.0f - 1.0f / std::max(1.0f, cfg.ratio)) : 1.0f;
    }

    double m_sampleRate;
    float m_gain = 1.0f;
};

} // namespace Aura::DSP::Mixing
