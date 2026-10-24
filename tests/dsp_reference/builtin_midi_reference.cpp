// Frozen reference for Track's former C++ built-in MIDI oscillator renderer.
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <memory>

namespace {
struct EventView {
    uint64_t sample_offset;
    uint32_t size;
    uint8_t data[256];
    uint8_t articulation_id;
};
struct Voice {
    bool active = false;
    bool held = false;
    uint8_t pitch = 60;
    uint8_t channel = 0;
    float velocity = 0.0f;
    float envelope = 0.0f;
    double phase = 0.0;
    double frequency = 440.0;
    uint64_t started_order = 0;
};
struct Reference {
    std::array<Voice, 32> voices{};
    uint64_t order = 0;
};
}

extern "C" void* hirari_builtin_midi_reference_create() {
    return new Reference{};
}
extern "C" void hirari_builtin_midi_reference_destroy(void* state) {
    delete static_cast<Reference*>(state);
}
extern "C" void hirari_builtin_midi_reference_process(
    void* opaque, const EventView* events, size_t event_count,
    float* left, float* right, uint32_t frames, double sample_rate) {
    auto& state = *static_cast<Reference*>(opaque);
    if (!std::isfinite(sample_rate) || sample_rate <= 1.0) sample_rate = 44100.0;
    size_t event_index = 0;
    for (uint32_t frame = 0; frame < frames; ++frame) {
        while (event_index < event_count && events[event_index].sample_offset <= frame) {
            const auto& event = events[event_index++];
            if (event.sample_offset != frame || event.size < 3) continue;
            const uint8_t status = event.data[0] & 0xf0;
            const uint8_t channel = event.data[0] & 0x0f;
            const uint8_t pitch = event.data[1] & 0x7f;
            if (status == 0x90 && event.data[2] > 0) {
                Voice* voice = nullptr;
                for (auto& candidate : state.voices) {
                    if (!candidate.active) { voice = &candidate; break; }
                    if (voice == nullptr || candidate.started_order < voice->started_order)
                        voice = &candidate;
                }
                if (voice) {
                    *voice = Voice{};
                    voice->active = voice->held = true;
                    voice->pitch = pitch;
                    voice->channel = channel;
                    voice->velocity = static_cast<float>(event.data[2]) / 127.0f;
                    voice->frequency = 440.0 * std::pow(2.0, (static_cast<int>(pitch) - 69) / 12.0);
                    voice->started_order = ++state.order;
                }
            } else if (status == 0x80 || (status == 0x90 && event.data[2] == 0)) {
                Voice* oldest = nullptr;
                for (auto& voice : state.voices) {
                    if (!voice.active || !voice.held || voice.pitch != pitch || voice.channel != channel) continue;
                    if (oldest == nullptr || voice.started_order < oldest->started_order) oldest = &voice;
                }
                if (oldest) oldest->held = false;
            } else if (status == 0xb0 && event.data[1] == 123) {
                for (auto& voice : state.voices) {
                    if (voice.active && voice.channel == channel) voice.held = false;
                }
            }
        }
        float mixed = 0.0f;
        for (auto& voice : state.voices) {
            if (!voice.active) continue;
            const float attack = static_cast<float>(1.0 / (sample_rate * 0.005));
            const float release = static_cast<float>(1.0 / (sample_rate * 0.12));
            voice.envelope = voice.held ? std::min(1.0f, voice.envelope + attack)
                                        : std::max(0.0f, voice.envelope - release);
            if (!voice.held && voice.envelope <= 0.0f) { voice.active = false; continue; }
            mixed += static_cast<float>(std::sin(voice.phase)) * voice.velocity * voice.envelope * 0.2f;
            voice.phase += 2.0 * M_PI * voice.frequency / sample_rate;
            if (voice.phase >= 2.0 * M_PI) voice.phase = std::fmod(voice.phase, 2.0 * M_PI);
        }
        left[frame] += mixed;
        right[frame] += mixed;
    }
}
