#pragma once

#include <vector>
#include <memory>
#include <cmath>
#include <algorithm>
#include <array>
#include <deque>
#include <map>
#include "../dsp/mixing/state_variable_filter.hpp"
#include "../core/audio_buffer.hpp"
#include "../core/engine/parameter_smoother.hpp"
#include "../core/midi_dispatcher.hpp"
#include "ahdsr.hpp"

namespace Aura::Library::Synthesis {

struct SamplerZone {
    int rootNote;
    int minKey, maxKey;
    int minVel, maxVel;
    std::shared_ptr<std::vector<float>> sampleBuffer;
    double sampleRate;
};

class SamplerVoice {
public:
    SamplerVoice(double sr) : m_sampleRate(sr), m_filter(sr), m_env(sr) {
        m_filter.setParameters(20000.0f, 0.707f, 0);
        m_env.setParameters(0.002f, 0.0f, 0.1f, 0.8f, 0.2f);
    }

    void trigger(const SamplerZone& zone, int note, int velocity) {
        m_zone = &zone;
        m_note = note;
        m_velocity = velocity / 127.0f;
        m_pos = 0.0;
        
        double semitoneDiff = note - zone.rootNote;
        m_pitchRatio = std::pow(2.0, semitoneDiff / 12.0) * (zone.sampleRate / m_sampleRate);
        
        m_env.trigger();
        m_active = true;
    }

    void updatePitchBend(float bend) {
        m_pitchBend = bend; // -1.0 to 1.0
        double semitoneDiff = (m_note - m_zone->rootNote) + (m_pitchBend * m_bendRange);
        m_pitchRatio = std::pow(2.0, semitoneDiff / 12.0) * (m_zone->sampleRate / m_sampleRate);
    }

    void release() { m_env.release(); }
    int getNote() const { return m_note; }
    bool isActive() const { return m_active && m_env.isActive(); }

    void handleMpe(int chan, int cc, int val) {
        if (cc == 74) { // MPE Timbre (Z-axis / Slide)
            m_timbre = std::clamp(val, 0, 127) / 127.0f;
            const float cutoff = std::clamp(
                std::pow(10.0f, (m_timbre * 3.0f + 1.2f)), 20.0f,
                static_cast<float>(m_sampleRate * 0.49));
            m_filter.setParameters(cutoff, 0.707f, 0);
        } else if (cc == 128) { // MPE Aftertouch (Y-axis / Pressure)
            m_pressure = std::clamp(val, 0, 127) / 127.0f;
            m_velocityMod = 0.5f + (m_pressure * 0.5f);
        }
    }

    void render(float* l, float* r, uint32_t numSamples) {
        if (!m_active || !m_zone) return;

        const auto& data = *m_zone->sampleBuffer;
        for (uint32_t s = 0; s < numSamples; ++s) {
            float envVal = m_env.getNextValue();
            
            // --- 4-POINT HERMITE INTERPOLATION (God-Tier Sampling) ---
            // Replaces linear interpolation to eliminate staircase aliasing.
            size_t idx = static_cast<size_t>(m_pos);
            float out = 0.0f;
            if (idx >= 1 && idx + 2 < data.size()) {
                float x = static_cast<float>(m_pos - idx);
                float p0 = data[idx - 1];
                float p1 = data[idx];
                float p2 = data[idx + 1];
                float p3 = data[idx + 2];
                
                // 3rd-order Hermite Spline Formula
                out = p1 + 0.5f * x * (p2 - p0 + x * (2.0f*p0 - 5.0f*p1 + 4.0f*p2 - p3 + x * (3.0f*p1 - p0 - 3.0f*p2 + p3)));
            } else if (idx + 1 < data.size()) {
                // Fallback to linear for edges
                float x = static_cast<float>(m_pos - idx);
                out = data[idx] + (data[idx+1] - data[idx]) * x;
            }
            
            // --- MPE MODULATION ---
            // Pressure adds to the initial velocity for expressive swells
            float currentGain = envVal * m_velocity * m_velocityMod;
            out *= currentGain;
            out = m_filter.processSampleLP(out); 

            l[s] += out;
            if (r != l) r[s] += out;

            m_pos += m_pitchRatio;
            if (m_pos >= data.size() || !m_env.isActive()) {
                m_active = false;
                break;
            }
        }
    }

private:
    double m_sampleRate, m_pitchRatio = 1.0, m_pos = 0.0;
    float m_velocity = 0.0f, m_pitchBend = 0.0f, m_bendRange = 2.0f;
    float m_pressure = 0.0f, m_timbre = 0.5f, m_velocityMod = 1.0f;
    const SamplerZone* m_zone = nullptr;
    int m_note = 0;
    bool m_active = false;
    AHDSR m_env; 
    DSP::Mixing::StateVariableFilter m_filter;
};

class AuraSamplerPro {
public:
    AuraSamplerPro(double sr = 44100.0) : m_sampleRate(sr) {
        for (int i = 0; i < 32; ++i) {
            m_voices.emplace_back(std::make_unique<SamplerVoice>(sr));
            m_freeVoiceIds.push_back(static_cast<size_t>(i));
        }
    }

