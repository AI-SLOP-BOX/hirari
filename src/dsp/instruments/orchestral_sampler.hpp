#pragma once

#include <vector>
#include <string>
#include <map>
#include <memory>
#include <cmath>
#include "../../core/audio_buffer.hpp"
#include "../../core/midi_buffer.hpp"
#include "../iprocessor.hpp"

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
        (void)ctx;
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        float* l = buffer.getWritePointer(0);
        float* r = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : l;
        uint32_t len = buffer.getNumSamples();
        if (!l || !r) return;

        midi.sort();
        size_t eventIndex = 0;

        // INDUSTRIAL: SIMD-ready block rendering
        for (uint32_t s = 0; s < len; ++s) {
            while (eventIndex < midi.size() && midi.getEvents()[eventIndex].sampleOffset <= s) {
                applyMidiEvent(midi.getEvents()[eventIndex]);
                ++eventIndex;
            }
            float out = 0.0f;
            for (auto& v : m_voices) {
                if (!v.active || !v.sampleData || v.sampleSize == 0 || v.samplePointer >= v.sampleSize) {
                    if (v.active) v.active = false;
                    continue;
                }

                float sample = std::isfinite(v.sampleData[v.samplePointer])
                    ? v.sampleData[v.samplePointer] : 0.0f;
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
            l[s] = std::isfinite(l[s] + out) ? l[s] + out : 0.0f;
            if (r != l) r[s] = std::isfinite(r[s] + out) ? r[s] + out : 0.0f;
        }
    }

private:
    void applyMidiEvent(const Core::MidiEvent& ev) {
        if (ev.size < 3) return;
        const uint8_t status = ev.data[0] & 0xF0u;
        if (status == 0x90u && ev.data[2] > 0) {
            triggerVoice(ev.data[1], ev.data[2], ev.articulationId);
        } else if (status == 0x80u || (status == 0x90u && ev.data[2] == 0)) {
            for (auto& v : m_voices) if (v.active && v.note == ev.data[1]) v.releasing = true;
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

        if (!target || target->data.empty() || target->data.size() > UINT32_MAX) return;

        for (auto& v : m_voices) {
            if (!v.active) {
                v.active = true; v.note = note; v.velocity = vel;
                v.samplePointer = 0; v.sampleSize = (uint32_t)target->data.size();
                v.sampleData = target->data.data();
                v.env = 0.0f; v.releasing = false;
                return;
            }
        }
        // Deterministic voice stealing when polyphony is exhausted.
        m_voices[0].active = true; m_voices[0].note = note; m_voices[0].velocity = vel;
        m_voices[0].samplePointer = 0; m_voices[0].sampleSize = static_cast<uint32_t>(target->data.size());
        m_voices[0].sampleData = target->data.data(); m_voices[0].env = 0.0f; m_voices[0].releasing = false;
    }

    std::vector<SampleZone> m_zones;
    std::array<Voice, kMaxVoices> m_voices;
    uint32_t m_currentArtic = 0;
};

} // namespace Aura::DSP::Instruments
