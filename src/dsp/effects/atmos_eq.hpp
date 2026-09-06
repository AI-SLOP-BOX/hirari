#pragma once

#include <vector>
#include <array>
#include <cstring>
#include <cstdio>
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

    AtmosEQ(double sr = 44100.0) : m_sampleRate(
        std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0) {
        // Init 12 instances of filters (one set per channel)
        for (auto& ch : m_filters) {
            for (auto& f : ch) f.setSampleRate(m_sampleRate);
        }
    }

    /**
     * @brief PROCESS IMMERSIVE: Applies identical EQ to up to 12 channels simultaneously.
     */
    void processImmersiveRaw(float* const* buffers, uint32_t bufferCount,
                             uint32_t numSamples, const std::vector<Band>& bands) {
        processImmersiveRaw(buffers, bufferCount, numSamples, bands.data(), bands.size());
    }

    void processImmersiveRaw(float* const* buffers, uint32_t bufferCount,
                             uint32_t numSamples, const Band* bands, size_t bandCount) {
        if (!buffers || !bands || bandCount == 0) return;
        const uint32_t channels = std::min<uint32_t>(bufferCount, 12);
        for (uint32_t c = 0; c < channels; ++c) {
            if (!buffers[c]) continue;
            const size_t count = std::min<size_t>(bandCount, 8);
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

    void processImmersive(std::vector<float*>& buffers, uint32_t numSamples,
                          const std::vector<Band>& bands) {
        processImmersiveRaw(buffers.data(), static_cast<uint32_t>(buffers.size()), numSamples, bands);
    }


    void prepareToPlay(double sr, uint32_t bs) noexcept override { (void)bs; setSampleRate(sr); reset(); }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi; (void)context;
        const uint32_t slot = m_activeBandSlot.load(std::memory_order_acquire);
        const uint32_t count = m_bandCount[slot].load(std::memory_order_relaxed);
        if (count == 0 || buffer.getNumChannels() == 0) return;
        std::array<float*, 12> buffers{};
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 12);
        for (uint32_t i = 0; i < channels; ++i) buffers[i] = buffer.getWritePointer(i);
        processImmersiveRaw(buffers.data(), channels, buffer.getNumSamples(), m_bands[slot].data(), count);
    }

    void process(float* l, float* r, uint32_t numSamples) {
        if (!l || !r) return;
        std::array<float*, 2> buffers{l, r};
        static const std::vector<Band> defaultBands = {{1000.0f, 0.0f, 0.707f, Band::Mode::LowPass}};
        processImmersiveRaw(buffers.data(), 2, numSamples, defaultBands);
    }

    void reset() noexcept override { for (auto& channel : m_filters) for (auto& filter : channel) filter.reset(); }

    void setSampleRate(double sr) {
        if (std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0) m_sampleRate = sr;
        else m_sampleRate = 44'100.0;
        for (auto& channel : m_filters)
            for (auto& filter : channel) filter.setSampleRate(m_sampleRate);
    }
    std::string getName() const override { return "Atmos Immersive EQ"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }

    // Called from the control thread. The audio thread only reads the active
    // immutable slot, so band edits never allocate or mutate its live table.
    void setBands(const std::vector<Band>& bands) noexcept {
        const uint32_t current = m_activeBandSlot.load(std::memory_order_relaxed);
        const uint32_t next = current ^ 1u;
        const size_t count = std::min<size_t>(bands.size(), 8);
        for (size_t i = 0; i < count; ++i) {
            Band b = bands[i];
            b.freq = std::clamp(std::isfinite(b.freq) ? b.freq : 1000.0f, 5.0f, static_cast<float>(m_sampleRate * 0.49));
            b.gain = std::clamp(std::isfinite(b.gain) ? b.gain : 0.0f, -24.0f, 24.0f);
            b.q = std::clamp(std::isfinite(b.q) ? b.q : 0.707f, 0.05f, 4.0f);
            m_bands[next][i] = b;
        }
        m_bandCount[next].store(static_cast<uint32_t>(count), std::memory_order_relaxed);
        m_activeBandSlot.store(next, std::memory_order_release);
    }
    void clearBands() noexcept {
        const uint32_t current = m_activeBandSlot.load(std::memory_order_relaxed);
        m_bandCount[current].store(0, std::memory_order_release);
    }

    std::vector<uint8_t> getState() const override {
        constexpr size_t kStateSize = 160;
        std::vector<uint8_t> state(kStateSize, 0);
        const uint32_t magic = 0x41555241u; const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        const uint32_t slot = m_activeBandSlot.load(std::memory_order_acquire);
        const uint32_t count = std::min<uint32_t>(m_bandCount[slot].load(std::memory_order_relaxed), 8u);
        std::memcpy(state.data(), &magic, 4); std::memcpy(state.data()+4, &version, 2);
        std::memcpy(state.data()+6, &flags, 2); std::memcpy(state.data()+8, &m_mix, 4);
        std::memcpy(state.data()+12, &m_sidechainBusId, 4); std::memcpy(state.data()+16, &count, 4);
        for (uint32_t i = 0; i < count; ++i) {
            const size_t offset = 20 + static_cast<size_t>(i) * 16;
            const Band& b = m_bands[slot][i]; const int mode = static_cast<int>(b.mode);
            std::memcpy(state.data()+offset, &b.freq, 4); std::memcpy(state.data()+offset+4, &b.gain, 4);
            std::memcpy(state.data()+offset+8, &b.q, 4); std::memcpy(state.data()+offset+12, &mode, 4);
        }
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 160) return false;
        uint32_t magic = 0, sidechain = 0, count = 0; uint16_t version = 0, flags = 0; float mix = 0.0f;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data()+4, 2); std::memcpy(&flags, state.data()+6, 2);
        std::memcpy(&mix, state.data()+8, 4); std::memcpy(&sidechain, state.data()+12, 4); std::memcpy(&count, state.data()+16, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 || !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || count > 8) return false;
        std::vector<Band> bands; bands.reserve(count);
        for (uint32_t i = 0; i < count; ++i) {
            const size_t offset = 20 + static_cast<size_t>(i) * 16; Band b{}; int mode = 0;
            std::memcpy(&b.freq, state.data()+offset, 4); std::memcpy(&b.gain, state.data()+offset+4, 4);
            std::memcpy(&b.q, state.data()+offset+8, 4); std::memcpy(&mode, state.data()+offset+12, 4);
            if (!std::isfinite(b.freq) || !std::isfinite(b.gain) || !std::isfinite(b.q) || mode < 0 || mode > 2) return false;
            b.mode = static_cast<Band::Mode>(mode); bands.push_back(b);
        }
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain; setBands(bands); return true;
    }

private:
    double m_sampleRate;
    // 12 Channels x 8 Bands (Typical)
    std::array<std::array<Mixing::StateVariableFilter, 8>, 12> m_filters;
    std::array<std::array<Band, 8>, 2> m_bands{};
    std::array<std::atomic<uint32_t>, 2> m_bandCount{{0, 0}};
    std::atomic<uint32_t> m_activeBandSlot{0};
};

} // namespace Aura::DSP::Effects
