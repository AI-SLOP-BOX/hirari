#pragma once

#include <cmath>
#include <vector>
#include <array>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../iprocessor.hpp"
#include "../utils/dsp_utils.hpp"

namespace Aura::DSP::Synthesis {

/**
 * @class VirtuosoStradivari
 * @brief Maniacal Physical Modeling String Synthesis (WDF & Waveguide).
 */
class VirtuosoStradivari : public IProcessor {
public:
    static constexpr int kMaxPolyphony = 8;
    static constexpr int kDelayBufferSize = 4096;
    static constexpr int kDelayMask = kDelayBufferSize - 1;

    VirtuosoStradivari(double sr = 44100.0) : m_sampleRate(sr) {
        reset();
        // Pre-calculate Note to Freq LUT
        for (int i = 0; i < 128; ++i) {
            m_noteToFreq[i] = 440.0f * std::pow(2.0f, (i - 69.0f) / 12.0f);
        }
    }

    void prepareToPlay(double sr, uint32_t /*blockSize*/) noexcept override { m_sampleRate = sr; }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& /*context*/) noexcept override {
        // 1. MIDI Trigger
        for (const auto& ev : midi) {
            if ((ev.data[0] & 0xF0) == 0x90 && ev.data[2] > 0) triggerNote(ev.data[1], ev.data[2] / 127.0f);
        }

        // 2. Physical Modeling Engine Loop
        float* outL = buffer.getWritePointer(0);
        float* outR = buffer.getWritePointer(1);
        uint32_t numSamples = buffer.getNumSamples();

        for (uint32_t s = 0; s < numSamples; ++s) {
            float sum = 0.0f;
            for (auto& v : m_voices) {
                if (v.envelope <= 0.0001f) continue;

                // --- WAVEGUIDE MODEL ---
                float delaySamples = m_sampleRate / v.frequency;
                int intDelay = static_cast<int>(delaySamples);
                float frac = delaySamples - intDelay;

                // Bitwise AND mask (Fast)
                int readIdx = (v.writeIdx - intDelay) & kDelayMask;
                int readIdxNext = (readIdx + 1) & kDelayMask;
                
                // Read from feedback loop (Linear interpolation)
                float val = v.buffer[readIdx] * (1.0f - frac) + v.buffer[readIdxNext] * frac;

                // Excitation (XORShift noise)
                float noise = (fastRand() / (float)0xFFFFFFFF) - 0.5f;
                float excitation = noise * v.envelope * 0.1f;
                
                // Feedback with Loss
                float nextVal = (val + excitation) * 0.9994f;
                v.buffer[v.writeIdx] = nextVal;
                v.writeIdx = (v.writeIdx + 1) & kDelayMask;

                sum += nextVal;
                v.envelope *= 0.99985f; // Slightly slower decay
            }
            outL[s] += sum;
            outR[s] += sum; 
        }
    }

    void reset() noexcept override {
        for (auto& v : m_voices) {
            v.buffer.fill(0.0f);
            v.envelope = 0.0f;
            v.writeIdx = 0;
        }
    }

private:
    struct Voice {
        std::array<float, kDelayBufferSize> buffer;
        uint32_t writeIdx = 0;
        float frequency = 440.0f;
        float envelope = 0.0f;
        uint8_t note = 0;
    };

    uint32_t fastRand() {
        m_randState ^= m_randState << 13;
        m_randState ^= m_randState >> 17;
        m_randState ^= m_randState << 5;
        return m_randState;
    }

    void triggerNote(uint8_t note, float velocity) {
        Voice& v = m_voices[m_nextVoice++ & (kMaxPolyphony - 1)];
        v.note = note;
        v.frequency = m_noteToFreq[note & 127];
        v.envelope = velocity;
        v.writeIdx = 0;
    }

    double m_sampleRate;
    std::array<Voice, kMaxPolyphony> m_voices;
    std::array<float, 128> m_noteToFreq;
    uint32_t m_nextVoice = 0;
    uint32_t m_randState = 0x5EED;
};

} // namespace Aura::DSP::Synthesis
