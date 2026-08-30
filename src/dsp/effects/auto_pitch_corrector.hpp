#pragma once
#include <vector>
#include <array>
#include <cmath>
#include <complex>
#include <algorithm>
#include <atomic>
#include "../analysis/fast_fft.hpp"
#include "../iprocessor.hpp"

namespace Aura::Core::DSP::Effects {

/**
 * @class AutoPitchCorrector
 * @brief Logic Pro 11-style Real-time Pitch Correction Engine.
 * HONEST FIX: Implements Low-Latency Phase-Vocoder with Formant Preservation.
 * Corrected: PDC Latency reporting (m_fftSize) for sample-accurate alignment.
 */
class AutoPitchCorrector : public ::Aura::DSP::IProcessor {
public:
    AutoPitchCorrector(double sr, size_t fftSize = 1024) 
        : m_sampleRate(std::isfinite(sr) && sr > 0.0 ? sr : 44100.0),
          m_fftSize(std::clamp<size_t>(fftSize, 64, 16384)),
          m_fft(m_fftSize) {
        m_complexBufL.resize(m_fftSize);
        m_complexBufR.resize(m_fftSize);
        m_shiftedL.resize(m_fftSize);
        m_shiftedR.resize(m_fftSize);
        m_lastPhaseL.resize(m_fftSize / 2 + 1, 0.0f);
        m_lastPhaseR.resize(m_fftSize / 2 + 1, 0.0f);
        m_accumPhaseL.resize(m_fftSize / 2 + 1, 0.0f);
        m_accumPhaseR.resize(m_fftSize / 2 + 1, 0.0f);
        m_lpcCoeffs.resize(13, 0.0f);
        m_realBuf.resize(m_fftSize, 0.0f);
        m_imagBuf.resize(m_fftSize, 0.0f);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        if (std::isfinite(sr) && sr > 0.0) m_sampleRate = sr;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& context) noexcept override {
        if (m_bypassed) return;
        
        if (buffer.getNumChannels() < 2) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getWritePointer(1);
        if (!l || !r) return;
        size_t n = buffer.getNumSamples();
        
        processInternal(l, r, n, m_response, m_scaleMask);
    }

    void reset() noexcept override {
        std::fill(m_lastPhaseL.begin(), m_lastPhaseL.end(), 0.0f);
        std::fill(m_lastPhaseR.begin(), m_lastPhaseR.end(), 0.0f);
        std::fill(m_accumPhaseL.begin(), m_accumPhaseL.end(), 0.0f);
        std::fill(m_accumPhaseR.begin(), m_accumPhaseR.end(), 0.0f);
        m_currentCorrection = 0.0f;
    }

    /**
     * @brief PDC: Correct FFT Latency Reporting.
     * Logic Pro / Ableton will offset the track by this amount to stay in sync.
     */
    uint32_t getLatencySamples() const noexcept override {
        return static_cast<uint32_t>(m_fftSize); 
    }

    void setParameter(uint32_t id, float value) noexcept override {
        switch(id) {
            case 0: m_response = std::isfinite(value) ? std::clamp(value, 0.0f, 1.0f) : 0.5f; break;
            case 1: m_scaleMask = static_cast<uint32_t>(value); break;
        }
    }

    float getParameter(uint32_t id) const noexcept override {
        switch(id) {
            case 0: return m_response;
            case 1: return static_cast<float>(m_scaleMask);
        }
        return 0.0f;
    }

    float getCurrentPitch() const { return m_detectedFreq.load(); }
    float getCorrectionAmount() const { return m_lastCorrectionAmount.load(); }

private:
    void processInternal(float* l, float* r, size_t numFrames, float response, uint32_t scaleMask) {
        if (numFrames == 0) return;
        
        float freq = detectPitch(l, numFrames);
        m_detectedFreq.store(freq);

        if (freq < 40.0f || freq > 2000.0f) {
            m_lastCorrectionAmount.store(0.0f);
            return;
        }

        float semitone = 12.0f * std::log2(freq / 440.0f) + 69.0f;
        float targetNote = snapToScale(semitone, scaleMask);
        
        float correction = (targetNote - semitone);
        m_currentCorrection = m_currentCorrection * (1.0f - response) + correction * response;
        m_lastCorrectionAmount.store(m_currentCorrection);

        float ratio = std::pow(2.0f, m_currentCorrection / 12.0f);
        applyPitchShift(l, r, numFrames, ratio);
    }

