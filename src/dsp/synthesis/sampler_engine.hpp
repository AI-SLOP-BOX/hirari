#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>
#include "../../core/audio_buffer.hpp"
#include "../../core/audio_types.hpp"
#include "../../core/rust_ffi.hpp"
#include "../iprocessor.hpp"

namespace Hirari::DSP::Synthesis {

inline bool samplerReadAudioBuffer(void* context, const float** left, const float** right,
                                   uint32_t* frames) noexcept {
    if (!context || !left || !right || !frames) return false;
    const auto* buffer = static_cast<const Core::AudioBuffer*>(context);
    if (buffer->isEmpty() || buffer->getNumChannels() == 0) return false;
    *left = buffer->getReadPointer(0);
    *right = buffer->getNumChannels() > 1 ? buffer->getReadPointer(1) : *left;
    *frames = buffer->getNumSamples();
    return *left != nullptr && *frames > 0;
}

/** C++ AudioBuffer/MIDI adapter; sampler voices and rendering live in Rust. */
class SamplerEngine : public IProcessor {
public:
    explicit SamplerEngine(double sampleRate = 44100.0)
        : m_state(hirari_sampler_create(sampleRate, &samplerReadAudioBuffer)) {}
    ~SamplerEngine() override { hirari_sampler_destroy(m_state); }
    SamplerEngine(const SamplerEngine&) = delete;
    SamplerEngine& operator=(const SamplerEngine&) = delete;
    SamplerEngine(SamplerEngine&&) = delete;
    SamplerEngine& operator=(SamplerEngine&&) = delete;

    void noteOn(uint8_t note, uint8_t velocity, Core::AudioBuffer* sample,
                uint8_t rootNote = 60, double sourceSampleRate = 0.0) {
        if (m_state && sample)
            hirari_sampler_note_on_buffer(m_state, note, velocity, sample, rootNote, sourceSampleRate);
    }

    void addZone(const Core::SamplerZone& zone) {
        if (!m_state || !zone.buffer.data || zone.buffer.length == 0) return;
        const double sourceRate = zone.sourceSampleRate == 0.0 ? zone.buffer.sampleRate : zone.sourceSampleRate;
        hirari_sampler_add_zone(m_state, zone.buffer.data, zone.buffer.length, sourceRate,
            zone.rootKey, zone.lowKey, zone.highKey, zone.lowVelocity, zone.highVelocity,
            static_cast<uint32_t>(std::min<size_t>(zone.loopStart, UINT32_MAX)),
            static_cast<uint32_t>(std::min<size_t>(zone.loopEnd, UINT32_MAX)), zone.loopEnabled);
    }

    void NoteOff(uint8_t note, uint8_t channel = 0xFFu) {
        if (m_state) hirari_sampler_note_off(m_state, note, channel);
    }
    bool anySustainHeld() const noexcept {
        return m_state && hirari_sampler_any_sustain(m_state);
    }
    void setSustain(uint8_t channel, bool held) {
        if (m_state) hirari_sampler_set_sustain(m_state, channel, held);
    }

    void process(Core::AudioBuffer& buffer) {
        if (!m_state || buffer.isEmpty()) return;
        auto* left = buffer.getWritePointer(0);
        auto* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (left) hirari_sampler_process(m_state, left, right, buffer.getNumSamples());
    }

    void prepareToPlay(double sampleRate, uint32_t blockSize) noexcept override {
        (void)blockSize;
        if (m_state) hirari_sampler_prepare(m_state, sampleRate);
    }
    void reset() noexcept override {
        if (m_state) hirari_sampler_reset(m_state);
    }
    std::vector<bool> getStreamingHealth() const override {
        std::vector<bool> result;
        if (!m_state) return result;
        const size_t count = hirari_sampler_streaming_count(m_state);
        result.reserve(count);
        for (size_t index = 0; index < count; ++index)
            result.push_back(hirari_sampler_streaming_at(m_state, index));
        return result;
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi,
                 const ProcessContext& context) noexcept override {
        (void)context;
        if (isBypassed() || !m_state) return;
        auto* left = buffer.getWritePointer(0);
        auto* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (left) hirari_sampler_process_midi_events(m_state, midi.getEvents(), midi.size(),
            left, right, buffer.getNumSamples());
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Synthesis
