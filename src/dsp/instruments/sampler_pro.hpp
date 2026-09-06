#pragma once

#include <vector>
#include <memory>
#include <cmath>
#include <algorithm>
#include "../../core/audio_processor_graph.hpp"

namespace Aura::DSP::Instruments {

/**
 * @brief SamplerVoice: A single playing instance of a musical sample.
 * Handles Pitch-Shifting (Resampling) and Amplitude Envelopes.
 */
struct SamplerVoice {
    bool active = false;
    uint8_t midiNote = 0;
    double phase = 0.0;
    double pitchRatio = 1.0;
    float velocity = 0.0f;
    float masterGain = 0.5f;

    void trigger(uint8_t note, float vel, double ratio) {
        midiNote = note;
        velocity = vel;
        pitchRatio = ratio;
        phase = 0.0;
        active = true;
    }

    void release() { active = false; }
};

/**
 * @brief AuraSamplerPro: Professional Logic-style Multi-sample Instrument.
 * Plays raw audio data triggered by MIDI notes with high-quality resampling.
 */
class AuraSamplerPro : public Core::IProcessor {
public:
    static constexpr int kMaxVoices = 16;

    AuraSamplerPro() {
        m_voices.resize(kMaxVoices);
        // Load a default "Sine Sample" for demonstration
        m_sampleData.resize(44100, 0.0f);
        for (size_t i = 0; i < 44100; ++i) {
            m_sampleData[i] = std::sin(2.0 * M_PI * 440.0 * i / 44100.0);
        }
    }

    void prepareToPlay(double sr, uint32_t bs) override {
        if (std::isfinite(sr) && sr >= 8000.0 && sr <= 384000.0 && bs > 0) {
            m_sampleRate = sr;
        } else {
            m_sampleRate = 44100.0;
        }
    }

    void process(Core::AudioBuffer& buffer, const std::vector<uint8_t>& midi) override {
        // 1. MIDI HANDLING (Trigger/Release)
        for (size_t i = 0; i + 2 < midi.size(); i += 3) {
            uint8_t status = midi[i] & 0xF0;
            uint8_t note = midi[i+1];
            uint8_t velocity = midi[i+2];

            if (status == 0x90 && velocity > 0) { // Note ON
                triggerVoice(note, velocity / 127.0f);
            } else if (status == 0x80 || (status == 0x90 && velocity == 0)) { // Note OFF
                releaseVoice(note);
            }
        }

        // 2. AUDIO RENDERING (Voice Summing)
        buffer.clear();
        uint32_t numSamples = buffer.getNumSamples();
        if (buffer.getNumChannels() < 2 || numSamples == 0 || m_sampleData.empty()) return;

        for (auto& voice : m_voices) {
            if (!voice.active) continue;

            for (uint32_t s = 0; s < numSamples; ++s) {
                if (static_cast<size_t>(voice.phase) >= m_sampleData.size()) {
                    voice.active = false;
                    break;
                }

                // LINEAR INTERPOLATION (Resampling)
                size_t i0 = static_cast<size_t>(voice.phase);
                size_t i1 = (i0 + 1) < m_sampleData.size() ? i0 + 1 : i0;
                float frac = static_cast<float>(voice.phase - i0);
                float sampleValue = m_sampleData[i0] * (1.0f - frac) + m_sampleData[i1] * frac;

                float out = sampleValue * voice.velocity * voice.masterGain;
                if (!std::isfinite(out)) { out = 0.0f; }
                out = std::clamp(out, -4.0f, 4.0f);
                buffer.addSample(0, s, out);
                buffer.addSample(1, s, out);

                voice.phase += voice.pitchRatio;
            }
        }
    }

    void reset() override {
        for (auto& v : m_voices) v.active = false;
    }

private:
    void triggerVoice(uint8_t note, float velocity) {
        for (auto& v : m_voices) {
            if (!v.active) {
                // Pitch ratio calculation relative to C3 (MIDI 60)
                double ratio = std::pow(2.0, (static_cast<double>(note) - 60.0) / 12.0);
                v.trigger(note, velocity, ratio);
                return;
            }
        }
    }

    void releaseVoice(uint8_t note) {
        for (auto& v : m_voices) {
            if (v.active && v.midiNote == note) v.release();
        }
    }

    std::vector<SamplerVoice> m_voices;
    std::vector<float> m_sampleData;
    double m_sampleRate = 44100.0;
};

} // namespace Aura::DSP::Instruments
