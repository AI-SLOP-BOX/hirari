#pragma once

#include "../core/rust_ffi.hpp"

namespace Hirari::Library::Synthesis {

/** Native compatibility façade for the Rust-owned AHDSR envelope. */
class AHDSR {
public:
    explicit AHDSR(double sampleRate = 44100.0)
        : m_state(hirari_ahdsr_create(sampleRate)) {}

    ~AHDSR() { hirari_ahdsr_destroy(m_state); }

    AHDSR(const AHDSR&) = delete;
    AHDSR& operator=(const AHDSR&) = delete;
    AHDSR(AHDSR&&) = delete;
    AHDSR& operator=(AHDSR&&) = delete;

    void setParameters(float attack, float hold, float decay, float sustain, float release) {
        hirari_ahdsr_set_parameters(m_state, attack, hold, decay, sustain, release);
    }

    void reset() noexcept { hirari_ahdsr_reset(m_state); }
    void trigger() { hirari_ahdsr_trigger(m_state); }
    void release() { hirari_ahdsr_release(m_state); }
    float getNextValue() { return hirari_ahdsr_next_value(m_state); }
    bool isActive() const { return hirari_ahdsr_is_active(m_state); }

private:
    void* m_state;
};

} // namespace Hirari::Library::Synthesis
