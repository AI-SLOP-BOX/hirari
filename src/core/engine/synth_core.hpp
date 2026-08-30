#pragma once

#include <stdint.h>
#include <vector>
#include <cmath>
#include <atomic>
#include <algorithm>
#include <array>
#include <cstddef>
#include "../../dsp/iprocessor.hpp"
#include "../audio_buffer.hpp"
#include "macro_control_manager.hpp"

namespace Aura::Core::Engine {

/**
 * @class SynthCore
 * @brief Multi-engine synthesizer.
 */
class SynthCore : public ::Aura::DSP::IProcessor {
public:
    enum class EngineType { Subtractive, FM, Wavetable };

    SynthCore() : m_engineType(EngineType::Subtractive) { reset(); }

    void prepareToPlay(double sr, uint32_t sz) noexcept override {
        (void)sz;
        m_sampleRate = std::isfinite(sr) && sr > 1000.0 ? sr : 44100.0;
        reset();
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& context) noexcept override {
        (void)context;
        if (buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        const uint32_t count = buffer.getNumSamples();
        buffer.clear(count);
        midi.sort();
        uint32_t cursor = 0;
        for (const auto& event : midi) {
            const uint32_t offset = std::min<uint32_t>(static_cast<uint32_t>(event.sampleOffset), count);
            if (offset > cursor) renderRange(buffer, cursor, offset - cursor);
            if (event.size >= 2) {
                const uint8_t status = event.data[0] & 0xF0u;
                const uint8_t note = event.data[1] & 0x7Fu;
                const uint8_t velocity = event.size >= 3 ? event.data[2] & 0x7Fu : 0;
                if (status == 0x90u && velocity != 0) startNote(note, velocity);
                else if (status == 0x80u || (status == 0x90u && velocity == 0)) stopNote(note);
            }
            cursor = offset;
        }
        if (cursor < count) renderRange(buffer, cursor, count - cursor);
    }

    void startNote(uint8_t note, uint8_t velocity) {
        if (note > 127 || velocity == 0) return;
        Voice* target = nullptr;
        for (auto& voice : m_voices) if (voice.active && voice.note == note) { target = &voice; break; }
        if (!target) for (auto& voice : m_voices) if (!voice.active) { target = &voice; break; }
        if (!target) {
            target = &m_voices[0];
            for (auto& voice : m_voices) if (voice.age > target->age) target = &voice;
        }
        target->note = note;
        target->phase = 0.0;
        target->level = 0.0f;
        target->velocity = static_cast<float>(velocity) / 127.0f;
        target->active = true;
        target->releasing = false;
        target->noteAge = 0;
        target->releaseAge = 0;
        target->age = ++m_age;
    }

    void stopNote(uint8_t note) {
        for (auto& voice : m_voices) if (voice.active && voice.note == note) voice.releasing = true;
    }

    void setEngineType(EngineType type) noexcept { m_engineType = type; }
    EngineType engineType() const noexcept { return m_engineType; }
    void setGain(float gain) noexcept { m_gain = std::isfinite(gain) ? std::clamp(gain, 0.0f, 2.0f) : 1.0f; }

private:
    struct Voice {
        uint8_t note = 0;
        bool active = false;
        bool releasing = false;
        double phase = 0.0;
        float level = 0.0f;
        float velocity = 0.0f;
        uint64_t noteAge = 0;
        uint64_t releaseAge = 0;
        uint64_t age = 0;
    };

    void renderRange(Core::AudioBuffer& buffer, uint32_t offset, uint32_t frames) noexcept {
        float* left = buffer.getWritePointer(0, offset);
        float* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1, offset) : left;
        if (!left || !right) return;
        constexpr double kTwoPi = 6.28318530717958647692;
        const auto& macros = MacroControlManager::getInstance();
        const float morph = macros.getMacroValue(0);
        const float detuneCents = (macros.getMacroValue(1) - 0.5f) * 40.0f;
        const float drive = 0.5f + macros.getMacroValue(2) * 3.0f;
        const float cutoff = 0.01f + macros.getMacroValue(3) * 0.45f;
        const float resonance = macros.getMacroValue(4) * 0.35f;
        const float outputGain = 0.25f + macros.getMacroValue(5) * 1.25f;
        for (uint32_t i = 0; i < frames; ++i) {
            float sample = 0.0f;
            for (auto& voice : m_voices) {
                if (!voice.active) continue;
                ++voice.noteAge;
                if (voice.releasing) ++voice.releaseAge;
                const double freq = 440.0 * std::pow(2.0, (static_cast<double>(voice.note) - 69.0) / 12.0 + detuneCents / 1200.0);
                const float phase = static_cast<float>(voice.phase);
                const float sine = std::sin(phase * static_cast<float>(kTwoPi));
                const float saw = 2.0f * (phase - std::floor(phase + 0.5f));
                float oscillator = sine;
                if (m_engineType == EngineType::FM) {
                    oscillator = std::sin(phase * static_cast<float>(kTwoPi) + 2.0f * sine);
                } else if (m_engineType == EngineType::Wavetable) {
                    oscillator = sine * (1.0f - morph) + saw * morph;
                }
                const float attackSamples = static_cast<float>(m_sampleRate * 0.012);
                const float decaySamples = static_cast<float>(m_sampleRate * 0.09);
                const float releaseSamples = static_cast<float>(m_sampleRate * 0.18);
                float envelope = 1.0f;
                if (voice.releasing) {
                    envelope = 1.0f - std::min(1.0f, voice.releaseAge / std::max(1.0f, releaseSamples));
                } else if (voice.noteAge < static_cast<uint64_t>(attackSamples)) {
                    envelope = voice.noteAge / std::max(1.0f, attackSamples);
                } else if (voice.noteAge < static_cast<uint64_t>(attackSamples + decaySamples)) {
                    const float decay = (voice.noteAge - attackSamples) / std::max(1.0f, decaySamples);
                    envelope = 1.0f - decay * 0.25f;
                } else {
                    envelope = 0.75f;
                }
                const float lfo = std::sin(static_cast<float>(voice.noteAge) * 2.0f * static_cast<float>(kTwoPi) * 5.0f / static_cast<float>(m_sampleRate));
                const float modulated = oscillator * (1.0f + lfo * resonance * 0.18f);
                voice.level = voice.velocity * std::max(0.0f, envelope);
                sample += modulated * voice.level;
                voice.phase += freq / m_sampleRate;
                voice.phase -= std::floor(voice.phase);
                if (voice.releasing && (envelope <= 0.0f || voice.level < 1e-4f)) voice.active = false;
            }
            const float driven = std::tanh(sample * drive);
            m_filter_z1 += cutoff * (driven - m_filter_z1);
            const float filtered = m_filter_z1 + resonance * (driven - m_filter_z1);
            const float out = std::isfinite(filtered) ? filtered * m_gain * outputGain : 0.0f;
            left[i] += out;
            if (right != left) right[i] += out;
        }
    }

    void reset() noexcept override {
        for (auto& voice : m_voices) voice = Voice{};
        m_filter_z1 = 0.0f;
        m_age = 0;
    }


    EngineType m_engineType;
    std::array<Voice, 32> m_voices{};
    double m_sampleRate = 44100.0;
    float m_gain = 1.0f;
    float m_filter_z1 = 0.0f;
    uint64_t m_age = 0;
};

} // namespace Aura::Core::Engine