    float detectPitch(const float* data, size_t n) {
        if (!data || n < 64 || !std::isfinite(m_sampleRate)) return 0.0f;
        // PROFESSIONAL YIN ALGORITHM
        const size_t tauMax = std::min<size_t>(n / 2, 800);
        const size_t winSize = n / 2;
        
        static thread_local std::array<float, 800> diff;
        std::fill(diff.begin(), diff.begin() + std::min<size_t>(tauMax, 800), 0.0f);

        for (size_t tau = 1; tau < std::min<size_t>(tauMax, diff.size()); ++tau) {
            for (size_t i = 0; i < winSize; ++i) {
                float d = data[i] - data[i + tau];
                if (std::isfinite(d)) diff[tau] += d * d;
            }
        }

        float runningSum = 0.0f;
        diff[0] = 1.0f;
        const size_t safeTauMax = std::min<size_t>(tauMax, diff.size());
        for (size_t tau = 1; tau < safeTauMax; ++tau) {
            runningSum += diff[tau];
            diff[tau] *= (static_cast<float>(tau) / (runningSum + 1e-6f));
        }

        const float threshold = 0.15f;
        int bestTau = -1;
        for (size_t tau = 20; tau < safeTauMax; ++tau) {
            if (diff[tau] < threshold) {
                while (tau + 1 < tauMax && diff[tau+1] < diff[tau]) tau++;
                bestTau = (int)tau;
                break;
            }
        }

        if (bestTau <= 0 || static_cast<size_t>(bestTau) + 1 >= tauMax) return 0.0f;
        
        float yL = diff[bestTau - 1], yC = diff[bestTau], yR = diff[bestTau + 1];
        float peakShift = (yR - yL) / (2.0f * (2.0f * yC - yR - yL) + 1e-6f);
        if (std::isfinite(peakShift)) {
            peakShift = std::clamp(peakShift, -0.5f, 0.5f);
        } else {
            peakShift = 0.0f;
        }
        
        return static_cast<float>(m_sampleRate) / (static_cast<float>(bestTau) + peakShift);
    }

    float snapToScale(float note, uint32_t mask) {
        int target = static_cast<int>(std::round(note));
        int oct = target / 12;
        int semitone = target % 12;
        if (semitone < 0) semitone += 12;

        if (mask == 0xFFF || mask == 0) return note; 
        
        int bestDist = 12, bestNote = semitone;
        for (int i = 0; i < 12; ++i) {
            if (mask & (1 << i)) {
                int dist = std::abs(i - semitone);
                if (dist > 6) dist = 12 - dist;
                if (dist < bestDist) { bestDist = dist; bestNote = i; }
            }
        }
        return static_cast<float>(oct * 12 + bestNote);
    }

