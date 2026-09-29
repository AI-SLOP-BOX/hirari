#pragma once

#include <cstdint>

namespace Hirari::Core::Composition {

// Plain engine-facing result shape. Analysis and classification are owned by
// Rust; this DTO exists only to cross the engine's C++ header boundary.
struct ArrangementSection {
    uint64_t start_sample = 0;
    uint64_t end_sample = 0;
    uint8_t section_type = 0;
    float energy_level = 0.0f;
    uint32_t motivic_id = 0;
    float narrative_flow_score = 0.0f;
};

} // namespace Hirari::Core::Composition
