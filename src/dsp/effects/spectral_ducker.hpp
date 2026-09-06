#pragma once

#include <vector>
#include <complex>
#include <cmath>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include "../iprocessor.hpp"
#include "../utils/fft_utils.hpp"

namespace Aura::DSP::Effects {

/**
 * @class SidechainSpectralDucker
 * @brief Calculation-based 'Smart' Ducker (Soothe2/TrackSpacer style).
 * HONEST FIX: Implements FFT-based dynamic ducking. 
 * If the Sidechain (Modulator) has energy in a specific frequency bin, 
 * this effect ducks ONLY that bin in the main track.
 * Prevents 'Frequency Masking' (clashes between Kick/Bass or Vocal/Music) 
 * using surgical spectral subtraction rather than broadband ducking.
 */
class SidechainSpectralDucker : public IProcessor {
public:
    static constexpr size_t kFFTSize = 1024;

    SidechainSpectralDucker() : m_writeIdx(0) {
        m_modBuffer.assign(kFFTSize, 0.0f);
        m_carBuffer.assign(kFFTSize, 0.0f);
        m_outBuffer.assign(kFFTSize, 0.0f);
        m_env.assign(kFFTSize / 2, 0.0f);
        m_fftMod.assign(kFFTSize, {});
        m_fftCar.assign(kFFTSize, {});
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        reset();
    }

    /**
     * @brief PROCESS: Uses Sidechain input to dynamically EQ the carrier.
     */
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)midi;
        if (m_bypassed) return;

        // 1. Fetch External Sidechain (e.g., Vocal or Kick)
        auto sidechainBus = context.sidechainBuffer;
        if (!sidechainBus || sidechainBus->getNumChannels() < 2 ||
            buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0 ||
            sidechainBus->getNumSamples() < buffer.getNumSamples()) return;

        uint32_t numSamples = buffer.getNumSamples();
        const float* scL = sidechainBus->getReadPointer(0);
        const float* scR = sidechainBus->getReadPointer(1);
        if (!scL || !scR) return;

        for (uint32_t s = 0; s < numSamples; ++s) {
            const float left = buffer.getReadPointer(0)[s];
            const float right = buffer.getReadPointer(1)[s];
            float midCar = std::isfinite(left) && std::isfinite(right) ? (left + right) * 0.5f : 0.0f;
            float midMod = std::isfinite(scL[s]) && std::isfinite(scR[s]) ? (scL[s] + scR[s]) * 0.5f : 0.0f;

            m_carBuffer[m_writeIdx] = midCar;
            m_modBuffer[m_writeIdx] = midMod;
            
            // 2. Playback Output
            // Preserve the carrier's stereo image while applying the
            // frequency-selective mid-channel reduction.
            const float ducked = m_outBuffer[m_writeIdx];
            const float ratio = std::fabs(midCar) > 1.0e-6f ? ducked / midCar : 1.0f;
            buffer.getWritePointer(0)[s] = std::isfinite(left * ratio) ? left * ratio : 0.0f;
            buffer.getWritePointer(1)[s] = std::isfinite(right * ratio) ? right * ratio : 0.0f;

            m_writeIdx++;
            if (m_writeIdx >= kFFTSize) {
                analyzeAndDuck();
                m_writeIdx = 0;
            }
        }
    }

    void analyzeAndDuck() {
        // Workspaces are allocated once in the constructor, never on the
        // audio callback path.
        for (size_t i = 0; i < kFFTSize; ++i) {
            float win = 0.5f * (1.0f - std::cos(2*M_PI*i / (kFFTSize-1)));
            m_fftMod[i] = std::complex<float>(m_modBuffer[i] * win, 0.0f);
            m_fftCar[i] = std::complex<float>(m_carBuffer[i] * win, 0.0f);
        }

        Utils::FFTUtils::fft(m_fftMod);
        Utils::FFTUtils::fft(m_fftCar);

        // 3. Subtract spectral energy (Calculation-based Smart Eq)
        for (size_t i = 0; i < kFFTSize / 2; ++i) {
            float modMag = std::abs(m_fftMod[i]);
            float duckAmount = std::clamp(1.0f - (modMag * m_amount), 0.1f, 1.0f);
            
            m_fftCar[i] *= duckAmount;
            if (i > 0) m_fftCar[kFFTSize - i] *= duckAmount;
        }

        Utils::FFTUtils::ifft(m_fftCar);
        for (size_t i = 0; i < kFFTSize; ++i) m_outBuffer[i] = m_fftCar[i].real();
    }

    void reset() noexcept override {
        m_writeIdx = 0;
        std::fill(m_modBuffer.begin(), m_modBuffer.end(), 0.0f);
        std::fill(m_carBuffer.begin(), m_carBuffer.end(), 0.0f);
        std::fill(m_outBuffer.begin(), m_outBuffer.end(), 0.0f);
    }

    // Parameters
    void setAmount(float a) noexcept { m_amount = std::isfinite(a) ? std::clamp(a, 0.0f, 1.0f) : 0.5f; }
    std::string getName() const override { return "Sidechain Spectral Ducker"; }
    uint32_t getLatencySamples() const noexcept override { return static_cast<uint32_t>(kFFTSize); }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) setAmount(value); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false;
        out = {0.0f, 1.0f, false};
        return true;
    }
    void getParameterName(uint32_t id, char* outName, uint32_t maxSize) const noexcept override {
        if (!outName || maxSize == 0) return;
        std::snprintf(outName, maxSize, "%s", id == 0 ? "Amount" : "");
    }
    std::vector<uint8_t> getState() const override {
        std::vector<uint8_t> state(20, 0);
        const uint32_t magic = 0x41555241u;
        const uint16_t version = 1;
        const uint16_t flags = static_cast<uint16_t>(m_bypassed ? 1u : 0u);
        std::memcpy(state.data(), &magic, 4);
        std::memcpy(state.data() + 4, &version, 2);
        std::memcpy(state.data() + 6, &flags, 2);
        std::memcpy(state.data() + 8, &m_mix, 4);
        std::memcpy(state.data() + 12, &m_sidechainBusId, 4);
        std::memcpy(state.data() + 16, &m_amount, 4);
        return state;
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() != 20) return false;
        uint32_t magic = 0, sidechain = 0;
        uint16_t version = 0, flags = 0;
        float mix = 0.0f, amount = 0.0f;
        std::memcpy(&magic, state.data(), 4); std::memcpy(&version, state.data() + 4, 2);
        std::memcpy(&flags, state.data() + 6, 2); std::memcpy(&mix, state.data() + 8, 4);
        std::memcpy(&sidechain, state.data() + 12, 4); std::memcpy(&amount, state.data() + 16, 4);
        if (magic != 0x41555241u || version != 1 || (flags & ~1u) != 0 ||
            !std::isfinite(mix) || mix < 0.0f || mix > 1.0f || !std::isfinite(amount) ||
            amount < 0.0f || amount > 1.0f) return false;
        m_bypassed = (flags & 1u) != 0; m_mix = mix; m_sidechainBusId = sidechain;
        m_amount = amount;
        return true;
    }

private:
    double m_sampleRate = 44100.0;
    std::vector<float> m_modBuffer, m_carBuffer, m_outBuffer, m_env;
    std::vector<std::complex<float>> m_fftMod, m_fftCar;
    uint32_t m_writeIdx;
    float m_amount = 0.5f;
};

} // namespace Aura::DSP::Effects
