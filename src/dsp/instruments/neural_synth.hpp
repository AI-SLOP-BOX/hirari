#pragma once

#include <vector>
#include <cmath>
#include <array>
#include <string>
#include <memory>
#include <atomic>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../../core/engine/macro_control_manager.hpp"

namespace Aura::DSP::Instruments {

/**
 * @class NeuralSynth
 * @brief High-Performance Wavetable Morphing Engine.
 */
class NeuralSynth {
public:
    static constexpr int kMaxVoices = 32;
    static constexpr int kOscPerVoice = 4;
    static constexpr int kTableSize = 2048;

    struct Voice {
        bool active = false;
        uint8_t note = 0;
        float velocity = 0.0f;
        double phase[kOscPerVoice]{0.0};
        float envValue = 0.0f;
        uint8_t envState = 0; // 0: Idle, 1: Attack, 2: Decay, 3: Sustain, 4: Release
        float filterState[2][4]{{0.0f}};
    };

    NeuralSynth() {
        m_voices.fill({});
        // --- 1. SOVEREIGN WAVETABLE INITIALIZATION ---
        for (int i = 0; i < kTableSize; ++i) {
            float ph = (float)i / kTableSize;
            m_sinTable[i] = std::sin(ph * 2.0f * 3.14159f);
            m_gritTable[i] = (ph < 0.5f) ? 1.0f : -1.0f; // Square/Grit
        }
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, double sampleRate) {
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getWritePointer(1);
        uint32_t len = buffer.getNumSamples();

        handleMidi(midi);

        // Fetch Macro 0 for Neural Morphing
        m_morph = Core::Engine::MacroControlManager::getInstance().getMacroValue(0);

        for (uint32_t s = 0; s < len; ++s) {
            float out = 0.0f;
            for (auto& v : m_voices) {
                if (v.active) out += generateVoiceSample(v, sampleRate);
            }
            l[s] += out * 0.25f;
            r[s] += out * 0.25f;
        }
    }

private:
    void handleMidi(Core::MidiBuffer& midi) {
        for (const auto& ev : midi) {
            uint8_t status = ev.data[0] & 0xF0;
            if (status == 0x90 && ev.data[2] > 0) {
                triggerVoice(ev.data[ status == 0x90 ? 1 : 0], ev.data[2] / 127.0f);
            } else if (status == 0x80) {
                releaseVoice(ev.data[1]);
            }
        }
    }

    void triggerVoice(uint8_t note, float vel) {
        for (auto& v : m_voices) {
            if (!v.active) {
                v.active = true; v.note = note; v.velocity = vel;
                v.envState = 1; v.envValue = 0.0f;
                return;
            }
        }
    }

    void releaseVoice(uint8_t note) {
        for (auto& v : m_voices) {
            if (v.active && v.note == note) v.envState = 4;
        }
    }

    float generateVoiceSample(Voice& v, double sr) {
        float freq = 440.0f * std::pow(2.0f, (v.note - 69.0f) / 12.0f);
        float sig = 0.0f;

        // --- ENVELOPE ---
        updateEnvelope(v);
        
        // --- WAVETABLE OSCILLATORS ---
        for (int i = 0; i < kOscPerVoice; ++i) {
            v.phase[i] += freq / sr;
            if (v.phase[i] >= 1.0) v.phase[i] -= 1.0;
            
            int idx = (int)(v.phase[i] * kTableSize) % kTableSize;
            float silk = m_sinTable[idx];
            float grit = m_gritTable[idx];
            
            // NEURAL MORPHING: Blend between Silk and Grit
            sig += silk * (1.0f - m_morph) + grit * m_morph;
        }

        return sig * v.envValue * v.velocity;
    }

    void updateEnvelope(Voice& v) {
        const float a = 0.001f, r = 0.0005f;
        if (v.envState == 1) { // A
            v.envValue += a; if (v.envValue >= 1.0f) { v.envValue = 1.0f; v.envState = 3; }
        } else if (v.envState == 4) { // R
            v.envValue -= r; if (v.envValue <= 0.0f) { v.envValue = 0.0f; v.active = false; }
        }
    }

    std::array<float, kTableSize> m_sinTable, m_gritTable;
    std::array<Voice, kMaxVoices> m_voices;
    float m_morph = 0.0f;
};

} // namespace Aura::DSP::Instruments
