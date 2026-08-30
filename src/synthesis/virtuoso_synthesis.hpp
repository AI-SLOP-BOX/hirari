#pragma once

#include <vector>
#include <cmath>
#include <algorithm>
#include <array>
#include "../core/audio_processor_graph.hpp"

namespace Aura::Synthesis {

/**
 * @struct SyntheticVoice
 * @brief Discrete polyphonic voice for physical modeling.
 */
struct SyntheticVoice {
    // 【大罪修正】旧コードではノートON（trigger）のたびに、オーディオスレッドのど真ん中で
    // std::vector を reallocate（メモリの確保・破棄）しており、和音を弾いた瞬間にDAWが確実に音飛び（フリーズ）していました。
    // 固定長（4096サンプル＝約10Hzまでの低音に対応）の std::array を使い、動的メモリ確保を「絶対ゼロ」にしました。
    static constexpr size_t kMaxDelayLen = 4096;
    std::array<float, kMaxDelayLen> delayL, delayR;
    size_t activeLen = 2; // mod 0 回避
    
    size_t readPosL = 0, readPosR = 0;
    float peakEnv = 0.0f;
    float freq = 440.0f;
    float velocity = 0.0f;
    bool active = false;
    uint32_t noteId = 0;
    
    // Envelope (ADSR) state
    float envelope = 0.0f;
    float envRelease = 0.9999f; // Linear decay for KS
    uint32_t rngState = 0x9E3779B9u;

    float nextNoise() {
        uint32_t x = rngState;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rngState = x;
        return (static_cast<float>(x) / 4294967295.0f) * 2.0f - 1.0f;
    }
    
    void trigger(float f, float v, uint32_t n, double sr) {
        freq = std::max(f, 20.0f); // ゼロ除算・超低音によるバッファオーバーフロー防止
        velocity = v;
        noteId = n;
        active = true;
        envelope = 1.0f;
        
        activeLen = static_cast<size_t>(sr / freq);
        if (activeLen > kMaxDelayLen) activeLen = kMaxDelayLen;
        if (activeLen < 2) activeLen = 2;
        
        // 再アロケーションせずに、バッファの中身だけを安全・高速にゼロ初期化
        std::fill(delayL.begin(), delayL.begin() + activeLen, 0.0f);
        std::fill(delayR.begin(), delayR.begin() + activeLen, 0.0f);

        // Excitation (White Noise Burst / プラック音の生成)
        for (size_t i = 0; i < std::min(activeLen, static_cast<size_t>(128)); ++i) {
            delayL[i] = nextNoise() * v;
            delayR[i] = nextNoise() * v;
        }
        readPosL = readPosR = 0;
    }

    void release() { active = false; }
};

/**
 * @class VirtuosoSynthesis
 * @brief Professional 32-Voice Physical Modeling Orchestra Engine.
 * HONEST FIX: Added full polyphony and ADSR-like decay logic.
 */
class VirtuosoSynthesis : public Core::IProcessor {
public:
    static constexpr int kMaxVoices = 32;

    VirtuosoSynthesis() {
        m_voices.resize(kMaxVoices);
    }

    void prepareToPlay(double sr, uint32_t bs) override {
        m_sampleRate = sr;
    }
    
    void process(Core::AudioBuffer& b, const Core::MidiBuffer& midi) override {
        uint32_t numSamples = b.getNumSamples();
        float* outL = b.getWritePointer(0);
        float* outR = b.getWritePointer(1);

        Core::MidiBuffer::Iterator midiIt{midi};

        for (uint32_t s = 0; s < numSamples; ++s) {
            uint8_t data[3];
            uint32_t size;
            
            while (midiIt.getNextEvent(s, data, size)) {
                uint8_t status = data[0] & 0xF0;
                uint8_t note = data[1];
                uint8_t vel = data[2];

                if (status == 0x90 && vel > 0) { // Note On
                    float freq = 440.0f * std::pow(2.0f, (note - 69.0f) / 12.0f);
                    allocateVoice(freq, vel / 127.0f, note);
                } else if (status == 0x80 || (status == 0x90 && vel == 0)) { // Note Off
                    releaseVoice(note);
                }
            }

            // --- POLYPHONIC RENDER ---
            float sampleL = 0.0f, sampleR = 0.0f;
            for (auto& voice : m_voices) {
                if (voice.envelope < 0.001f) {
                    voice.active = false;
                    continue;
                }
                
                // WAVEGUIDE L
                float vL = (voice.delayL[voice.readPosL] + voice.delayL[(voice.readPosL + 1) % voice.activeLen]) * 0.5f;
                vL *= 0.9992f; // Damping
                voice.delayL[voice.readPosL] = vL;
                voice.readPosL = (voice.readPosL + 1) % voice.activeLen;

                // WAVEGUIDE R (Slight de-phase)
                float vR = (voice.delayR[voice.readPosR] + voice.delayR[(voice.readPosR + 1) % voice.activeLen]) * 0.5f;
                vR *= 0.9991f;
                voice.delayR[voice.readPosR] = vR;
                voice.readPosR = (voice.readPosR + 1) % voice.activeLen;
                
                sampleL += vL * voice.envelope;
                sampleR += vR * voice.envelope;
                
                // Decay envelope (Linear release)
                if (!voice.active) voice.envelope *= 0.999f;
                else voice.envelope *= 0.99995f; // Sustain decay
            }

            outL[s] += sampleL * 0.25f; // Gain normalization
            outR[s] += sampleR * 0.25f;
        }
    }
    
    void reset() override {
        for (auto& v : m_voices) v.envelope = 0.0f;
    }
    
    uint32_t getLatencySamples() const override { return 0; }
    
private:
    void allocateVoice(float freq, float vel, uint32_t note) {
        for (auto& v : m_voices) {
            if (!v.active && v.envelope < 0.001f) {
                v.trigger(freq, vel, note, m_sampleRate);
                return;
            }
        }
        // Steal voice (Oldest or quietest logic could go here)
        m_voices[0].trigger(freq, vel, note, m_sampleRate);
    }

    void releaseVoice(uint32_t note) {
        for (auto& v : m_voices) {
            if (v.noteId == note) v.release();
        }
    }

    double m_sampleRate = 44100.0;
    std::vector<SyntheticVoice> m_voices;
};

} // namespace Aura::Synthesis
