#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>
#include "transient_detector.hpp"

namespace Aura::DSP::Analysis {

/**
 * @brief DrumReplacer: Audio-to-MIDI Trigger System.
 * Converts drum recording peaks into MIDI notes with sample-level phase alignment.
 * Essential for professional 'Layering' - reinforcing weak drums with studio samples.
 */
class DrumReplacer {
public:
    struct TriggerEvent {
        uint64_t samplePosition;
        float velocity;
        uint8_t midiNote;
    };

    static std::vector<TriggerEvent> convertToMidi(const float* buffer, size_t size, double sr, uint8_t targetNote = 36) {
        const uint64_t guard = std::isfinite(sr) && sr > 0.0 ? static_cast<uint64_t>(sr * 0.020) : 0;
        return convertToMidiWithConfig(buffer, size, sr, targetNote, 0.15f, guard);
    }

    static std::vector<TriggerEvent> convertToMidiWithConfig(const float* buffer, size_t size,
                                                              double sr, uint8_t targetNote,
                                                              float sensitivity,
                                                              uint64_t retriggerSamples) {
        std::vector<TriggerEvent> result;
        if (buffer == nullptr || size < 2 || !std::isfinite(sr) || sr < 8000.0 || sr > 384000.0 ||
            !std::isfinite(sensitivity) || sensitivity < 0.0f || sensitivity > 1.0f) {
            return result;
        }

        const uint8_t note = std::min<uint8_t>(targetNote, 127);
        Aura::Core::DSP::Analysis::TransientDetector detector(sr);
        const auto transients = detector.analyze(buffer, size, sensitivity);
        result.reserve(transients.size());
        uint64_t lastPosition = 0;
        bool hasLast = false;

        for (const auto& transient : transients) {
            if (!std::isfinite(transient.strength) || transient.strength <= 0.0f) {
                continue;
            }
            if (hasLast && transient.sampleIndex < lastPosition + retriggerSamples) continue;

            // Flux is unbounded, while MIDI velocity is 7-bit.  Compress the
            // detector strength into a useful musical range and keep silence
            // from producing Note On velocity zero.
            const float velocity = std::clamp(
                1.0f + 126.0f * (1.0f - std::exp(-transient.strength * 8.0f)),
                1.0f,
                127.0f);
            result.push_back({transient.sampleIndex, velocity, note});
            lastPosition = transient.sampleIndex;
            hasLast = true;
        }
        return result;
    }

    static std::vector<uint8_t> generateMidiStream(const std::vector<TriggerEvent>& triggers) {
        // This is a deterministic sample-timestamped event stream, not a
        // Standard MIDI File. Each packet is:
        // [sample position: uint64 LE][Note On: 0x90,note,velocity]
        // [Note Off: 0x80,note,0]. Keeping the timestamp preserves the
        // sample-accurate trigger position for the engine-side scheduler.
        constexpr size_t kPacketSize = sizeof(uint64_t) + 6;
        std::vector<uint8_t> stream;
        stream.reserve(triggers.size() * kPacketSize);

        for (const auto& trigger : triggers) {
            const uint8_t note = std::min<uint8_t>(trigger.midiNote, 127);
            const auto velocity = static_cast<uint8_t>(std::clamp(
                std::isfinite(trigger.velocity) ? trigger.velocity : 1.0f,
                1.0f,
                127.0f));

            for (unsigned shift = 0; shift < sizeof(uint64_t); ++shift) {
                stream.push_back(static_cast<uint8_t>((trigger.samplePosition >> (shift * 8)) & 0xffu));
            }
            stream.insert(stream.end(), {0x90u, note, velocity, 0x80u, note, 0x00u});
        }
        return stream;
    }
};


} // namespace Aura::DSP::Analysis
