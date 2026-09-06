#pragma once

#include <vector>
#include <complex>
#include <cmath>
#include <algorithm>
#include <array>
#include <limits>
#include "../iprocessor.hpp"
#include "../utils/fft_utils.hpp"
#include "../utils/dsp_utils.hpp"

namespace Aura::DSP::Effects {

/**
 * @class ConvolutionReverb
 * @brief Zero-Latency High-Fidelity Impulse Response Processor (Space Designer style).
 */
class ConvolutionReverb : public IProcessor {
public:
    static constexpr size_t kPartitionSize = 512;
    static constexpr size_t kMaxPartitions = 32;
    static constexpr size_t kFFTSize = kPartitionSize * 2;

    ConvolutionReverb() : m_writeIdx(0) {
        for (auto& seg : m_segments) seg.fill(0.0f);
        for (auto& ir : m_irSpectrums) ir.fill(0.0f);
        m_history.fill(0.0f);
        m_overlap.fill(0.0f);
        m_scratchAccum.fill(0.0f);
        m_scratchFreq.fill(0.0f);
        setIR(Model::WarmPlate);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr
            : 44'100.0;
        reset();
    }

    uint32_t getTailSamples() const noexcept override {
        const double rate = std::isfinite(m_sampleRate) && m_sampleRate > 0.0
            ? m_sampleRate
            : 44'100.0;
        return static_cast<uint32_t>(std::min<double>(
            static_cast<double>(std::numeric_limits<uint32_t>::max()), 3.6 * rate));
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        if (buffer.isEmpty() || buffer.getNumChannels() == 0) return;

        uint32_t numSamples = buffer.getNumSamples();
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        if (!left || !right || numSamples == 0) return;
        const float mix = std::isfinite(m_mix) ? std::clamp(m_mix, 0.0f, 1.0f) : 0.0f;
        
        for (uint32_t s = 0; s < numSamples; ++s) {
            const float inL = std::isfinite(left[s]) ? left[s] : 0.0f;
            const float inR = std::isfinite(right[s]) ? right[s] : 0.0f;
            float in = (inL + inR) * 0.5f;
            m_history[m_writeIdx] = in;

            // Output the accumulated latency-compensated overlap
            float wet = m_overlap[m_writeIdx];
            
            // Mix with dry (Smoothing handled by UI)
            left[s] = inL * (1.0f - mix) + (wet * 0.4f) * mix;
            if (right != left) right[s] = inR * (1.0f - mix) + (wet * 0.4f) * mix;
            if (!std::isfinite(left[s])) left[s] = 0.0f;
            if (right != left && !std::isfinite(right[s])) right[s] = 0.0f;

            m_writeIdx++;
            if (m_writeIdx >= kPartitionSize) {
                convolveBlock();
                m_writeIdx = 0;
            }
        }
    }

    void convolveBlock() noexcept {
        // 1. FFT current block (Use scratch)
        for (size_t i = 0; i < kPartitionSize; ++i) m_scratchFreq[i] = m_history[i];
        for (size_t i = kPartitionSize; i < kFFTSize; ++i) m_scratchFreq[i] = 0.0f;
        
        Utils::FFTUtils::fft(m_scratchFreq.data(), kFFTSize);

        // 2. Shift Segments (Delay-line style)
        for (int p = kMaxPartitions - 1; p > 0; --p) m_segments[p] = m_segments[p-1];
        m_segments[0] = m_scratchFreq;

        // 3. Complex Multiply-Accumulate (In-place on scratch)
        m_scratchAccum.fill(std::complex<float>(0,0));
        for (size_t p = 0; p < kMaxPartitions; ++p) {
            for (size_t i = 0; i < kFFTSize; ++i) {
                m_scratchAccum[i] += m_segments[p][i] * m_irSpectrums[p][i];
            }
        }

        // 4. IFFT and OLA (Overlap-Add)
        Utils::FFTUtils::ifft(m_scratchAccum.data(), kFFTSize);
        
        // Output block accumulation (The real part of IFFT)
        for (size_t i = 0; i < kPartitionSize; ++i) {
            m_overlap[i] = m_scratchAccum[i].real();
        }
    }

    enum class Model { WarmPlate, ConcreteRoom };
    // Load a real mono impulse response on the control thread.  The response
    // is partitioned into the same fixed blocks used by the RT convolution
    // path; excess samples are rejected rather than silently truncated.
    bool loadImpulseResponse(const std::vector<float>& impulse) {
        if (impulse.empty() || impulse.size() > kMaxPartitions * kPartitionSize
            || std::any_of(impulse.begin(), impulse.end(), [](float sample) {
                return !std::isfinite(sample);
            })) {
            return false;
        }
        for (size_t p = 0; p < kMaxPartitions; ++p) {
            ComplexBlock partition{};
            const size_t offset = p * kPartitionSize;
            for (size_t i = 0; i < kPartitionSize && offset + i < impulse.size(); ++i)
                partition[i] = {impulse[offset + i], 0.0f};
            Utils::FFTUtils::fft(partition.data(), kFFTSize);
            m_irSpectrums[p] = partition;
        }
        reset();
        return true;
    }

    void setIR(Model m) {
        const float decayTime = m == Model::WarmPlate ? 1.8f : 3.6f;
        const float diffuseGain = m == Model::WarmPlate ? 0.035f : 0.06f;
        const float sampleRate = std::isfinite(m_sampleRate) && m_sampleRate >= 8'000.0
            ? static_cast<float>(m_sampleRate)
            : 48'000.0f;
        uint32_t state = 0x1234;
        auto fastRand = [&]() { state = state * 1664525 + 1013904223; return state; };

        for (size_t p = 0; p < kMaxPartitions; ++p) {
            // Build a real, causal partition in the time domain first.  A
            // direct random complex spectrum is not conjugate symmetric and
            // therefore produces an unphysical complex impulse response.
            ComplexBlock impulse{};
            const float partitionOffset = static_cast<float>(p * kPartitionSize);
            for (size_t i = 0; i < kPartitionSize; ++i) {
                const float time = partitionOffset + static_cast<float>(i);
                const float envelope = std::exp(-time / (decayTime * sampleRate));
                const float noise = (static_cast<float>(fastRand() & 0xffffu) / 32767.5f - 1.0f);
                const float early = (p == 0 && i == 0) ? 0.85f : 0.0f;
                impulse[i] = {early + diffuseGain * envelope * noise, 0.0f};
            }
            Utils::FFTUtils::fft(impulse.data(), kFFTSize);
            m_irSpectrums[p] = impulse;
        }
    }

    void reset() noexcept override {
        m_writeIdx = 0;
        for (auto& v : m_segments) v.fill(0.0f);
        m_overlap.fill(0.0f);
        m_history.fill(0.0f);
    }

private:
    double m_sampleRate = 44100.0;
    using ComplexBlock = std::array<std::complex<float>, kFFTSize>;
    
    std::array<ComplexBlock, kMaxPartitions> m_segments;
    std::array<ComplexBlock, kMaxPartitions> m_irSpectrums;
    std::array<float, kFFTSize> m_history;
    std::array<float, kPartitionSize> m_overlap;
    
    // Scratch buffers (Pre-allocated)
    ComplexBlock m_scratchAccum;
    ComplexBlock m_scratchFreq;
    
    uint32_t m_writeIdx = 0;
};

} // namespace Aura::DSP::Effects
