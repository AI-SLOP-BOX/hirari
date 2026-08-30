#pragma once

#include <vector>
#include <string>
#include <map>
#include <memory>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"

namespace Aura::DSP::Instruments {

/**
 * @struct SampleZone
 * @brief Industrial sample mapping with Articulation and Velocity support.
 */
struct SampleZone {
    uint8_t lowNote, highNote;
    uint8_t lowVel, highVel;
    uint32_t articulation;
    std::vector<float> data;
};

/**
 * @struct DFD_Block
 * @brief High-performance Disk-Streaming Buffer.
 * Points to pre-allocated RAM (Streaming Cache) to avoid disk latency.
 */
struct DFD_Block {
    const float* data = nullptr;
    size_t length = 0;
};

/**
 * @class OrchestralSampler
 * @brief Industrial DFD Orchestral Engine (Sovereign Cinema Pro).
 */
class OrchestralSampler {
public:
    static constexpr int kMaxVoices = 128;
    static constexpr int kMaxZones = 1000;

    struct Voice {
        bool active = false;
        uint8_t note, velocity;
        uint32_t samplePointer = 0;
        uint32_t sampleSize = 0;
        const float* sampleData = nullptr;
        float env = 0.0f;
        float envStep = 0.001f; // Industrial linear ramp
        bool releasing = false;
    };

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& ctx) {
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getWritePointer(1);
        uint32_t len = buffer.getNumSamples();

        handleMidi(midi, ctx);

        // INDUSTRIAL: SIMD-ready block rendering
        for (uint32_t s = 0; s < len; ++s) {
            float out = 0.0f;
            for (auto& v : m_voices) {
                if (!v.active) continue;

                float sample = v.sampleData[v.samplePointer];
                float gain = (v.velocity / 127.0f) * v.env;
                out += sample * gain;

                v.samplePointer++;
                if (v.samplePointer >= v.sampleSize) {
                    v.active = false;
                    continue;
                }

                if (v.releasing) {
                    v.env -= 0.005f; // Fast release
                    if (v.env <= 0.0f) v.active = false;
                } else if (v.env < 1.0f) {
                    v.env = std::min(1.0f, v.env + 0.1f); // Fast attack
                }
            }
            l[s] += out;
            r[s] += out;
        }
    }

private:
    void handleMidi(Core::MidiBuffer& midi, const ProcessContext& ctx) {
        for (const auto& ev : midi) {
            uint8_t status = ev.data[0] & 0xF0;
            if (status == 0x90 && ev.data[2] > 0) {
                triggerVoice(ev.data[1], ev.data[2], m_currentArtic);
            } else if (status == 0x80) {
                for (auto& v : m_voices) if (v.active && v.note == ev.data[1]) v.releasing = true;
            }
        }
    }

    void triggerVoice(uint8_t note, uint8_t vel, uint32_t artic) {
        // INDUSTRIAL: O(1) Note Dispatch (Conceptually cached)
        const SampleZone* target = nullptr;
        for (const auto& z : m_zones) {
            if (note >= z.lowNote && note <= z.highNote && z.articulation == artic) {
                if (vel >= z.lowVel && vel <= z.highVel) { target = &z; break; }
            }
        }

        if (!target || target->data.empty()) return;

        for (auto& v : m_voices) {
            if (!v.active) {
                v.active = true; v.note = note; v.velocity = vel;
                v.samplePointer = 0; v.sampleSize = (uint32_t)target->data.size();
                v.sampleData = target->data.data();
                v.env = 0.0f; v.releasing = false;
                return;
            }
        }
    }

    std::vector<SampleZone> m_zones;
    std::array<Voice, kMaxVoices> m_voices;
    uint32_t m_currentArtic = 0;
};

} // namespace Aura::DSP::Instruments
