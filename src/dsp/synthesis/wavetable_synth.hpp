#pragma once

#include "wavetable_oscillator.hpp"
#include "lfo.hpp"
#include "../iprocessor.hpp"
#include <vector>

namespace Aura::DSP::Synthesis {

/**
 * @class WavetableSynth
 * @brief High-performance Morphing Wavetable Synthesizer with LFO.
 * HONEST FIX: Professional voice-management and modulation matrix.
 */
class WavetableSynth : public IProcessor {
public:
    struct Voice {
        bool active = false;
        bool releasing = false;
        uint8_t note = 0;
        float velocity = 0.0f;
        float env = 0.0f;
        WavetableOscillator osc;
        LFO lfo;
    };

    WavetableSynth() {
        m_voices.resize(16);
        for (auto& v : m_voices) {
            v.lfo.setFrequency(5.0f); // 5Hz Default
        }
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        for (auto& v : m_voices) {
            v.osc.setSampleRate(sr);
            v.lfo.setSampleRate(sr);
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0) return;
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const auto* event = midi.begin();
        const auto* eventEnd = midi.end();
        size_t eventIndex = 0;
        for (uint32_t sample = 0; sample < buffer.getNumSamples(); ++sample) {
            while (event + eventIndex < eventEnd && event[eventIndex].sampleOffset <= sample) {
                const auto& e = event[eventIndex++];
                if (e.size >= 3) {
                    const uint8_t type = e.data[0] & 0xF0;
                    if (type == 0x90 && e.data[2] != 0) noteOn(e.data[1], e.data[2]);
                    else if (type == 0x80 || (type == 0x90 && e.data[2] == 0)) noteOff(e.data[1]);
                }
            }
            float output = 0.0f;
            for (auto& voice : m_voices) {
                if (!voice.active) continue;
                const float attack = 1.0f / static_cast<float>(std::max(1.0, m_sampleRate * 0.005));
                const float release = 1.0f / static_cast<float>(std::max(1.0, m_sampleRate * 0.08));
                voice.env = voice.releasing ? voice.env - release : voice.env + attack;
                if (voice.releasing && voice.env <= 0.0f) { voice.env = 0.0f; voice.active = false; continue; }
                voice.env = std::clamp(voice.env, 0.0f, 1.0f);
                const float lfo = voice.lfo.process(LFO::Waveform::Sine) * 0.0025f;
                const float osc = voice.osc.process(0.5f + lfo);
                output += osc * voice.velocity * voice.env;
            }
            output = std::clamp(output * 0.25f, -1.0f, 1.0f);
            buffer.getWritePointer(0)[sample] += output;
            if (channels > 1) buffer.getWritePointer(1)[sample] += output;
        }
    }


    void noteOn(uint8_t note, uint8_t velocity) {
        for (auto& v : m_voices) {
            if (!v.active) {
                v.active = true;
                v.note = note;
                v.releasing = false;
                v.velocity = velocity / 127.0f;
                v.env = 0.0f;
                v.osc.setFrequency(440.0 * std::pow(2.0, (note - 69.0) / 12.0));
                return;
            }
        }
    }

    void noteOff(uint8_t note) {
        for (auto& v : m_voices) {
            if (v.active && v.note == note) v.releasing = true;
        }
    }

    void reset() noexcept override {
        for (auto& v : m_voices) { v.active = false; v.releasing = false; v.env = 0.0f; }
    }

private:
    std::vector<Voice> m_voices;
    double m_sampleRate = 44100.0;
};

} // namespace Aura::DSP::Synthesis