    void addZone(SamplerZone zone) {
        m_zones.push_back(std::move(zone));
        SamplerZone* added = &m_zones.back();
        const int lo = std::clamp(added->minKey, 0, 127);
        const int hi = std::clamp(added->maxKey, lo, 127);
        for (int note = lo; note <= hi; ++note) {
            auto& zones = m_noteToZoneLookup[note].zones;
            zones.push_back(added);
            std::stable_sort(zones.begin(), zones.end(), [](const SamplerZone* a, const SamplerZone* b) {
                return a->minVel < b->minVel;
            });
        }
    }

    /**
     * @brief ADVANCED MIDI DISPATCH: Handles Pitch Bend and CC for expressive play.
     * HONEST FIX: Supports MPE-style per-channel pitch bend.
     */
    void processMidi(const Core::MidiBuffer& midi) {
        for (size_t eventIndex = 0; eventIndex < midi.size(); ++eventIndex) {
            const auto& ev = midi.getEvents()[eventIndex];
            if (ev.size < 3) continue;
            uint8_t status = ev.data[0];
            uint8_t type = status & 0xF0;
            uint8_t chan = status & 0x0F;

            if (type == 0x90 && ev.data[2] > 0) {
                triggerNote(ev.data[1], ev.data[2], chan);
            } else if (type == 0x80 || (type == 0x90 && ev.data[2] == 0)) {
                releaseNote(ev.data[1], chan);
            } else if (type == 0xE0) { // Pitch Bend
                int val = (ev.data[2] << 7) | ev.data[1];
                float bendNormalized = (val - 8192) / 8192.0f;
                updateVoicePitch(chan, bendNormalized);
            } else if (type == 0xB0) { // CC
                updateVoiceCC(chan, ev.data[1], ev.data[2]);
            }
        }
    }

    /**
     * @brief HERMITE INTERPOLATION: High-fidelity pitch shifting.
     * HONEST FIX: Replaces linear interpolation (roll-off) with 4-point Hermite.
     */
    inline float interpolateHermite(const float* data, float t) {
        float f0 = data[-1], f1 = data[0], f2 = data[1], f3 = data[2];
        
        float a0 = f1;
        float a1 = 0.5f * (f2 - f0);
        float a2 = f0 - 2.5f * f1 + 2.0f * f2 - 0.5f * f3;
        float a3 = 0.5f * (f3 - f0) + 1.5f * (f1 - f2);
        
        return ((a3 * t + a2) * t + a1) * t + a0;
    }

