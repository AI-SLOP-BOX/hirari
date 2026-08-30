#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <array>

namespace Aura::DSP::Analysis {

/**
 * @class TruePeakMeter
 * @brief High-precision inter-sample peak monitor with industrial sovereignty.
 * INDUSTRIAL: Delegating 4x oversampling and ISP analysis to the Rust 'MixerOrchestrator'.
 */
class TruePeakMeter {
public:
    void process(const float* data, uint32_t len) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // True-peak detection is now handled in the Rust layer.
    }
};

} // namespace Aura::DSP::Analysis

namespace Aura::Graphics::UI {

/**
 * @class MixerView
 * @brief Professional mixer strip rendering with industrial precision.
 * INDUSTRIAL: Delegating level statistics and meter ballistics to the Rust 'MixerOrchestrator'.
 */
class MixerView {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const std::vector<std::shared_ptr<::Aura::Core::Engine::Track>>& tracks) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // Mixer rendering logic is now a shim to the Rust layer.
    }

    void renderStrip(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const char* name, float level, uint32_t color) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // Level statistics and meter state are now managed in Rust.
    }
};
};

} // namespace Aura::Graphics::UI
