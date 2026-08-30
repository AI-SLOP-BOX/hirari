#pragma once

#include <vector>
#include <memory>
#include <atomic>
#include <cmath>
#include <algorithm>
#include "dsp/effects/pro_limiter.hpp"
#include "dsp/effects/true_peak_limiter.hpp"
#include "dsp/effects/virtuoso_tape.hpp"
#include "dsp/effects/virtuoso_pultec.hpp"
#include "dsp/effects/fet_compressor.hpp"
#include "dsp/analysis/master_meter.hpp"
#include "dsp/utils/dither.hpp"
#include "core/concurrency/simd_kernel.hpp"
#include "core/audio_buffer.hpp"
#include "dsp/iprocessor.hpp"

namespace Aura::DSP::Mixing {

/**
 * @class MasterSuite
 * @brief Professional High-End Mastering Strip.
 * Inherits from IProcessor for unified engine integration.
 */
class MasterSuite : public IProcessor {
public:
    MasterSuite(double sr = 44100.0) : m_sampleRate(sr), m_busCompressor(sr), m_tapeSaturator(sr), m_pultec(sr) {
        m_busCompressor.setThreshold(-20.0f);
        m_busCompressor.setRatio(2.0f);
        m_busCompressor.setAttack(30.0f);
        m_busCompressor.setRelease(100.0f);
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        m_pultec.prepareToPlay(sr, bs);
        m_tapeSaturator.prepareToPlay(sr, bs);
        m_busCompressor.prepareToPlay(sr, bs);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getWritePointer(1);
        uint32_t numSamples = buffer.getNumSamples();

        // 1. AI: AUTOMATIC GAIN STAGING (Loudness Matching)
        if (m_autoGainEnabled.load(std::memory_order_relaxed)) {
            const auto metrics = m_meter.getLatestData();
            const float measured = metrics.lufsShortTerm;
            if (std::isfinite(measured) && measured > -100.0f) {
                const float targetOffset = std::clamp(
                    m_targetLUFS.load(std::memory_order_relaxed) - measured, -12.0f, 12.0f);
                m_autoGainOffset += 0.02f * (targetOffset - m_autoGainOffset);
            }
            const float gain = std::pow(10.0f, m_autoGainOffset / 20.0f);
            for (uint32_t i = 0; i < numSamples; ++i) {
                l[i] *= gain;
                r[i] *= gain;
            }
        }

        // 2. TONAL SHAPING (Pultec EQ -> Tape Saturation)
        m_pultec.process(buffer, midi, context);
        m_tapeSaturator.process(buffer, midi, context);
        
        // 3. DYNAMICS CONTROL (Glue Compression)
        m_busCompressor.process(buffer, midi, context);

        // 2. MID/SIDE STEREO WIDTH (Professional Matrix Implementation)
        if (m_enableMidSide.load(std::memory_order_relaxed)) {
            const float width = std::clamp(m_stereoWidth.load(std::memory_order_relaxed), 0.0f, 2.0f);
            constexpr float kInvSqrt2 = 0.7071067811865475f;
            for (uint32_t i = 0; i < numSamples; ++i) {
                const float mid = (l[i] + r[i]) * kInvSqrt2;
                const float side = (l[i] - r[i]) * kInvSqrt2 * width;
                l[i] = (mid + side) * kInvSqrt2;
                r[i] = (mid - side) * kInvSqrt2;
            }
        }
        
        // 3. FINAL TRUE-PEAK LIMITING (Must be done AFTER all processing)
        m_limiter.process(l, r, numSamples, 0.0f, -0.1f);
        
        // 4. DITHER (Optional, only for final output stage)
        if (m_ditherEnabled.load(std::memory_order_relaxed)) {
            m_ditherL.processBlock(l, numSamples);
            m_ditherR.processBlock(r, numSamples);
        }

        // 5. FINAL ANALYSIS (Meter sees the EXACT output hitting the speakers)
        m_meter.process(l, r, numSamples);
    }


    void reset() noexcept override {
        m_pultec.reset();
        m_tapeSaturator.reset();
        m_busCompressor.reset();
        m_limiter.reset();
        m_autoGainOffset = 0.0f;
    }

    Analysis::MasterMeter::MeterData getLatestMetrics() const {
        return m_meter.getLatestData();
    }

    void setAutoGain(bool enable, float targetLUFS = -14.0f) {
        m_targetLUFS.store(std::clamp(targetLUFS, -60.0f, 0.0f), std::memory_order_relaxed);
        m_autoGainEnabled.store(enable, std::memory_order_release);
    }

    void setMidSideEnabled(bool enable) noexcept { m_enableMidSide.store(enable, std::memory_order_release); }
    void setStereoWidth(float width) noexcept { m_stereoWidth.store(std::clamp(width, 0.0f, 2.0f), std::memory_order_relaxed); }
    void setDitherEnabled(bool enable) noexcept { m_ditherEnabled.store(enable, std::memory_order_release); }

    float getMasterGain() const { return std::pow(10.0f, m_autoGainOffset / 20.0f); }

private:
    double m_sampleRate = 44100.0;
    std::atomic<bool> m_enableMidSide{false};
    std::atomic<bool> m_ditherEnabled{false};
    std::atomic<bool> m_autoGainEnabled{false};
    std::atomic<float> m_targetLUFS{-14.0f};
    float m_autoGainOffset = 0.0f;
    std::atomic<float> m_stereoWidth{1.0f};
    Effects::TruePeakLimiter m_limiter;
    Effects::FETCompressor m_busCompressor;
    Effects::VirtuosoTapeSaturator m_tapeSaturator;
    Effects::VirtuosoPultec m_pultec;
    Analysis::MasterMeter m_meter;
    Utils::TPDFDither m_ditherL, m_ditherR;
};

} // namespace Aura::DSP::Mixing
