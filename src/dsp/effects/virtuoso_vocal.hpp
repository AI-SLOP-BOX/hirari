#pragma once
#include <cmath>
#include <algorithm>
#include <array>
#include "../../core/audio_buffer.hpp"
#include "../iprocessor.hpp"
#include "../utils/pitch_shifter.hpp"
#include "../utils/zdf_filter.hpp"

namespace Aura::DSP::Effects {

/**
 * @class VirtuosoVocal
 * @brief High-end Pitch & Formant Shifter (Vocal Transformer).
 */
class VirtuosoVocal : public IProcessor {
public:
    VirtuosoVocal(double sr = 44100.0) : m_sampleRate(sr) {
        reset();
    }

    void prepareToPlay(double sr, uint32_t /*blockSize*/) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0;
        m_formantFilterL.setSampleRate(m_sampleRate);
        m_formantFilterR.setSampleRate(m_sampleRate);
        reset();
    }

    std::string getName() const override { return "Virtuoso Vocal"; }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;

        const uint32_t numSamples = buffer.getNumSamples();
        const bool isStereo = buffer.getNumChannels() >= 2;
        float* left = buffer.getWritePointer(0);
        float* right = isStereo ? buffer.getWritePointer(1) : nullptr;

        const float pitchRatio = std::pow(2.0f, std::clamp(m_pitchShiftSemi, -24.0f, 24.0f) / 12.0f);

        // Formant shift modifies cutoff frequency
        float formantCutoff = 3000.0f * std::pow(2.0f, std::clamp(m_formantShift, -12.0f, 12.0f) / 12.0f);
        formantCutoff = std::clamp(formantCutoff, 200.0f, 18000.0f);
        // ZDFFilter exposes stable SVF responses rather than a peaking EQ.
        // Band-pass is the closest formant-emphasis response available in this
        // processor and keeps the call aligned with the actual API.
        m_formantFilterL.updateCoefficients(formantCutoff, 0.707f, Utils::ZDFFilter::Type::BandPass);
        if (isStereo) m_formantFilterR.updateCoefficients(formantCutoff, 0.707f, Utils::ZDFFilter::Type::BandPass);

        // PitchShifter is a block processor.  Calling it once per sample would
        // reset its block assumptions and was the source of the build failure.
        m_shifterL.process(left, numSamples, pitchRatio, static_cast<float>(m_sampleRate));
        if (isStereo && right) {
            m_shifterR.process(right, numSamples, pitchRatio, static_cast<float>(m_sampleRate));
        }

        for (uint32_t s = 0; s < numSamples; ++s) {
            float outL = std::isfinite(left[s]) ? left[s] : 0.0f;
            outL = m_formantFilterL.process(outL);
            if (std::abs(outL) < 1.0e-24f) outL = 0.0f;
            left[s] = outL;

            if (isStereo && right) {
                float outR = std::isfinite(right[s]) ? right[s] : 0.0f;
                outR = m_formantFilterR.process(outR);
                if (std::abs(outR) < 1.0e-24f) outR = 0.0f;
                right[s] = outR;
            }
        }
    }


    void reset() noexcept override {
        m_shifterL.reset();
        m_shifterR.reset();
        m_formantFilterL.reset();
        m_formantFilterR.reset();
    }

    void setPitchShift(float semitones) { m_pitchShiftSemi = semitones; }
    void setFormantShift(float shift) { m_formantShift = shift; }

private:
    double m_sampleRate;
    Utils::PitchShifter m_shifterL, m_shifterR;
    Utils::ZDFFilter m_formantFilterL, m_formantFilterR;
    float m_pitchShiftSemi = 0.0f;
    float m_formantShift = 0.0f;
};

/**
 * @class DeEsser
 * @brief Professional Sibilance Reducer.
 */
class DeEsser : public IProcessor {
public:
    DeEsser() : m_threshold(0.2f) {}
    void prepareToPlay(double sr, uint32_t /*blockSize*/) noexcept override {
        m_filterL.setSampleRate(sr);
        m_filterR.setSampleRate(sr);
    }
    std::string getName() const override { return "DeEsser"; }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& /*midi*/, const ProcessContext& /*context*/) noexcept override {
        if (m_bypassed || buffer.getNumChannels() < 2 || buffer.getNumSamples() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getWritePointer(1);
        if (!l || !r) return;
        m_filterL.updateCoefficients(6000.0f, 0.707f, Utils::ZDFFilter::Type::HighPass);
        m_filterR.updateCoefficients(6000.0f, 0.707f, Utils::ZDFFilter::Type::HighPass);

        for (uint32_t s = 0; s < buffer.getNumSamples(); ++s) {
            float sibL = std::abs(m_filterL.process(l[s]));
            float sibR = std::abs(m_filterR.process(r[s]));
            float maxSib = std::max(sibL, sibR);
            float reduction = (maxSib > m_threshold) ? (1.0f - (maxSib - m_threshold) * 0.8f) : 1.0f;
            reduction = std::clamp(reduction, 0.2f, 1.0f);
            l[s] *= reduction;
            r[s] *= reduction;
        }
    }
    void reset() noexcept override {}
private:
    Utils::ZDFFilter m_filterL, m_filterR;
    float m_threshold;
};

} // namespace Aura::DSP::Effects
