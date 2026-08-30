#pragma once

#include <vector>
#include <array>
#include <atomic>
#include <random>
#include <algorithm>
#include "../../core/midi_buffer.hpp"

namespace Aura::Core::Engine {

/**
 * @class StepSequencer
 * @brief Logic Pro Style High-Density Polyphonic Pattern Engine.
 * Manages Step Grid data and generates sample-accurate MIDI Note triggers.
 */
class StepSequencer {
public:
    static constexpr int kMaxSteps = 64;
    static constexpr int kMaxLanes = 16; 

    struct PendingTrigger {
        bool active = false;
        uint32_t lane = 0;
        uint32_t step = 0;
        uint64_t sampleOffset = 0;
    };

    struct PendingNoteOff {
        bool active = false;
        uint8_t pitch = 0;
        uint64_t offSamplePos = 0; // Absolute sample position
    };

    StepSequencer() : m_rng(std::random_device{}()) { reset(); initDefaults(); }

    void reset() {
        for (int l = 0; l < kMaxLanes; ++l) {
            for (int s = 0; s < kMaxSteps; ++s) {
                m_lanes[l][s].store(false, std::memory_order_relaxed);
                m_laneVelocities[l][s].store(100, std::memory_order_relaxed);
                m_stepProbability[l][s].store(100, std::memory_order_relaxed);
                m_stepSubsteps[l][s].store(1, std::memory_order_relaxed);
                m_stepOffsets[l][s].store(0.0f, std::memory_order_relaxed);
            }
        }
        m_numActiveNotes.store(0);
        for (auto& p : m_pendingPool) p.active = false;
        for (auto& o : m_activeNoteOffs) o.active = false;
    }

    void initDefaults() {
        m_active = true;
        m_lanes[0][0].store(true);
        m_lanes[0][4].store(true);
        m_lanes[0][8].store(true);
        m_lanes[0][12].store(true);
        
        m_lanes[1][4].store(true);
        m_lanes[1][12].store(true);
        
        for (int s = 0; s < 16; s += 2) {
            m_lanes[2][s].store(true);
        }
    }