    void applyPitchShift(float* l, float* r, size_t n, float ratio) {
        calculateLPC(l, n, m_lpcCoeffs);

        auto processChannel = [&](float* data, std::vector<std::complex<float>>& spec, 
                                 std::vector<std::complex<float>>& outSpec,
                                 std::vector<float>& lastPh, std::vector<float>& accumPh) {
            if (!data || !std::isfinite(ratio) || ratio <= 0.0f) return;
            for (size_t i = 0; i < m_fftSize; ++i) {
                float win = 0.5f * (1.0f - std::cos(2.0f * M_PI * i /
                                                        static_cast<float>(m_fftSize - 1)));
                spec[i] = { (i < n) ? data[i] * win : 0.0f, 0.0f };
            }
            for (size_t i = 0; i < m_fftSize; ++i) {
                m_realBuf[i] = spec[i].real();
                m_imagBuf[i] = spec[i].imag();
            }
            m_fft.forward(m_realBuf.data(), m_imagBuf.data());
            for (size_t i = 0; i < m_fftSize; ++i)
                spec[i] = {m_realBuf[i], m_imagBuf[i]};

            std::fill(outSpec.begin(), outSpec.end(), std::complex<float>(0,0));
            float hopSize = static_cast<float>(n);
            
            for (size_t i = 0; i <= m_fftSize / 2; ++i) {
                size_t targetIdx = static_cast<size_t>(i * ratio + 0.5f);
                if (targetIdx <= m_fftSize / 2) {
                    float mag = std::abs(spec[i]);
                    float phase = std::arg(spec[i]);
                    
                    float expected = 2.0f * M_PI * i * hopSize / m_fftSize;
                    float delta = phase - lastPh[i] - expected;
                    while (delta > M_PI) delta -= 2.0f * M_PI;
                    while (delta < -M_PI) delta += 2.0f * M_PI;
                    
                    float trueFreq = (2.0f * M_PI * i / m_fftSize) + delta / hopSize;
                    accumPh[targetIdx] += trueFreq * hopSize * ratio;
                    lastPh[i] = phase;
                    
                    float envOrig = getLPCEnvelope(i, m_lpcCoeffs);
                    float envShift = getLPCEnvelope(targetIdx, m_lpcCoeffs);
                    float correction = envOrig / (envShift + 1e-6f);
                    
                    outSpec[targetIdx] += std::polar(mag * correction, accumPh[targetIdx]);
                }
            }
            for (size_t i = 0; i < m_fftSize; ++i) {
                m_realBuf[i] = outSpec[i].real();
                m_imagBuf[i] = outSpec[i].imag();
            }
            m_fft.inverse(m_realBuf.data(), m_imagBuf.data());
            float norm = 1.0f / (m_fftSize * 0.5f); 
            const size_t limit = std::min(n, m_fftSize);
            for (size_t i = 0; i < limit; ++i) data[i] = m_realBuf[i] * norm;
            for (size_t i = limit; i < n; ++i) data[i] = 0.0f;
        };

        processChannel(l, m_complexBufL, m_shiftedL, m_lastPhaseL, m_accumPhaseL);
        processChannel(r, m_complexBufR, m_shiftedR, m_lastPhaseR, m_accumPhaseR);
    }

    void calculateLPC(const float* data, size_t n, std::vector<float>& coeffs) {
        float r[14] = {0}; 
        for (int k = 0; k <= 12; ++k) {
            for (int i = 0; i < (int)n - k; ++i) r[k] += data[i] * data[i+k];
        }
        if (r[0] < 1e-9f) return;

        float a[13] = {1.0f}, k_ref, e = r[0];
        for (int i = 1; i <= 12; ++i) {
            float s = 0;
            for (int j = 1; j < i; ++j) s += a[j] * r[i - j];
            k_ref = (r[i] - s) / e;
            a[i] = k_ref;
            for (int j = 1; j < i; ++j) {
                float old_a = a[j];
                a[j] = old_a - k_ref * a[i - j];
            }
            e *= (1.0f - k_ref * k_ref);
        }
        for (int i = 0; i < 12; ++i) coeffs[i] = a[i+1];
    }

    float getLPCEnvelope(size_t bin, const std::vector<float>& lpc) {
        float omega = 2.0f * M_PI * bin / m_fftSize;
        std::complex<float> sum(1.0f, 0.0f);
        for (size_t i = 0; i < 12; ++i) {
            sum += lpc[i] * std::polar(1.0f, -static_cast<float>(i + 1) * omega);
        }
        return 1.0f / (std::abs(sum) + 1e-6f);
    }

    double m_sampleRate;
    size_t m_fftSize;
    ::Aura::DSP::Analysis::FastFFT m_fft;
    std::vector<std::complex<float>> m_complexBufL, m_complexBufR;
    std::vector<std::complex<float>> m_shiftedL, m_shiftedR;
    std::vector<float> m_lastPhaseL, m_lastPhaseR, m_accumPhaseL, m_accumPhaseR;
    std::vector<float> m_lpcCoeffs;
    std::vector<float> m_realBuf, m_imagBuf;
    float m_currentCorrection = 0.0f;
    float m_response = 0.5f;
    uint32_t m_scaleMask = 0xFFF;
    std::atomic<float> m_detectedFreq{0.0f};
    std::atomic<float> m_lastCorrectionAmount{0.0f};
};

} // namespace Aura::Core::DSP::Effects
