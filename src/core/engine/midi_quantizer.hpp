#pragma once
#include <vector>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include "../engine_types.hpp"

namespace Aura::Core::Engine {

struct MIDINote {
    uint64_t startTick;
    uint64_t lengthTicks;
    uint8_t note;
    uint8_t velocity;
};

/**
 * @class MidiQuantizer
 * @brief Industrial MIDI Grid Alignment Engine.
 * Tick-based quantization with swing, strength lerp, and all standard resolutions.
 * kTicksPerBeat = 960 (Logic Pro standard, inherited from MusicalTime).
 */
class MidiQuantizer {
public:
    enum class Resolution {
        Q1_4, Q1_8, Q1_16, Q1_32,
        Q1_8T, Q1_16T, Q1_8D, Q1_16D
    };

    /**
     * @brief Quantize notes to the specified grid resolution.
     * @param notes     Note list to quantize in-place.
     * @param res       Target grid resolution.
     * @param swing     Swing amount [0.0, 1.0]. 0.5 = no swing, >0.5 = late offbeat.
     * @param strength  Quantize pull strength [0.0, 1.0]. 0=no change, 1=full snap.
     */
    static void quantize(std::vector<MIDINote>& notes, Resolution res,
                         float swing, float strength) {
        const uint64_t gridTicks = resolutionToTicks(res);
        if (gridTicks == 0 || !std::isfinite(strength) || !std::isfinite(swing) || strength <= 0.0f) return;

        const float clampedStrength = std::clamp(strength, 0.0f, 1.0f);
        // swing is expressed as fraction of gridTicks that offbeats are shifted.
        // 0.5 = neutral (no swing), >0.5 = offbeat pushed later.
        const float swingShift = (std::clamp(swing, 0.0f, 1.0f) - 0.5f) * 2.0f; // [-1, 1]

        for (auto& note : notes) {
            // Determine which beat boundary this note is near
            uint64_t gridIndex = note.startTick / gridTicks;
            uint64_t gridStart = gridIndex * gridTicks;
            uint64_t gridEnd = gridStart > UINT64_MAX - gridTicks
                ? UINT64_MAX : gridStart + gridTicks;

            // Apply swing to the grid line being considered, not the interval
            // containing the note. The interval's right edge is often the
            // odd offbeat (the old code incorrectly left it unswung).
            const auto swingOffsetFor = [gridTicks, swingShift](uint64_t index) {
                return index % 2 == 1
                    ? static_cast<int64_t>(swingShift * 0.5 * static_cast<float>(gridTicks))
                    : int64_t{0};
            };
            const int64_t startSwing = swingOffsetFor(gridIndex);
            const int64_t endSwing = swingOffsetFor(gridIndex + (gridEnd == UINT64_MAX ? 0 : 1));

            // Choose nearest grid line (start or next), accounting for swing
            // MIDI tick positions are 64-bit unsigned; never narrow them
            // directly into signed arithmetic near the project-size limit.
            const int64_t originalSigned = note.startTick > static_cast<uint64_t>(INT64_MAX)
                ? INT64_MAX : static_cast<int64_t>(note.startTick);
            const int64_t gridStartSigned = gridStart > static_cast<uint64_t>(INT64_MAX)
                ? INT64_MAX : static_cast<int64_t>(gridStart);
            const int64_t gridEndSigned = gridEnd > static_cast<uint64_t>(INT64_MAX)
                ? INT64_MAX : static_cast<int64_t>(gridEnd);
            const int64_t distToStart = originalSigned - gridStartSigned - startSwing;
            const int64_t distToEnd = originalSigned - gridEndSigned - endSwing;

            int64_t nearestGridTick;
            if (std::abs(distToStart) <= std::abs(distToEnd)) {
                nearestGridTick = gridStartSigned + startSwing;
            } else {
                nearestGridTick = gridEndSigned + endSwing;
            }
            nearestGridTick = std::clamp(nearestGridTick, int64_t{0}, INT64_MAX);

            // Lerp between original position and quantized position by strength
            int64_t originalTick = originalSigned;
            int64_t quantizedTick = originalTick +
                static_cast<int64_t>(clampedStrength * static_cast<float>(nearestGridTick - originalTick));

            note.startTick = static_cast<uint64_t>(std::max(int64_t{0}, quantizedTick));
        }
    }

    /**
     * @brief Quantizes both note starts and ends while preserving a minimum
     * musical duration. This is the length-aware mode used by piano-roll
     * editors; the legacy quantize() remains start-only for compatibility.
     */
    static void quantizeWithLength(std::vector<MIDINote>& notes, Resolution res,
                                   float swing, float strength) {
        if (!std::isfinite(strength) || strength <= 0.0f) return;
        const uint64_t grid = resolutionToTicks(res);
        if (grid == 0) return;
        std::vector<uint64_t> ends;
        ends.reserve(notes.size());
        for (const auto& note : notes) {
            ends.push_back(note.startTick > UINT64_MAX - note.lengthTicks
                ? UINT64_MAX : note.startTick + note.lengthTicks);
        }
        quantize(notes, res, swing, strength);
        for (size_t i = 0; i < notes.size(); ++i) {
            MIDINote endpoint{ends[i], 0, 0, 0};
            std::vector<MIDINote> endpointNotes{endpoint};
            quantize(endpointNotes, res, swing, strength);
            const uint64_t end = endpointNotes.front().startTick;
            const uint64_t minimum = std::max<uint64_t>(1, grid / 4);
            const uint64_t duration = end > notes[i].startTick ? end - notes[i].startTick : minimum;
            notes[i].lengthTicks = std::max(minimum, duration);
        }
    }

private:
    static uint64_t resolutionToTicks(Resolution res) {
        using T = MusicalTime;
        switch (res) {
            case Resolution::Q1_4:   return T::kTicksPerBeat;
            case Resolution::Q1_8:   return T::kTicksPerBeat / 2;
            case Resolution::Q1_16:  return T::kTicksPerBeat / 4;
            case Resolution::Q1_32:  return T::kTicksPerBeat / 8;
            case Resolution::Q1_8T:  return (T::kTicksPerBeat * 2) / 3;
            case Resolution::Q1_16T: return T::kTicksPerBeat / 3;
            case Resolution::Q1_8D:  return (T::kTicksPerBeat * 3) / 4;
            case Resolution::Q1_16D: return (T::kTicksPerBeat * 3) / 8;
            default: return 0;
        }
    }
};

} // namespace Aura::Core::Engine
