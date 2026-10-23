#pragma once

#include <cstddef>
#include <utility>
#include "../../composition/harmonic_context_tracker.hpp"
#include "../../mixing/aesthetic_evaluator_kernel.hpp"
#include "../../rust_ffi.hpp"

namespace Hirari::Core::DSP::Effects {

/** Host adapter for the Rust evolving multi-delay soundscape processor. */
class AtmosphericProcessorKernel {
public:
    explicit AtmosphericProcessorKernel(size_t numDelays = 4)
        : m_state(hirari_atmospheric_processor_create(numDelays)) {}
    ~AtmosphericProcessorKernel() { hirari_atmospheric_processor_destroy(m_state); }
    AtmosphericProcessorKernel(const AtmosphericProcessorKernel& other)
        : m_state(hirari_atmospheric_processor_clone(other.m_state)) {}
    AtmosphericProcessorKernel& operator=(const AtmosphericProcessorKernel& other) {
        if (this == &other) return *this;
        void* replacement = hirari_atmospheric_processor_clone(other.m_state);
        if (replacement) {
            hirari_atmospheric_processor_destroy(m_state);
            m_state = replacement;
        }
        return *this;
    }
    AtmosphericProcessorKernel(AtmosphericProcessorKernel&& other) noexcept
        : m_state(std::exchange(other.m_state, nullptr)) {}
    AtmosphericProcessorKernel& operator=(AtmosphericProcessorKernel&& other) noexcept {
        if (this != &other) {
            hirari_atmospheric_processor_destroy(m_state);
            m_state = std::exchange(other.m_state, nullptr);
        }
        return *this;
    }

    void process(float* buffer, uint32_t samples, float decay = 0.5f) {
        const auto harmony = Composition::HarmonicContextTracker::getInstance().getState();
        const auto aesthetic = Mixing::AestheticEvaluatorKernel::getInstance().analyze(
            buffer, buffer, samples);
        hirari_atmospheric_processor_process(m_state, buffer, samples, decay,
                                              harmony.tension, aesthetic.transientClarity);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::DSP::Effects
