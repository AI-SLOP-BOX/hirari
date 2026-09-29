#pragma once

#include <cstdint>
#include "../midi_buffer.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

// Compatibility facade. Pattern state, timing, probability, MIDI generation,
// and pending note-off ownership live in the Rust audio runtime.
class StepSequencer {
public:
    static constexpr int kMaxSteps = 64;
    static constexpr int kMaxLanes = 16;

    StepSequencer() : m_state(hirari_step_sequencer_create()) {}
    ~StepSequencer() { hirari_step_sequencer_destroy(m_state); }

    StepSequencer(const StepSequencer&) = delete;
    StepSequencer& operator=(const StepSequencer&) = delete;

    void process(MidiBuffer& midi, uint64_t currentPosition, uint32_t numSamples,
                 double bpm, double sampleRate) {
        hirari_step_sequencer_process(m_state, midi.rustStateHandle(), currentPosition,
                                      numSamples, bpm, sampleRate);
    }

    void setSwing(float amount) {
        hirari_step_sequencer_set_swing(m_state, amount);
    }
    void setActive(bool active) {
        hirari_step_sequencer_set_active(m_state, active);
    }

    bool getStep(int lane, int step) const {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return false;
        return hirari_step_sequencer_get_step(
            m_state, static_cast<uint32_t>(lane), static_cast<uint32_t>(step));
    }
    void setStep(int lane, int step, bool active) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        hirari_step_sequencer_set_step(
            m_state, static_cast<uint32_t>(lane), static_cast<uint32_t>(step), active);
    }
    void setStepProbability(int lane, int step, uint32_t probability) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        hirari_step_sequencer_set_probability(
            m_state, static_cast<uint32_t>(lane), static_cast<uint32_t>(step), probability);
    }
    void setStepSubsteps(int lane, int step, uint32_t count) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        hirari_step_sequencer_set_substeps(
            m_state, static_cast<uint32_t>(lane), static_cast<uint32_t>(step), count);
    }
    void setStepOffset(int lane, int step, float offset) {
        if (lane < 0 || lane >= kMaxLanes || step < 0 || step >= kMaxSteps) return;
        hirari_step_sequencer_set_offset(
            m_state, static_cast<uint32_t>(lane), static_cast<uint32_t>(step), offset);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
