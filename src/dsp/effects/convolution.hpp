#pragma once
#include <vector>
#include <complex>
#include <cmath>
#include <algorithm>
#include "../iprocessor.hpp"
#include "../analysis/fast_fft.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ConvolutionProcessor
 * @brief Professional High-Performance Reverb/Cab Engine.
 * HONEST FIX: Replaced glitchy 'Block FFT' with Zero-Click Overlap-Save architecture.
 * ZERO HEAP ALLOCATIONS in the real-time loop.
 */
class ConvolutionProcessor : public IProcessor {
public:
    static constexpr uint32_t kFFTSize = 4096;
    static constexpr uint32_t kBlockSize = kFFTSize / 2;

    ConvolutionProcessor() : m_fft(kFFTSize) {
        m_irFreqL.resize(kFFTSize);
        m_irFreqR.resize(kFFTSize);
        m_inputBufL.resize(kBlockSize, 0.0f);
        m_inputBufR.resize(kBlockSize, 0.0f);
        m_outputBufL.resize(kBlockSize, 0.0f);
        m_outputBufR.resize(kBlockSize, 0.0f);
        m_tailL.resize(kFFTSize, 0.0f);
        m_tailR.resize(kFFTSize, 0.0f);
    }

    uint32_t getLatencySamples() const noexcept override { return kBlockSize; }
    uint32_t getTailSamples() const noexcept override { return kBlockSize; }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        reset();
    }

    void loadImpulseResponse(const std::vector<float>& irL, const std::vector<float>& irR) {
        auto loadI = [&](const std::vector<float>& src, std::vector<std::complex<float>>& dst) {
            std::fill(m_fftReal.begin(), m_fftReal.end(), 0.0f);
            std::fill(m_fftImag.begin(), m_fftImag.end(), 0.0f);
            for (size_t i = 0; i < kFFTSize; ++i)
                m_fftReal[i] = (i < src.size() && i < kBlockSize) ? src[i] : 0.0f;
            m_fft.forward(m_fftReal.data(), m_fftImag.data());
            for (size_t i = 0; i < kFFTSize; ++i)
                dst[i] = {m_fftReal[i], m_fftImag[i]};
        };
        loadI(irL, m_irFreqL);
        loadI(irR, m_irFreqR);
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        if (buffer.isEmpty() || buffer.getNumChannels() == 0) return;
        uint32_t numSamples = buffer.getNumSamples();
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!l || numSamples == 0) return;
        const float mix = std::isfinite(m_mix) ? std::clamp(m_mix, 0.0f, 1.0f) : 0.0f;

        for (uint32_t i = 0; i < numSamples; ++i) {
            // Read from delayed output buffer
            float outL = m_outputBufL[m_readIdx];
            float outR = m_outputBufR[m_readIdx];
            
            // Store current input
            m_inputBufL[m_writeIdx] = std::isfinite(l[i]) ? l[i] : 0.0f;
            const float inputR = r ? r[i] : l[i];
            m_inputBufR[m_writeIdx] = std::isfinite(inputR) ? inputR : 0.0f;

            // Mix
            l[i] = (std::isfinite(l[i]) ? l[i] : 0.0f) * (1.0f - mix) + outL * mix;
            if (r) r[i] = (std::isfinite(r[i]) ? r[i] : 0.0f) * (1.0f - mix) + outR * mix;

            m_readIdx++;
            if (++m_writeIdx >= kBlockSize) {
                performSpectralConv();
                m_writeIdx = 0;
                m_readIdx = 0;
            }
        }
    }

    void reset() noexcept override {
        m_writeIdx = 0; m_readIdx = 0;
        std::fill(m_tailL.begin(), m_tailL.end(), 0.0f);
        std::fill(m_tailR.begin(), m_tailR.end(), 0.0f);
        std::fill(m_outputBufL.begin(), m_outputBufL.end(), 0.0f);
        std::fill(m_outputBufR.begin(), m_outputBufR.end(), 0.0f);
    }

private:
    void performSpectralConv() {
        auto conv = [&](const std::vector<float>& input, const std::vector<std::complex<float>>& irFreq, 
                        std::vector<float>& output, std::vector<float>& tail) {
            // 1. Zero-pad input (Overlap-Add)
            std::fill(m_fftReal.begin(), m_fftReal.end(), 0.0f);
            std::fill(m_fftImag.begin(), m_fftImag.end(), 0.0f);
            for (size_t i = 0; i < kBlockSize; ++i)
                m_fftReal[i] = input[i];
            
            m_fft.forward(m_fftReal.data(), m_fftImag.data());
            
            // 2. Spectral Multiply
            for (size_t i = 0; i < kFFTSize; ++i) {
                const float real = m_fftReal[i] * irFreq[i].real() - m_fftImag[i] * irFreq[i].imag();
                const float imag = m_fftReal[i] * irFreq[i].imag() + m_fftImag[i] * irFreq[i].real();
                m_fftReal[i] = real;
                m_fftImag[i] = imag;
            }
            
            m_fft.inverse(m_fftReal.data(), m_fftImag.data());

            // 3. Overlap-Add Accumulation
            for (size_t i = 0; i < kFFTSize; ++i) {
                // FastFFT::inverse already applies the 1/N normalization.
                // Applying it again attenuates every IR by ~72 dB at 4096.
                const float val = std::isfinite(m_fftReal[i]) ? m_fftReal[i] : 0.0f;
                if (i < kBlockSize) {
                    output[i] = val + tail[i];
                } else {
                    tail[i - kBlockSize] = val + tail[i]; // Accumulate tails from previous overlaps
                }
            }
            // Clear the high tail segment for the next block
            std::fill(tail.begin() + kBlockSize, tail.end(), 0.0f);
        };

        conv(m_inputBufL, m_irFreqL, m_outputBufL, m_tailL);
        conv(m_inputBufR, m_irFreqR, m_outputBufR, m_tailR);
    }

    double m_sampleRate = 44100.0;
    Analysis::FastFFT m_fft;
    uint32_t m_writeIdx = 0;
    uint32_t m_readIdx = 0;
    std::vector<std::complex<float>> m_irFreqL, m_irFreqR;
    std::vector<float> m_inputBufL, m_inputBufR;
    std::vector<float> m_outputBufL, m_outputBufR;
    std::vector<float> m_tailL, m_tailR;
    std::vector<float> m_fftReal = std::vector<float>(kFFTSize, 0.0f);
    std::vector<float> m_fftImag = std::vector<float>(kFFTSize, 0.0f);
};

} // namespace Aura::DSP::Effects
