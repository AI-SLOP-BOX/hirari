#pragma once
#include <vector>
#include <string>
#include <map>
#include <cmath>
#include <algorithm>
#include "../midi_buffer.hpp"

namespace Aura::Core::Midi {

/**
 * @struct GrooveTemplate
 * @brief Represents a rhythmic feel for humanization or swing.
 */
struct GrooveTemplate {
    std::string name;
    std::vector<float> swingOffsets; // PPQ deviations
    std::vector<float> velocityWeights;
};

/**
 * @class RhythmicQuantizer
 * @brief Industrial MIDI Quantization Engine.
 * HONEST FIX: Replaced 'Intelligence' branding with a real grid-based quantizer.
 */
class RhythmicQuantizer {
public:
    static RhythmicQuantizer& getInstance() { static RhythmicQuantizer i; return i; }

    /**
     * @brief Quantizes MIDI events in the buffer to the specified grid.
     * @param gridPPQ The grid resolution (e.g., 480 for 1/4 notes, 120 for 1/16th).
     * @param strength 0.0 to 1.0 (amount of correction).
     * @param swing 0.0 to 1.0 (amount of shuffle).
     */
    void quantize(MidiBuffer& buffer, uint32_t gridPPQ, float strength, float swing) {
        if (gridPPQ == 0) return;

        for (auto& ev : buffer.getEvents()) {
            // Process Note On events
            if ((ev.status & 0xF0) == 0x90 && ev.data2 > 0) {
                uint64_t originalPos = ev.timestamp;
                uint64_t targetPos = ((originalPos + gridPPQ / 2) / gridPPQ) * gridPPQ;

                // Apply Swing (delay every 2nd grid point)
                if (swing > 0.0f) {
                    uint32_t gridIdx = (targetPos / gridPPQ) % 2;
                    if (gridIdx == 1) {
                        targetPos += static_cast<uint64_t>(gridPPQ * 0.5f * swing);
                    }
                }

                // Strength interpolation: pos = original + strength * (target - original)
                ev.timestamp = static_cast<uint64_t>(originalPos + strength * (static_cast<int64_t>(targetPos) - static_cast<int64_t>(originalPos)));
            }
        }
        
        // Sort buffer to maintain chronological order
        buffer.sort();
    }

private:
    RhythmicQuantizer() {
        m_templates["MPC 60"] = { "MPC 60", {0.0f, 20.0f, 0.0f, 20.0f}, {1.0f, 0.9f, 1.0f, 0.9f} };
    }
    std::map<std::string, GrooveTemplate> m_templates;
};

} // namespace Aura::Core::Midi
