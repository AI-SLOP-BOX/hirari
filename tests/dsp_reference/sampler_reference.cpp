// Frozen scalar reference extracted from the pre-Rust SamplerEngine implementation.
// Built only with the dsp-differential-reference Cargo feature.
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <limits>
#include <vector>

namespace {
struct Voice {
    bool active = false;
    double position = 0.0;
    double pitch = 1.0;
    double base_pitch = 1.0;
    double sample_rate_ratio = 1.0;
    float velocity = 1.0f;
    float envelope = 0.0f;
    uint32_t envelope_state = 0;
    uint8_t note = 0;
    uint8_t channel = 0;
    bool key_released = false;
    const float* left = nullptr;
    const float* right = nullptr;
    uint32_t length = 0;
};

struct Zone {
    const float* data;
    uint32_t length;
    double sample_rate;
    uint8_t root, low_key, high_key, low_velocity, high_velocity;
};

struct SamplerReference {
    explicit SamplerReference(double rate) : sample_rate(rate) {
        for (uint32_t i = 64; i-- > 0;) free_stack[free_count++] = i;
    }
    double sample_rate;
    std::array<Voice, 64> voices{};
    std::array<uint32_t, 64> free_stack{};
    uint32_t free_count = 0;
    std::array<bool, 16> sustain{};
    std::vector<Zone> zones;
    std::array<uint32_t, 128> round_robin{};
};

float interpolate(const float* data, uint32_t length, double position) {
    if (!data || length == 0 || !std::isfinite(position)) return 0.0f;
    const auto at = [&](int64_t raw) {
        const int64_t index = std::clamp<int64_t>(raw, 0, static_cast<int64_t>(length - 1));
        const float value = data[static_cast<size_t>(index)];
        return std::isfinite(value) ? value : 0.0f;
    };
    const int64_t i = static_cast<int64_t>(std::floor(position));
    const float t = static_cast<float>(position - static_cast<double>(i));
    const float y0 = at(i - 1), y1 = at(i), y2 = at(i + 1), y3 = at(i + 2);
    const float c1 = 0.5f * (y2 - y0);
    const float c2 = y0 - 2.5f * y1 + 2.0f * y2 - 0.5f * y3;
    const float c3 = 0.5f * (y3 - y0) + 1.5f * (y1 - y2);
    const float value = ((c3 * t + c2) * t + c1) * t + y1;
    return std::isfinite(value) ? value : y1;
}

uint32_t allocate(SamplerReference& sampler) {
    if (sampler.free_count != 0) return sampler.free_stack[--sampler.free_count];
    uint32_t quietest = 0;
    for (uint32_t i = 1; i < sampler.voices.size(); ++i)
        if (sampler.voices[i].envelope < sampler.voices[quietest].envelope) quietest = i;
    return quietest;
}

void start_voice(SamplerReference& sampler, uint8_t note, uint8_t velocity,
                 uint8_t channel, const Zone& zone) {
    Voice voice;
    voice.active = true;
    voice.note = note;
    voice.channel = channel;
    voice.velocity = static_cast<float>(velocity) / 127.0f;
    voice.pitch = std::pow(2.0, (static_cast<double>(note) - zone.root) / 12.0);
    voice.base_pitch = voice.pitch;
    voice.sample_rate_ratio = std::isfinite(zone.sample_rate) && zone.sample_rate >= 8000.0 &&
        zone.sample_rate <= 384000.0 && sampler.sample_rate > 0.0 ? zone.sample_rate / sampler.sample_rate : 1.0;
    voice.envelope_state = 1;
    voice.left = zone.data;
    voice.right = zone.data;
    voice.length = zone.length;
    sampler.voices[allocate(sampler)] = voice;
}
}

