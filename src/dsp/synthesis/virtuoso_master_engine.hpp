#pragma once

#include <vector>
#include <cmath>
#include <atomic>
#include <numbers>
#include <array>
#include "../utils/dsp_utils.hpp"
#include "../utils/zdf_filter.hpp"

namespace Aura::Core::DSP::Synthesis {

/**
 * @class VirtuosoMasterEngine
 * @brief VIRTUOSO MASTER ENGINE EXTREME: Advanced Polyphonic Synthesis.
 */
class VirtuosoMasterEngine {
public:
    static constexpr int kMaxVoices = 16;
    
    enum class Instrument { 
        RetroSynth, VintageEP, B3Organ, FMSynth, 
        PhysicalBrass, DensityPad, Vintage808, KarplusString, Subtractive 
    };

    VirtuosoMasterEngine(double sr = 44100.0) : m_sampleRate(sr) {
        for (auto& v : m_voices) v.active = false;
    }

    void setSampleRate(double sr) { m_sampleRate = sr; }

    void triggerNote(int note, float velocity) {
        // Simple voice stealing or find free
        Voice* best = nullptr;
        for (auto& v : m_voices) {
            if (!v.active) { best = &v; break; }
        }
        if (!best) best = &m_voices[0]; // Stealing
        
        best->note = note;
        best->velocity = velocity;
        best->freq = 440.0f * std::pow(2.0f, (note - 69.0f) / 12.0f);
        best->phase = 0.0f;
        best->env = velocity;
        best->active = true;
        best->filter.setSampleRate(m_sampleRate);
        best->filter.updateCoefficients(1200.0f, 0.707f);
    }

    void releaseNote(int note) {
        for (auto& v : m_voices) {
            if (v.active && v.note == note) v.active = false; // Simple gate
        }
    }

    void process(float* l, float* r, size_t numFrames, Instrument type) {
        for (size_t i = 0; i < numFrames; ++i) {
            float out = 0.0f;
            for (auto& v : m_voices) {
                if (!v.active || v.env < 0.001f) continue;
                
                float voiceOut = 0.0f;
                switch (type) {
                    case Instrument::RetroSynth:   voiceOut = renderRetroSynth(v); break;
                    case Instrument::B3Organ:      voiceOut = renderB3Organ(v); break;
                    case Instrument::VintageEP:    voiceOut = renderVintageEP(v); break;
                    case Instrument::FMSynth:      voiceOut = renderFMSynth(v); break;
                    default: voiceOut = renderSubtractive(v); break;
                }
                
                out += voiceOut * v.env;
                v.phase += v.freq / m_sampleRate;
                if (v.phase >= 1.0) v.phase -= 1.0;
            }

            // Global Polishing
            float finalOut = std::tanh(out * 0.8f);
            l[i] += finalOut;
            r[i] += finalOut;
        }
    }

private:
    struct Voice {
        bool active = false;
        int note = 0;
        float freq = 440.0f;
        float velocity = 0.0f;
        float phase = 0.0f;
        float env = 0.0f;
        ::Aura::DSP::Utils::ZDFFilter filter;
    };

    float renderRetroSynth(Voice& v) {
        // Sawtooth + Sub Oscillator + SVF Filter
        float saw = (v.phase * 2.0f) - 1.0f;
        float sub = (std::fmod(v.phase * 0.5f, 1.0f) < 0.5f) ? 1.0f : -1.0f;
        return v.filter.process(saw + sub * 0.5f);
    }

    float renderB3Organ(Voice& v) {
        // Proper Drawbar Additive Modeling (TR-Hammond philosophy)
        static const float harmonics[] = { 0.5f, 1.0f, 1.51f, 2.0f, 3.0f, 4.07f, 5.01f, 6.0f, 8.0f };
        static const float weights[] = { 1.0f, 1.0f, 0.8f, 0.7f, 0.6f, 0.5f, 0.4f, 0.3f, 0.2f };
        float out = 0;
        for (int i = 0; i < 9; ++i) {
            out += std::sin(v.phase * harmonics[i] * Aura::Utils::DSPUtils::TWO_PI) * weights[i];
        }
        return out * 0.15f;
    }

    float renderVintageEP(Voice& v) {
        // Tine/Reed bell emulation: Main fundamental + metallic bell 
        float main = std::sin(v.phase * Aura::Utils::DSPUtils::TWO_PI);
        float bell = std::sin(v.phase * 4.31f * Aura::Utils::DSPUtils::TWO_PI) * std::exp(-v.phase * 10.0f);
        return (main * 0.8f + bell * 0.3f) * 0.7f;
    }

    float renderFMSynth(Voice& v) {
        // 2-Operator FM: Carrier modulated by Modulator
        float mod = std::sin(v.phase * 3.5f * Aura::Utils::DSPUtils::TWO_PI) * 2.0f;
        return std::sin((v.phase + mod) * Aura::Utils::DSPUtils::TWO_PI);
    }

    float renderSubtractive(Voice& v) { return std::sin(v.phase * Aura::Utils::DSPUtils::TWO_PI); }

    double m_sampleRate;
    Voice m_voices[kMaxVoices];
};

} // namespace Aura::Core::DSP::Synthesis
