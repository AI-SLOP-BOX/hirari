#pragma once
#include <vector>
#include <cmath>
#include <random>
#include "../../core/midi_buffer.hpp"

namespace Aura::DSP::Synthesis {

/**
 * @class VirtualDrummerEngine
 * @brief Logic Pro-style AI Performance Assistant.
 * INDUSTRIAL UPGRADE: Full 9-piece kit (Kick, Snare, Hi-Hat Open/Closed,
 * Crash, Ride, Tom-High/Mid/Floor) with phrase-aware fills and crescendo
 * velocity shaping.
 */
class VirtualDrummerEngine {
public:
    // --- MIDI note map (General MIDI) ---
    static constexpr uint8_t NOTE_KICK      = 36;
    static constexpr uint8_t NOTE_SNARE     = 38;
    static constexpr uint8_t NOTE_HAT_CL    = 42;
    static constexpr uint8_t NOTE_HAT_OP    = 46;
    static constexpr uint8_t NOTE_TOM_HI    = 50;
    static constexpr uint8_t NOTE_TOM_MID   = 47;
    static constexpr uint8_t NOTE_TOM_FLOOR = 43;
    static constexpr uint8_t NOTE_CRASH     = 49;
    static constexpr uint8_t NOTE_RIDE      = 51;

    VirtualDrummerEngine(double sr = 44100.0) : m_sampleRate(sr) {
        m_rng.seed(0xDEADC0DE);
    }

    /**
     * @brief PROCESS: Generates MIDI events for a full 9-piece drum kit.
     * INDUSTRIAL:
     *  - Phrase-aware fills: Crash on beat 1 every 8 bars, snare crescendo fill
     *    on bar 7 beats 3-4.
     *  - Style-based hi-hat switching (closed → open on up-beats at high complexity).
     *  - Tom rolls during fills with falling pitch sequence.
     */
    void process(Core::MidiBuffer& output, uint64_t playhead, uint32_t numSamples, float bpm) {
        const double samplesPerBeat  = (60.0 / bpm) * m_sampleRate;
        const double samplesPerStep  = samplesPerBeat * 0.25; // 1/16th note
        const double samplesPerBar   = samplesPerBeat * 4.0;

        const uint64_t start = playhead;
        const uint64_t end   = playhead + numSamples;

        uint64_t stepIdx = static_cast<uint64_t>(std::ceil(static_cast<double>(start) / samplesPerStep));

        while (true) {
            double exactSample = static_cast<double>(stepIdx) * samplesPerStep;

            // Swing: delay odd 1/16th notes
            if (stepIdx % 2 != 0)
                exactSample += samplesPerStep * m_swing * 0.5;

            if (exactSample >= static_cast<double>(end)) break;
            uint32_t offset = static_cast<uint32_t>(exactSample - static_cast<double>(start));

            const uint32_t stepInBar  = static_cast<uint32_t>(stepIdx % 16);       // 0-15
            const uint64_t barIdx     = stepIdx / 16;
            const uint32_t stepIn8    = static_cast<uint32_t>(stepIdx % (16 * 8)); // within 8-bar phrase

            // ---------------------------------------------------------------
            // Phrase awareness
            const bool isPhrase1     = (stepIn8 == 0);          // Bar 1 beat 1
            const bool isFillZone    = (stepIn8 >= 16*7 + 8);   // Bar 8, beats 3-4
            const bool isFillFinale  = (stepIn8 == 0 && barIdx > 0); // Phrase downbeat

            // ---------------------------------------------------------------
            // KICK
            if (!isFillZone) {
                float kickProb = (stepInBar == 0 || stepInBar == 8) ? 0.97f : (m_complexity * 0.28f);
                if (rf() < kickProb * m_intensity)
                    triggerNote(output, offset, NOTE_KICK, velCurve(90, 20, m_intensity));
            }

            // SNARE
            if (isFillZone) {
                // Crescendo fill: 4 snares with increasing velocity
                const uint32_t fillStep = stepIn8 - (16*7 + 8);
                const uint8_t fillVel = static_cast<uint8_t>(60 + fillStep * 12);
                triggerNote(output, offset, NOTE_SNARE, fillVel);
                // Tom sequence during fill
                const uint8_t tomPitch = (fillStep < 2) ? NOTE_TOM_HI : (fillStep < 4) ? NOTE_TOM_MID : NOTE_TOM_FLOOR;
                if (fillStep % 2 == 1) triggerNote(output, offset, tomPitch, fillVel - 10);
            } else {
                float snareProb = (stepInBar == 4 || stepInBar == 12) ? 0.98f : (m_complexity * 0.12f);
                if (rf() < snareProb * m_intensity)
                    triggerNote(output, offset, NOTE_SNARE, velCurve(100, 25, m_intensity));
            }

            // HI-HAT
            if (!isFillZone) {
                // Open hat on up-beats at high complexity
                bool useOpenHat = (stepInBar % 4 == 2) && (m_complexity > 0.7f);
                float hatProb = (stepInBar % 2 == 0) ? 0.88f : (m_complexity * 0.75f);
                if (rf() < hatProb * m_intensity)
                    triggerNote(output, offset, useOpenHat ? NOTE_HAT_OP : NOTE_HAT_CL, velCurve(75, 20, m_complexity));
            }

            // CRASH on phrase downbeat
            if (isFillFinale)
                triggerNote(output, offset, NOTE_CRASH, velCurve(110, 15, m_intensity));

            // RIDE (replaces hat at very high complexity)
            if (m_complexity > 0.85f && stepInBar % 4 == 0 && !isFillZone)
                triggerNote(output, offset, NOTE_RIDE, 70);

            stepIdx++;
        }
    }

    void setIntensity(float i) { m_intensity = std::clamp(i, 0.0f, 1.0f); }
    void setComplexity(float c) { m_complexity = std::clamp(c, 0.0f, 1.0f); }
    void setSwing(float s)     { m_swing = std::clamp(s, 0.0f, 1.0f); }

private:
    static uint8_t velCurve(uint8_t base, uint8_t range, float t) {
        return static_cast<uint8_t>(std::clamp(static_cast<int>(base + t * range), 1, 127));
    }

    void triggerNote(Core::MidiBuffer& out, uint32_t sample, uint8_t note, uint8_t vel) {
        uint8_t noteOn[3] = { 0x90, note, vel };
        out.addEvent(sample, noteOn, 3);
    }

    float rf() {
        return std::uniform_real_distribution<float>(0.0f, 1.0f)(m_rng);
    }

    double  m_sampleRate;
    float   m_intensity  = 0.5f;
    float   m_complexity = 0.3f;
    float   m_swing      = 0.0f;
    std::mt19937 m_rng;
};

} // namespace Aura::DSP::Synthesis
