#pragma once

#include <array>
#include <algorithm>
#include <cmath>
#include <limits>
#include "../utils/dsp_utils.hpp"
#include "../../core/audio_buffer.hpp"
#include "../../core/audio_types.hpp"
#include "../iprocessor.hpp"

namespace Aura::DSP::Synthesis {

/**
 * @class SamplerEngine
 * @brief High-performance Multi-Voice Sample Playback Engine (HARDENED).
 * OPTIMIZATIONS:
 * - O(1) Voice Allocation via Free Stack.
 * - Zero-Atomic noteOn (uses raw pointers).
 * - LOD Interpolation (Hermite vs Linear based on level).
 * - Telemetry for streaming state visualization.
 */
class SamplerEngine : public IProcessor {
public:
    struct Voice {
        bool active = false;
        double position = 0.0;
        double pitchRatio = 1.0;
        double basePitchRatio = 1.0;
        double sampleRateRatio = 1.0;
        float velocity = 1.0f;
        float envLevel = 0.0f;
        uint32_t envState = 0; // 0=Idle, 1=Attack, 2=Decay, 3=Sustain, 4=Release
        uint8_t note = 0;
        uint8_t channel = 0;
        Core::AudioBuffer* sample = nullptr; // RAW POINTER for speed (Managed by Library)
        const float* rawSample = nullptr;
        uint32_t rawSampleLength = 0;
        bool isStreaming = false; // For UI Telemetry
        float brightness = 1.0f; // LOD hint
        bool keyReleased = false; // held by sustain pedal until CC64 up
        float pressureGain = 1.0f;
        uint32_t loopStart = 0;
        uint32_t loopEnd = 0;
        bool loopEnabled = false;
    };

    SamplerEngine(double sr = 44100.0)
        : m_sampleRate(std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0 ? sr : 44'100.0) {
        m_voices.resize(64);
        m_channelVolume.fill(1.0f);
        m_channelExpression.fill(1.0f);
        m_zoneRoundRobin.fill(0);
        resetFreeVoices();
    }

    void noteOn(uint8_t note, uint8_t velocity, Core::AudioBuffer* sample, uint8_t rootNote = 60,
                double sourceSampleRate = 0.0) {
        if (!sample) return;
        
        int voiceIdx = -1;
        if (hasFreeVoice()) {
            voiceIdx = static_cast<int>(popFreeVoice());
        } else {
            // Priority-based stealing
            voiceIdx = findVoiceToSteal();
        }

        if (voiceIdx >= 0) {
            startVoice(m_voices[voiceIdx], note, velocity, sample, rootNote, sourceSampleRate);
        }
    }