extern "C" void* sampler_reference_create(double sample_rate) {
    return new SamplerReference(sample_rate);
}
extern "C" void sampler_reference_destroy(void* state) {
    delete static_cast<SamplerReference*>(state);
}
extern "C" void sampler_reference_note_on(void* state, uint8_t note, uint8_t velocity,
    const float* left, const float* right, uint32_t length, uint8_t root, double source_rate) {
    if (!state || !left || !length) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    Voice voice;
    voice.active = true;
    voice.note = note;
    voice.velocity = static_cast<float>(velocity) / 127.0f;
    voice.pitch = std::pow(2.0, (static_cast<double>(note) - root) / 12.0);
    voice.base_pitch = voice.pitch;
    voice.sample_rate_ratio = std::isfinite(source_rate) && source_rate >= 8000.0 &&
        source_rate <= 384000.0 && sampler.sample_rate > 0.0 ? source_rate / sampler.sample_rate : 1.0;
    voice.envelope_state = 1;
    voice.left = left;
    voice.right = right ? right : left;
    voice.length = length;
    sampler.voices[allocate(sampler)] = voice;
}
extern "C" void sampler_reference_add_zone(void* state, const float* data, uint32_t length,
    double sample_rate, uint8_t root, uint8_t low_key, uint8_t high_key,
    uint8_t low_velocity, uint8_t high_velocity) {
    if (!state || !data || !length || low_key > high_key || low_velocity > high_velocity) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    if (sampler.zones.size() >= 256) return;
    if (sample_rate == 0.0) sample_rate = sampler.sample_rate;
    sampler.zones.push_back({data, length, sample_rate, root, low_key, high_key, low_velocity, high_velocity});
    std::stable_sort(sampler.zones.begin(), sampler.zones.end(), [](const Zone& left, const Zone& right) {
        if (left.low_key != right.low_key) return left.low_key < right.low_key;
        if (left.high_key != right.high_key) return left.high_key < right.high_key;
        return left.low_velocity < right.low_velocity;
    });
}
extern "C" void sampler_reference_note_on_zone(void* state, uint8_t note, uint8_t velocity, uint8_t channel) {
    if (!state) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    unsigned best_velocity_span = std::numeric_limits<unsigned>::max();
    unsigned best_key_span = std::numeric_limits<unsigned>::max();
    size_t matches = 0;
    for (const auto& zone : sampler.zones) {
        if (note < zone.low_key || note > zone.high_key || velocity < zone.low_velocity || velocity > zone.high_velocity) continue;
        const unsigned velocity_span = static_cast<unsigned>(zone.high_velocity) - zone.low_velocity;
        const unsigned key_span = static_cast<unsigned>(zone.high_key) - zone.low_key;
        if (velocity_span < best_velocity_span || (velocity_span == best_velocity_span && key_span < best_key_span)) {
            best_velocity_span = velocity_span;
            best_key_span = key_span;
            matches = 1;
        } else if (velocity_span == best_velocity_span && key_span == best_key_span) ++matches;
    }
    if (matches == 0) return;
    const size_t target = matches > 1 ? sampler.round_robin[note]++ % matches : 0;
    size_t ordinal = 0;
    for (const auto& zone : sampler.zones) {
        if (note < zone.low_key || note > zone.high_key || velocity < zone.low_velocity || velocity > zone.high_velocity) continue;
        const unsigned velocity_span = static_cast<unsigned>(zone.high_velocity) - zone.low_velocity;
        const unsigned key_span = static_cast<unsigned>(zone.high_key) - zone.low_key;
        if (velocity_span != best_velocity_span || key_span != best_key_span) continue;
        if (ordinal++ == target) { start_voice(sampler, note, velocity, channel, zone); return; }
    }
}
extern "C" void sampler_reference_note_off(void* state, uint8_t note, uint8_t channel) {
    if (!state) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    for (auto& voice : sampler.voices)
        if (voice.active && voice.note == note && (channel == 0xff || voice.channel == channel)) {
            if (channel < 16 && sampler.sustain[channel]) voice.key_released = true;
            else voice.envelope_state = 4;
        }
}
extern "C" void sampler_reference_set_sustain(void* state, uint8_t channel, bool held) {
    if (!state || channel >= 16) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    sampler.sustain[channel] = held;
    if (!held) for (auto& voice : sampler.voices) {
        if (voice.active && voice.channel == channel && voice.key_released) {
            voice.key_released = false;
            voice.envelope_state = 4;
        }
    }
}
extern "C" void sampler_reference_pitch_bend(void* state, uint8_t channel, float bend) {
    if (!state || !std::isfinite(bend)) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    const double ratio = std::pow(2.0, std::clamp(static_cast<double>(bend), -1.0, 1.0) * (2.0 / 12.0));
    for (auto& voice : sampler.voices)
        if (voice.active && voice.channel == channel) voice.pitch = voice.base_pitch * ratio;
}
extern "C" void sampler_reference_process(void* state, float* left, float* right, uint32_t frames) {
    if (!state || !left || !frames) return;
    auto& sampler = *static_cast<SamplerReference*>(state);
    std::fill(left, left + frames, 0.0f);
    if (right) std::fill(right, right + frames, 0.0f);
    for (uint32_t index = 0; index < sampler.voices.size(); ++index) {
        auto& voice = sampler.voices[index];
        if (!voice.active || !voice.left || voice.length == 0) continue;
        for (uint32_t frame = 0; frame < frames; ++frame) {
            if (voice.envelope_state == 0 || !voice.active) {
                voice.active = false;
                voice.left = voice.right = nullptr;
                sampler.free_stack[sampler.free_count++] = index;
                break;
            }
            if (static_cast<uint64_t>(voice.position) >= voice.length) {
                voice.active = false;
                voice.envelope_state = 0;
                voice.left = voice.right = nullptr;
                sampler.free_stack[sampler.free_count++] = index;
                break;
            }
            const float sample_left = interpolate(voice.left, voice.length, voice.position);
            const float sample_right = interpolate(voice.right, voice.length, voice.position);
            if (voice.envelope_state == 1) {
                voice.envelope += 0.002f;
                if (voice.envelope >= 1.0f) { voice.envelope = 1.0f; voice.envelope_state = 2; }
            } else if (voice.envelope_state == 2) {
                voice.envelope -= 0.0005f;
                if (voice.envelope <= 0.8f) { voice.envelope = 0.8f; voice.envelope_state = 3; }
            } else if (voice.envelope_state == 4) {
                voice.envelope -= 0.001f;
                if (voice.envelope <= 0.0f) { voice.envelope = 0.0f; voice.active = false; voice.envelope_state = 0; }
            }
            const float gain = voice.envelope * voice.velocity;
            const float out_left = sample_left * gain;
            const float out_right = sample_right * gain;
            if (std::isfinite(out_left)) left[frame] += out_left;
            if (right && std::isfinite(out_right)) right[frame] += out_right;
            voice.position += voice.pitch * voice.sample_rate_ratio;
        }
    }
}

