#pragma once

#include <cstddef>
#include <cstdint>
#include "rust_ffi.hpp"

namespace Hirari::Core {

/** Native API façade for the Rust-owned lock-free parameter state. */
class AtomicParameter {
public:
    enum class SmoothingType : uint8_t { Linear, Exponential, None };
    enum class DisplayMode : uint8_t { Unipolar, Bipolar };
    enum class Unit : uint8_t { Percentage, Decibels, Frequency, Time, Raw };

    explicit AtomicParameter(float initialValue = 1.0f,
                             DisplayMode mode = DisplayMode::Unipolar)
        : m_state(hirari_atomic_parameter_create(initialValue, static_cast<uint8_t>(mode))) {}

    ~AtomicParameter() { hirari_atomic_parameter_destroy(m_state); }

    AtomicParameter(const AtomicParameter&) = delete;
    AtomicParameter& operator=(const AtomicParameter&) = delete;
    AtomicParameter(AtomicParameter&&) = delete;
    AtomicParameter& operator=(AtomicParameter&&) = delete;

    void getValueString(char* buffer, size_t size) const {
        hirari_atomic_parameter_get_value_string(m_state, buffer, size);
    }

    float getNormalizedValue() const {
        return hirari_atomic_parameter_get_normalized(m_state);
    }

    float getTarget() const noexcept {
        return hirari_atomic_parameter_get_target(m_state);
    }

    void setUnit(Unit unit) {
        hirari_atomic_parameter_set_unit(m_state, static_cast<uint8_t>(unit));
    }

    void setDisplayMode(DisplayMode mode) {
        hirari_atomic_parameter_set_display_mode(m_state, static_cast<uint8_t>(mode));
    }

    float getNextValue() {
        return hirari_atomic_parameter_get_next(m_state);
    }

    void getNextBlock(float* buffer, size_t numSamples) {
        hirari_atomic_parameter_get_block(m_state, buffer, numSamples);
    }

    void setTarget(float value) {
        hirari_atomic_parameter_set_target(m_state, value);
    }

    void setSampleRate(double sampleRate) {
        hirari_atomic_parameter_set_sample_rate(m_state, sampleRate);
    }

    void setSmoothingTime(double milliseconds) {
        hirari_atomic_parameter_set_smoothing_time(m_state, milliseconds);
    }

    void setAIModulation(float offset) {
        hirari_atomic_parameter_set_ai_modulation(m_state, offset);
    }

    void setSmoothingType(SmoothingType type) {
        hirari_atomic_parameter_set_smoothing_type(m_state, static_cast<uint8_t>(type));
    }

    void resetToTarget() noexcept {
        hirari_atomic_parameter_reset_to_target(m_state);
    }

private:
    void* m_state;
};

} // namespace Hirari::Core