    void addZone(const Core::SamplerZone& zone) {
        Core::SamplerZone normalized = zone;
        if (normalized.sourceSampleRate == 0.0) normalized.sourceSampleRate = normalized.buffer.sampleRate;
        if (normalized.lowKey > normalized.highKey || normalized.lowVelocity > normalized.highVelocity ||
            !normalized.buffer.data || normalized.buffer.length == 0 || m_zones.size() >= 256 ||
            (normalized.loopEnabled && (normalized.loopStart >= normalized.loopEnd || normalized.loopEnd > normalized.buffer.length)) ||
            (normalized.sourceSampleRate != 0.0 &&
             (!std::isfinite(normalized.sourceSampleRate) || normalized.sourceSampleRate < 8'000.0 ||
              normalized.sourceSampleRate > 384'000.0))) return;
        m_zones.push_back(normalized);
        std::stable_sort(m_zones.begin(), m_zones.end(),
            [](const Core::SamplerZone& lhs, const Core::SamplerZone& rhs) {
                if (lhs.lowKey != rhs.lowKey) return lhs.lowKey < rhs.lowKey;
                if (lhs.highKey != rhs.highKey) return lhs.highKey < rhs.highKey;
                return lhs.lowVelocity < rhs.lowVelocity;
            });
    }

    void NoteOff(uint8_t note, uint8_t channel = 0xFFu) {
        for (auto& v : m_voices) {
            if (v.active && v.note == note && (channel == 0xFFu || v.channel == channel)) {
                if (channel < 16u ? m_sustainPedal[channel] : anySustainHeld()) v.keyReleased = true;
                else v.envState = 4;
            }
        }
    }

    bool anySustainHeld() const noexcept {
        for (const bool held : m_sustainPedal) if (held) return true;
        return false;
    }

    void setSustain(uint8_t channel, bool held) {
        if (channel >= 16u || m_sustainPedal[channel] == held) return;
        m_sustainPedal[channel] = held;
        if (!held) {
            for (auto& v : m_voices) {
                if (v.active && v.channel == channel && v.keyReleased) {
                    v.keyReleased = false;
                    v.envState = 4;
                }
            }
        }
    }

    void process(Core::AudioBuffer& buffer) {
        if (buffer.isEmpty()) return;

        uint32_t numSamples = buffer.getNumSamples();
        uint32_t numChannels = buffer.getNumChannels();
        float* outL = buffer.getWritePointer(0);
        float* outR = numChannels > 1 ? buffer.getWritePointer(1) : nullptr;
        if (!outL) return;

        // Clear output buffer first
        std::fill(outL, outL + numSamples, 0.0f);
        if (outR) std::fill(outR, outR + numSamples, 0.0f);

        for (size_t i = 0; i < m_voices.size(); ++i) {
            auto& v = m_voices[i];
            if (!v.active || (!v.sample && (!v.rawSample || v.rawSampleLength == 0))) continue;

            const uint32_t sampleSamples = v.sample ? v.sample->getNumSamples() : v.rawSampleLength;
            const uint32_t sampleChannels = v.sample ? v.sample->getNumChannels() : 1;
            const float* sampleL = v.sample ? v.sample->getReadPointer(0) : v.rawSample;
            const float* sampleR = v.sample && sampleChannels > 1 ? v.sample->getReadPointer(1) : sampleL;

            for (uint32_t s = 0; s < numSamples; ++s) {
                if (v.envState == 0 || !v.active) {
                    v.active = false;
                    v.envState = 0;
                    v.sample = nullptr; // Prevent double pushes
                    v.rawSample = nullptr;
                    pushFreeVoice(static_cast<uint32_t>(i));
                    break;
                }

                double pos = v.position;
                size_t idx0 = static_cast<size_t>(pos);

                const bool looping = v.loopEnabled && v.loopEnd > v.loopStart + 1 &&
                                     v.loopEnd <= sampleSamples;
                if (looping && pos >= static_cast<double>(v.loopEnd)) {
                    const double loopLength = static_cast<double>(v.loopEnd - v.loopStart);
                    v.position = static_cast<double>(v.loopStart) +
                                 std::fmod(std::max(0.0, pos - v.loopStart), loopLength);
                    pos = v.position;
                    idx0 = static_cast<size_t>(pos);
                } else if (idx0 >= sampleSamples) {
                    v.active = false;
                    v.envState = 0;
                    v.sample = nullptr; // Prevent double pushes
                    v.rawSample = nullptr;
                    pushFreeVoice(static_cast<uint32_t>(i));
                    break;
                }

                // Fetch a finite, loop-aware cubic-interpolated sample.
                float sL = interpolateSample(sampleL, sampleSamples, pos, looping, v.loopStart, v.loopEnd);
                float sR = interpolateSample(sampleR, sampleSamples, pos, looping, v.loopStart, v.loopEnd);

                // Update envelope step
                updateEnvelope(v);
                float amp = v.envLevel * v.velocity;
                amp *= m_channelVolume[v.channel] * m_channelExpression[v.channel] * v.pressureGain;
                sL *= amp;
                sR *= amp;

                outL[s] += std::isfinite(sL) ? sL : 0.0f;
                if (outR) outR[s] += std::isfinite(sR) ? sR : 0.0f;

                // Advance pitch-scaled position
                v.position += v.pitchRatio * v.sampleRateRatio;
            }
        }
    }


    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        (void)bs;
        // Keep runtime reconfiguration within the same range as the
        // constructor; an accidental megahertz-rate would otherwise make
        // pitch/envelope behaviour numerically unstable.
        if (std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0) m_sampleRate = sr;
        reset();
    }

    void reset() noexcept override {
        m_sustainPedal.fill(false);
        m_channelVolume.fill(1.0f);
        m_channelExpression.fill(1.0f);
        m_zoneRoundRobin.fill(0);
        for (auto& v : m_voices) {
            v.active = false;
            v.envState = 0;
            v.channel = 0;
            v.basePitchRatio = 1.0;
            v.pitchRatio = 1.0;
            v.sampleRateRatio = 1.0;
            v.keyReleased = false;
            v.loopStart = 0;
            v.loopEnd = 0;
            v.loopEnabled = false;
            v.sample = nullptr;
            v.rawSample = nullptr;
            v.rawSampleLength = 0;
        }
        resetFreeVoices();
    }

    std::vector<bool> getStreamingHealth() const override {
        std::vector<bool> health;
        for (const auto& v : m_voices) {
            if (v.active) health.push_back(v.isStreaming);
        }
        return health;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ProcessContext& context) noexcept override {
        (void)context;
        if (isBypassed()) return;

        // --- INDUSTRIAL MIDI DISPATCHER + ZONE LOOKUP ---
        for (const auto& event : midi) {
            // MIDI 2.0 channel-voice UMP (64-bit payload in a 128-bit
            // container) uses a 0x4 message type.  The sampler keeps its
            // internal voice contract at 7-bit velocity, so consume the
            // high byte of the 16-bit velocity while preserving note timing.
            if (event.size == 16 && (event.data[0] >> 4u) == 0x4u) {
                const uint8_t umpStatus = event.data[1] & 0xF0u;
                const uint8_t umpNote = event.data[2] & 0x7Fu;
                const uint8_t umpVelocity = event.data[4];
                if (umpStatus == 0x90u && umpVelocity != 0) {
                    const auto zone = findZone(umpNote, umpVelocity);
                    if (zone != m_zones.end()) {
                        noteOnRaw(umpNote, umpVelocity, zone->buffer.data,
                                  static_cast<uint32_t>(std::min<size_t>(zone->buffer.length, UINT32_MAX)),
                                  zone->rootKey, event.data[1] & 0x0Fu,
                                  zone->loopStart, zone->loopEnd, zone->loopEnabled,
                                  zone->sourceSampleRate);
                    }
                } else if (umpStatus == 0x80u || (umpStatus == 0x90u && umpVelocity == 0)) {
                    NoteOff(umpNote, event.data[1] & 0x0Fu);
                } else if (umpStatus == 0xB0u && (event.data[2] & 0x7Fu) == 64u) {
                    setSustain(event.data[1] & 0x0Fu, umpVelocity >= 64u);
                } else if (umpStatus == 0xB0u && ((event.data[2] & 0x7Fu) == 7u ||
                                                   (event.data[2] & 0x7Fu) == 11u)) {
                    setChannelExpression(event.data[1] & 0x0Fu, event.data[2] & 0x7Fu,
                                         umpVelocity);
                } else if (umpStatus == 0xA0u) {
                    setVoicePressure(event.data[1] & 0x0Fu, event.data[2] & 0x7Fu,
                                     umpVelocity);
                } else if (umpStatus == 0xD0u) {
                    setChannelPressure(event.data[1] & 0x0Fu, umpVelocity);
                } else if (umpStatus == 0xE0u) {
                    const float bend = (static_cast<float>(event.data[4]) - 128.0f) / 128.0f;
                    updateVoicePitch(event.data[1] & 0x0Fu, bend);
                }
                continue;
            }
            if (event.size < 2 || event.data[0] < 0x80) continue;
            uint8_t status = event.data[0] & 0xF0;
            uint8_t pitch  = event.data[1];
            uint8_t vel    = event.size >= 3 ? event.data[2] : 0;

            if (status == 0x90 && vel > 0) {
                const auto zone = findZone(pitch, vel);
                if (zone != m_zones.end()) noteOnRaw(pitch, vel, zone->buffer.data,
                    static_cast<uint32_t>(std::min<size_t>(zone->buffer.length, UINT32_MAX)),
                    zone->rootKey, event.data[0] & 0x0Fu,
                    zone->loopStart, zone->loopEnd, zone->loopEnabled,
                    zone->sourceSampleRate);
            } else if (status == 0x80 || (status == 0x90 && vel == 0)) {
                NoteOff(pitch, event.data[0] & 0x0Fu);
            } else if (status == 0xB0 && pitch == 64) {
                setSustain(event.data[0] & 0x0Fu, vel >= 64);
            } else if (status == 0xB0 && (pitch == 7 || pitch == 11)) {
                setChannelExpression(event.data[0] & 0x0Fu, pitch, vel);
            } else if (status == 0xA0 && event.size >= 3) {
                setVoicePressure(event.data[0] & 0x0Fu, pitch, vel);
            } else if (status == 0xD0 && event.size >= 2) {
                setChannelPressure(event.data[0] & 0x0Fu, pitch);
            } else if (status == 0xE0 && event.size >= 3) {
                const int value = (static_cast<int>(event.data[2] & 0x7Fu) << 7) |
                                  static_cast<int>(event.data[1] & 0x7Fu);
                updateVoicePitch(event.data[0] & 0x0Fu,
                                 static_cast<float>(value - 8192) / 8192.0f);
            }
        }

        process(buffer);
    }

private:
    static float interpolateSample(const float* data, uint32_t length, double position,
                                   bool looping, uint32_t loopStart, uint32_t loopEnd) noexcept {
        if (!data || length == 0 || !std::isfinite(position)) return 0.0f;
        const auto at = [&](int64_t rawIndex) noexcept {
            int64_t index = rawIndex;
            if (looping && loopEnd > loopStart + 1) {
                const int64_t start = static_cast<int64_t>(loopStart);
                const int64_t end = static_cast<int64_t>(loopEnd);
                const int64_t span = end - start;
                if (index < start) {
                    index = end - ((start - index) % span);
                    if (index == end) index = start;
                } else if (index >= end) {
                    index = start + ((index - start) % span);
                }
            } else {
                index = std::clamp<int64_t>(index, 0, static_cast<int64_t>(length - 1));
            }
            const float value = data[static_cast<size_t>(index)];
            return std::isfinite(value) ? value : 0.0f;
        };
        const int64_t i = static_cast<int64_t>(std::floor(position));
        const float t = static_cast<float>(position - static_cast<double>(i));
        const float y0 = at(i - 1);
        const float y1 = at(i);
        const float y2 = at(i + 1);
        const float y3 = at(i + 2);
        const float c0 = y1;
        const float c1 = 0.5f * (y2 - y0);
        const float c2 = y0 - 2.5f * y1 + 2.0f * y2 - 0.5f * y3;
        const float c3 = 0.5f * (y3 - y0) + 1.5f * (y1 - y2);
        const float result = ((c3 * t + c2) * t + c1) * t + c0;
        return std::isfinite(result) ? result : y1;
    }

    // Resolve overlapping mappings by the most specific velocity layer first,
    // then by key span. This keeps stacked zones deterministic while allowing
    // broad fallback mappings underneath focused articulations.
    std::vector<Core::SamplerZone>::const_iterator findZone(uint8_t note, uint8_t velocity) noexcept {
        const auto matches = [note, velocity](const Core::SamplerZone& zone) {
            return note >= zone.lowKey && note <= zone.highKey &&
                   velocity >= zone.lowVelocity && velocity <= zone.highVelocity &&
                   zone.buffer.data && zone.buffer.length > 0;
        };
        auto best = m_zones.end();
        unsigned bestVelocitySpan = std::numeric_limits<unsigned>::max();
        unsigned bestKeySpan = std::numeric_limits<unsigned>::max();
        size_t equallySpecific = 0;
        for (auto it = m_zones.begin(); it != m_zones.end(); ++it) {
            if (!matches(*it)) continue;
            const auto velocitySpan = static_cast<unsigned>(it->highVelocity) - it->lowVelocity;
            const auto keySpan = static_cast<unsigned>(it->highKey) - it->lowKey;
            if (velocitySpan < bestVelocitySpan ||
                (velocitySpan == bestVelocitySpan && keySpan < bestKeySpan)) {
                best = it;
                bestVelocitySpan = velocitySpan;
                bestKeySpan = keySpan;
                equallySpecific = 1;
            } else if (velocitySpan == bestVelocitySpan && keySpan == bestKeySpan) {
                ++equallySpecific;
            }
        }
        if (best == m_zones.end() || equallySpecific <= 1) return best;
        size_t target = m_zoneRoundRobin[note]++ % equallySpecific;
        for (auto it = m_zones.begin(); it != m_zones.end(); ++it) {
            if (!matches(*it)) continue;
            const auto velocitySpan = static_cast<unsigned>(it->highVelocity) - it->lowVelocity;
            const auto keySpan = static_cast<unsigned>(it->highKey) - it->lowKey;
            if (velocitySpan == bestVelocitySpan && keySpan == bestKeySpan && target-- == 0) return it;
        }
        return best;
    }

    int findVoiceToSteal() {
        // Simple heuristic: loudest/oldest (but usually we want to steal quietest)
        int bestIdx = 0;
        float minLevel = 2.0f;
        for (size_t i = 0; i < m_voices.size(); ++i) {
            if (m_voices[i].envLevel < minLevel) {
                minLevel = m_voices[i].envLevel;
                bestIdx = i;
            }
        }
        return bestIdx;
    }

    void startVoice(Voice& v, uint8_t note, uint8_t velocity, Core::AudioBuffer* sample,
                    uint8_t rootNote, double sourceSampleRate = 0.0) {
        v.active = true;
        v.note = note;
            v.channel = 0;
            v.pressureGain = 1.0f;
        v.velocity = velocity / 127.0f;
        v.position = 0.0;
        v.pitchRatio = std::pow(2.0, (static_cast<double>(note) - rootNote) / 12.0);
        v.basePitchRatio = v.pitchRatio;
        v.sampleRateRatio = (std::isfinite(sourceSampleRate) && sourceSampleRate >= 8'000.0 &&
                             sourceSampleRate <= 384'000.0 && m_sampleRate > 0.0)
                                ? sourceSampleRate / m_sampleRate : 1.0;
        v.envState = 1; 
        v.envLevel = 0.0f;
        v.keyReleased = false;
        v.pressureGain = 1.0f;
        v.loopStart = 0;
        v.loopEnd = 0;
        v.loopEnabled = false;
        v.sample = sample;
        v.rawSample = nullptr;
        v.rawSampleLength = 0;
    }

    void noteOnRaw(uint8_t note, uint8_t velocity, const float* data, uint32_t length,
                   uint8_t rootNote, uint8_t channel = 0,
                   uint32_t loopStart = 0, uint32_t loopEnd = 0,
                   bool loopEnabled = false, double sourceSampleRate = 0.0) {
        if (!data || length == 0) return;
        int voiceIdx = -1;
        if (hasFreeVoice()) voiceIdx = static_cast<int>(popFreeVoice());
        else voiceIdx = findVoiceToSteal();
        if (voiceIdx < 0) return;
        Voice& v = m_voices[voiceIdx];
        v.active = true; v.note = note; v.channel = channel;
        v.velocity = velocity / 127.0f; v.position = 0.0;
        v.pitchRatio = std::pow(2.0, (static_cast<double>(note) - rootNote) / 12.0);
        v.basePitchRatio = v.pitchRatio;
        v.sampleRateRatio = (std::isfinite(sourceSampleRate) && sourceSampleRate >= 8'000.0 &&
                             sourceSampleRate <= 384'000.0 && m_sampleRate > 0.0)
                                ? sourceSampleRate / m_sampleRate : 1.0;
        v.envState = 1; v.envLevel = 0.0f; v.keyReleased = false;
        v.pressureGain = 1.0f;
        v.loopStart = loopStart;
        v.loopEnd = loopEnd;
        v.loopEnabled = loopEnabled && loopStart < loopEnd && loopEnd <= length;
        v.sample = nullptr; v.rawSample = data; v.rawSampleLength = length;
    }

    void updateVoicePitch(uint8_t channel, float bend) noexcept {
        if (!std::isfinite(bend)) return;
        const double normalized = std::clamp(static_cast<double>(bend), -1.0, 1.0);
        const double ratio = std::pow(2.0, normalized * (2.0 / 12.0));
        for (auto& voice : m_voices) {
            if (voice.active && voice.channel == channel)
                voice.pitchRatio = voice.basePitchRatio * ratio;
        }
    }

    void setChannelExpression(uint8_t channel, uint8_t controller, uint8_t value) noexcept {
        if (channel >= 16u) return;
        const float normalized = static_cast<float>(value) / 127.0f;
        if (controller == 7u) m_channelVolume[channel] = normalized;
        else if (controller == 11u) m_channelExpression[channel] = normalized;
    }

    void setChannelPressure(uint8_t channel, uint8_t value) noexcept {
        if (channel >= 16u) return;
        const float gain = 0.5f + 0.5f * (static_cast<float>(value) / 127.0f);
        for (auto& voice : m_voices)
            if (voice.active && voice.channel == channel) voice.pressureGain = gain;
    }

    void setVoicePressure(uint8_t channel, uint8_t note, uint8_t value) noexcept {
        const float gain = 0.5f + 0.5f * (static_cast<float>(value) / 127.0f);
        for (auto& voice : m_voices)
            if (voice.active && voice.channel == channel && voice.note == note)
                voice.pressureGain = gain;
    }

    void updateEnvelope(Voice& v) {
        const float attackStep = 0.002f;
        const float releaseStep = 0.001f;
        const float sustainLevel = 0.8f;

        switch (v.envState) {
            case 1: v.envLevel += attackStep; if (v.envLevel >= 1.0f) { v.envLevel = 1.0f; v.envState = 2; } break;
            case 2: v.envLevel -= 0.0005f; if (v.envLevel <= sustainLevel) { v.envLevel = sustainLevel; v.envState = 3; } break;
            case 4: v.envLevel -= releaseStep; if (v.envLevel <= 0.0f) { v.envLevel = 0.0f; v.active = false; v.envState = 0; } break;
        }
    }

    double m_sampleRate;
    std::vector<Voice> m_voices;
    // Fixed-capacity free-list stack. `std::stack` defaults to a deque and
    // leaves realtime allocation behavior to the library implementation;
    // voice count is bounded, so a local array is both simpler and strictly
    // allocation-free on note-on and voice reclamation paths.
    std::array<uint32_t, 64> m_freeVoiceStack{};
    uint32_t m_freeVoiceCount = 0;

    bool hasFreeVoice() const noexcept { return m_freeVoiceCount != 0; }

    uint32_t popFreeVoice() noexcept {
        return m_freeVoiceCount != 0 ? m_freeVoiceStack[--m_freeVoiceCount] : 0u;
    }

    void pushFreeVoice(uint32_t index) noexcept {
        if (index < m_voices.size() && m_freeVoiceCount < m_freeVoiceStack.size())
            m_freeVoiceStack[m_freeVoiceCount++] = index;
    }

    void resetFreeVoices() noexcept {
        m_freeVoiceCount = 0;
        for (uint32_t i = static_cast<uint32_t>(m_voices.size()); i-- > 0;)
            pushFreeVoice(i);
    }
    std::vector<Core::SamplerZone> m_zones;
    std::array<bool, 16> m_sustainPedal{};
    std::array<float, 16> m_channelVolume{};
    std::array<float, 16> m_channelExpression{};
    std::array<uint32_t, 128> m_zoneRoundRobin{};
};

} // namespace Aura::DSP::Synthesis
