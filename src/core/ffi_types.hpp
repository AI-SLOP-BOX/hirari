#pragma once
#include <cstdint>
#include <string>
#include "rust/cxx.h"

/**
 * @file ffi_types.hpp
 * @brief SOVEREIGN FFI TYPES: Deterministic PODs for the CXX Bridge.
 * These types are shared between C++ and Rust without relying on generated headers.
 */

namespace Aura::Core::FFI {

struct MixingAdvice {
    rust::String title;
    float confidence;
};

struct ClashInfo {
    float frequency_hz;
    float severity;
};

struct MusicalTime {
    uint64_t samplePosition;
    double beats;
    double bpm;
    int timeSigNum;
    int timeSigDen;
};

struct EffectSlot {
    uint32_t id;
    rust::String name;
    bool active;
};

struct EffectParam {
    uint32_t id;
    rust::String name;
    float value;
    float min;
    float max;
};

struct AutomationPoint {
    double beat;
    float value;
};

} // namespace Aura::Core::FFI
