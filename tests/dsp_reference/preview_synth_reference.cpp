#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>

namespace {
struct MidiEvent {
    uint64_t sampleOffset;
    uint32_t size;
    uint8_t data[256];
    uint8_t articulationId;
};
static_assert(sizeof(MidiEvent) == 272);

struct Voice {
    bool active = false;
    uint8_t pitch = 60;
    uint8_t channel = 0;
    float level = 0.0f;
    double phase = 0.0;
    uint64_t releaseAt = std::numeric_limits<uint64_t>::max();
    uint64_t startedAt = 0;
};

struct PreviewSynthReference {
    Voice voices[16]{};
    float filterState = 0.0f;
    uint32_t engine = 0;
};
}

extern "C" void* hirari_preview_synth_reference_create() {
    return new PreviewSynthReference;
}
extern "C" void hirari_preview_synth_reference_destroy(void* state) {
    delete static_cast<PreviewSynthReference*>(state);
}
extern "C" void hirari_preview_synth_reference_set_engine(void* state, uint32_t engine) {
    if (state) static_cast<PreviewSynthReference*>(state)->engine = std::min(engine, 2u);
}
extern "C" void hirari_preview_synth_reference_reset(void* state) {
    if (!state) return;
    auto& synth = *static_cast<PreviewSynthReference*>(state);
    std::fill(std::begin(synth.voices), std::end(synth.voices), Voice{});
    synth.filterState = 0.0f;
}
extern "C" void hirari_preview_synth_reference_process(
    void* state, const MidiEvent* events, size_t eventCount,
    float* left, float* right, uint32_t frames, uint64_t playhead,
    double sampleRate, float morph, double detuneRatio,
    float drive, float cutoff, float resonance, float outputGain) {
    if (!state || !left || !right) return;
    auto& synth = *static_cast<PreviewSynthReference*>(state);
    size_t eventIndex = 0;
    for (uint32_t frame = 0; frame < frames; ++frame) {
        const uint64_t absoluteSample = playhead + frame;
        while (eventIndex < eventCount && events[eventIndex].sampleOffset <= frame) {
            const auto& event = events[eventIndex++];
            if (event.sampleOffset != frame || event.size < 3) continue;
            const uint8_t status = event.data[0] & 0xf0;
            const uint8_t channel = event.data[0] & 0x0f;
            const uint8_t pitch = event.data[1] & 0x7f;
            if (status == 0x90 && event.data[2] > 0) {
                auto* voice = std::find_if(std::begin(synth.voices), std::end(synth.voices),
                    [](const Voice& v) { return !v.active; });
                if (voice == std::end(synth.voices)) voice = std::begin(synth.voices);
                voice->active = true;
                voice->pitch = pitch;
                voice->channel = channel;
                voice->level = static_cast<float>(event.data[2]) / 127.0f * 0.65f;
                voice->phase = 0.0;
                voice->releaseAt = std::numeric_limits<uint64_t>::max();
                voice->startedAt = absoluteSample;
            } else if (status == 0x80 || (status == 0x90 && event.data[2] == 0)) {
                Voice* oldest = nullptr;
                for (auto& voice : synth.voices) {
                    if (!voice.active || voice.pitch != pitch || voice.channel != channel ||
                        voice.releaseAt != std::numeric_limits<uint64_t>::max()) continue;
                    if (!oldest || voice.startedAt < oldest->startedAt) oldest = &voice;
                }
                if (oldest) oldest->releaseAt = absoluteSample;
            }
        }
        float sample = 0.0f;
        for (auto& voice : synth.voices) {
            if (!voice.active) continue;
            if (absoluteSample >= voice.releaseAt) {
                voice.level *= 0.995f;
                if (voice.level < 0.0005f) { voice.active = false; continue; }
            }
            const double frequency = 440.0 * std::pow(2.0, (static_cast<int>(voice.pitch) - 69) / 12.0) * detuneRatio;
            const float sine = static_cast<float>(std::sin(voice.phase));
            const float normalizedPhase = static_cast<float>(std::fmod(voice.phase + 2.0 * M_PI, 2.0 * M_PI) / (2.0 * M_PI));
            const float saw = normalizedPhase * 2.0f - 1.0f;
            float oscillator = sine;
            if (synth.engine == 1) oscillator = sine * (1.0f - morph) + saw * morph;
            else if (synth.engine == 2) {
                const float triangle = 1.0f - 4.0f * std::abs(normalizedPhase - 0.5f);
                oscillator = triangle * (1.0f - morph) + saw * morph;
            }
            sample += oscillator * voice.level;
            voice.phase += 2.0 * M_PI * frequency / sampleRate;
            if (voice.phase >= 2.0 * M_PI) voice.phase = std::fmod(voice.phase, 2.0 * M_PI);
        }
        const float driven = std::tanh(sample * drive);
        synth.filterState += cutoff * (driven - synth.filterState);
        sample = (synth.filterState + resonance * (driven - synth.filterState)) * outputGain;
        left[frame] += sample;
        right[frame] += sample;
    }
}