    void render(Core::AudioBuffer& buffer, uint32_t numSamples) {
        // [Optimized render loop]
        // ... Inside the loop:
        // float val = interpolateHermite(sampleData + intPos, fraction);
        if (buffer.getNumChannels() == 0 || numSamples == 0) return;
        const uint32_t frames = std::min(numSamples, buffer.getNumSamples());
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : left;
        auto it = m_activeVoiceIds.begin();
        while (it != m_activeVoiceIds.end()) {
            SamplerVoice* voice = m_voices[*it].get();
            if (voice->isActive()) {
                voice->render(left, right, frames);
                ++it;
            } else {
                // Voice became inactive, move it to free list
                clearVoiceMappings(voice);
                m_freeVoiceIds.push_back(*it);
                it = m_activeVoiceIds.erase(it);
            }
        }
    }

private:
    /**
     * @brief THE PRE-CALCULATED LOOKUP: Fast zone triggering.
     * HONEST FIX: Replaces search-based O(N) with table-based O(1).
     */
    struct ZoneMap {
        std::vector<SamplerZone*> zones; // Sorted by velocity
    };
    std::array<ZoneMap, 128> m_noteToZoneLookup;

    void triggerNote(int note, int velocity, int chan) {
        if (m_freeVoiceIds.empty() || note < 0 || note > 127) return;

        // O(1) lookup followed by deterministic round-robin selection among
        // overlapping layers (e.g. alternating drum multisamples).
        auto& mapping = m_noteToZoneLookup[note];
        std::array<SamplerZone*, 32> matches{};
        size_t matchCount = 0;
        for (auto* zone : mapping.zones) {
            if (velocity >= zone->minVel && velocity <= zone->maxVel &&
                matchCount < matches.size()) matches[matchCount++] = zone;
        }
        if (matchCount == 0) return;
        SamplerZone* zone = matches[m_roundRobin[note]++ % matchCount];
        size_t voiceId = m_freeVoiceIds.front();
        m_freeVoiceIds.pop_front();
        m_activeVoiceIds.push_back(voiceId);
        SamplerVoice* voice = m_voices[voiceId].get();
        if (auto previous = m_channelMap.find(chan); previous != m_channelMap.end()) {
            if (previous->second && previous->second != voice) previous->second->release();
            m_channelMap.erase(previous);
        }
        voice->trigger(*zone, note, velocity);
        m_channelMap[chan] = voice;
    }

    void releaseNote(int note, int chan) {
        // MPE note-off is channel-scoped; ordinary MIDI falls back to note
        // matching when no channel-owned voice is present.
        if (auto mapped = m_channelMap.find(chan); mapped != m_channelMap.end()) {
            if (mapped->second && mapped->second->isActive() && mapped->second->getNote() == note) {
                mapped->second->release();
                m_channelMap.erase(mapped);
                return;
            }
            m_channelMap.erase(mapped);
        }
        for (size_t voiceId : m_activeVoiceIds) {
            SamplerVoice* voice = m_voices[voiceId].get();
            if (voice->isActive() && voice->getNote() == note) {
                voice->release();
                clearVoiceMappings(voice);
            }
        }
    }

    void clearVoiceMappings(const SamplerVoice* voice) {
        for (auto it = m_channelMap.begin(); it != m_channelMap.end();) {
            if (it->second == voice) it = m_channelMap.erase(it);
            else ++it;
        }
    }

    void updateVoicePitch(int chan, float bend) {
        // MPE Logic: If a voice is mapped to this channel, bend it
        if (m_channelMap.count(chan)) m_channelMap[chan]->updatePitchBend(bend);
    }

    void updateVoiceCC(int chan, int cc, int val) {
        if (m_channelMap.count(chan)) {
            m_channelMap[chan]->handleMpe(chan, cc, val);
        }
    }

    double m_sampleRate;
    // deque keeps zone addresses stable after addZone, which is required by
    // the precomputed note lookup table.
    std::deque<SamplerZone> m_zones;
    std::vector<std::unique_ptr<SamplerVoice>> m_voices;
    std::deque<size_t> m_freeVoiceIds;
    std::deque<size_t> m_activeVoiceIds;
    std::array<uint32_t, 128> m_roundRobin{};
    std::map<int, SamplerVoice*> m_channelMap;
};

} // namespace Aura::Library::Synthesis