    void process(MidiBuffer& midi, uint64_t currentPos, uint32_t numSamples, double bpm, double sr) {
        if (!m_active) return;
        
        double stepDurationSamples = (60.0 / bpm / 4.0) * sr;
        if (stepDurationSamples <= 0.0) return;

        // 1. Process active note-offs that fall in this block to prevent stuck notes
        for (auto& noteOff : m_activeNoteOffs) {
            if (noteOff.active) {
                if (noteOff.offSamplePos >= currentPos && noteOff.offSamplePos < currentPos + numSamples) {
                    uint64_t offOffset = noteOff.offSamplePos - currentPos;
                    midi.addNoteOff(1, noteOff.pitch, offOffset);
                    noteOff.active = false;
                } else if (noteOff.offSamplePos < currentPos) {
                    // Fallback to prevent hang if transport jumped
                    midi.addNoteOff(1, noteOff.pitch, 0);
                    noteOff.active = false;
                }
            }
        }

        // 2. Trigger new note-ons and register their note-offs
        uint64_t startStep = static_cast<uint64_t>(std::floor(static_cast<double>(currentPos) / stepDurationSamples));
        uint64_t endStep = static_cast<uint64_t>(std::floor(static_cast<double>(currentPos + numSamples) / stepDurationSamples));

        const uint8_t lanePitches[kMaxLanes] = { 36, 38, 42, 46, 39, 41, 43, 45, 47, 48, 49, 50, 51, 52, 53, 54 };

        for (uint64_t s = startStep; s <= endStep; ++s) {
            uint32_t stepIdx = static_cast<uint32_t>(s % kMaxSteps);

            // Calculate timing parameters: Swing & Offsets
            double swingOffset = 0.0;
            if (stepIdx % 2 == 1) { // Swing applied to offbeats
                swingOffset = m_swingAmount * 0.5 * stepDurationSamples;
            }

            for (int l = 0; l < kMaxLanes; ++l) {
                if (m_lanes[l][stepIdx].load(std::memory_order_relaxed)) {
                    // Probability check
                    uint32_t prob = m_stepProbability[l][stepIdx].load(std::memory_order_relaxed);
                    if (prob < 100) {
                        // Use persistent m_rng (seeded once) for true stochastic behaviour
                        std::uniform_int_distribution<uint32_t> dist(0, 100);
                        if (dist(m_rng) > prob) continue;
                    }

                    // Substeps & Offsets
                    uint32_t substeps = std::max(1u, m_stepSubsteps[l][stepIdx].load(std::memory_order_relaxed));
                    float offsetPct = m_stepOffsets[l][stepIdx].load(std::memory_order_relaxed);
                    double userOffset = offsetPct * stepDurationSamples;
                    double totalOffset = swingOffset + userOffset;

                    double substepInterval = stepDurationSamples / substeps;

                    for (uint32_t sub = 0; sub < substeps; ++sub) {
                        double substepSamplePos = static_cast<double>(s * stepDurationSamples) + totalOffset + (sub * substepInterval);
                        
                        if (substepSamplePos >= currentPos && substepSamplePos < currentPos + numSamples) {
                            uint64_t triggerOffset = static_cast<uint64_t>(substepSamplePos - currentPos);
                            uint8_t pitch = lanePitches[l];
                            uint8_t vel = m_laneVelocities[l][stepIdx].load(std::memory_order_relaxed);
                            
                            midi.addNoteOn(1, pitch, vel, triggerOffset);

                            // Schedule Note Off (80% gate length or 1000 samples)
                            uint64_t offSamplePos = static_cast<uint64_t>(substepSamplePos + std::min(stepDurationSamples * 0.8, 1000.0));

                            // Find inactive slot in the activeNoteOffs pool
                            bool registered = false;
                            for (auto& noteOff : m_activeNoteOffs) {
                                if (!noteOff.active) {
                                    noteOff.active = true;
                                    noteOff.pitch = pitch;
                                    noteOff.offSamplePos = offSamplePos;
                                    registered = true;
                                    break;
                                }
                            }
                            // If pool is full, send note off immediately at the end of block to prevent stuck notes
                            if (!registered) {
                                midi.addNoteOff(1, pitch, numSamples - 1);
                            }
                        }
                    }
                }
            }
        }
    }

    void setSwing(float amount) { m_swingAmount = amount; }
    void setActive(bool a) { m_active = a; }

    bool getStep(int lane, int step) const {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return false;
        return m_lanes[lane][step].load(std::memory_order_relaxed);
    }

    void setStep(int lane, int step, bool active) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        m_lanes[lane][step].store(active, std::memory_order_relaxed);
    }

    void setStepProbability(int lane, int step, uint32_t prob) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        m_stepProbability[lane][step].store(std::min(100u, prob), std::memory_order_relaxed);
    }

    void setStepSubsteps(int lane, int step, uint32_t subs) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        m_stepSubsteps[lane][step].store(std::max(1u, subs), std::memory_order_relaxed);
    }

    void setStepOffset(int lane, int step, float offset) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        m_stepOffsets[lane][step].store(std::clamp(offset, -0.5f, 0.5f), std::memory_order_relaxed);
    }

private:
    bool m_active = false;
    float m_swingAmount = 0.0f;
    mutable std::mt19937 m_rng; // Seeded once — persistent for true stochastic behaviour

    std::array<std::array<std::atomic<bool>, kMaxSteps>, kMaxLanes> m_lanes;
    std::array<std::array<std::atomic<uint32_t>, kMaxSteps>, kMaxLanes> m_laneVelocities;
    std::array<std::array<std::atomic<uint32_t>, kMaxSteps>, kMaxLanes> m_stepProbability;
    std::array<std::array<std::atomic<uint32_t>, kMaxSteps>, kMaxLanes> m_stepSubsteps;
    std::array<std::array<std::atomic<float>, kMaxSteps>, kMaxLanes> m_stepOffsets;
    std::atomic<uint32_t> m_numActiveNotes;
    std::array<PendingTrigger, 256> m_pendingPool;
    std::array<PendingNoteOff, 128> m_activeNoteOffs;
};

} // namespace Aura::Core::Engine