struct MidiEventReference {
    uint64_t sample_offset;
    uint32_t size;
    uint8_t data[256];
    uint8_t articulation_id;
};
static_assert(sizeof(MidiEventReference) == 272);

// Frozen copy of the former C++ SamplerEngine MIDI dispatch path.
extern "C" void sampler_reference_process_midi_events(
    void* state, const MidiEventReference* events, size_t event_count,
    float* left, float* right, uint32_t frames) {
    if (!state || !left || frames == 0) return;
    event_count = std::min<size_t>(event_count, 1024);
    for (size_t index = 0; events && index < event_count; ++index) {
        const auto& event = events[index];
        if (event.size == 16 && (event.data[0] >> 4u) == 0x4u) {
            const uint8_t status = event.data[1] & 0xf0u;
            const uint8_t channel = event.data[1] & 0x0fu;
            const uint8_t note = event.data[2] & 0x7fu;
            const uint8_t value = event.data[4];
            if (status == 0x90u && value != 0) {
                sampler_reference_note_on_zone(state, note, value, channel);
            } else if (status == 0x80u || (status == 0x90u && value == 0)) {
                sampler_reference_note_off(state, note, channel);
            } else if (status == 0xb0u && (event.data[2] & 0x7fu) == 64u) {
                sampler_reference_set_sustain(state, channel, value >= 64u);
            } else if (status == 0xe0u) {
                sampler_reference_pitch_bend(state, channel,
                    (static_cast<float>(value) - 128.0f) / 128.0f);
            }
            continue;
        }
        if (event.size < 2 || event.data[0] < 0x80u) continue;
        const uint8_t status = event.data[0] & 0xf0u;
        const uint8_t channel = event.data[0] & 0x0fu;
        const uint8_t pitch = event.data[1] & 0x7fu;
        const uint8_t velocity = event.size >= 3 ? event.data[2] & 0x7fu : 0;
        if (status == 0x90u && velocity > 0) {
            sampler_reference_note_on_zone(state, pitch, velocity, channel);
        } else if (status == 0x80u || (status == 0x90u && velocity == 0)) {
            sampler_reference_note_off(state, pitch, channel);
        } else if (status == 0xb0u && pitch == 64u) {
            sampler_reference_set_sustain(state, channel, velocity >= 64u);
        } else if (status == 0xe0u && event.size >= 3) {
            const int value = (static_cast<int>(event.data[2] & 0x7fu) << 7) |
                              static_cast<int>(event.data[1] & 0x7fu);
            sampler_reference_pitch_bend(state, channel,
                static_cast<float>(value - 8192) / 8192.0f);
        }
    }
    sampler_reference_process(state, left, right, frames);
}
