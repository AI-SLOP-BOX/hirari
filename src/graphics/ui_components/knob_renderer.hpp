#pragma once
#include <string>
#include <cmath>
#include <algorithm>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::UI {

/**
 * @class KnobRenderer
 * @brief Professional DAW Knob with Glassmorphic styling.
 * HONEST FIX: Implemented real arc-based visualization and natural mapping.
 */
class KnobRenderer {
public:
    struct Config {
        float min = 0.0f;
        float max = 1.0f;
        float defaultValue = 0.5f;
        std::string label;
        std::string unit;
        uint32_t activeColor = 0xFFEAB308; // Amber
    };

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float cx, float cy, float radius, float value, const Config& cfg) {
        // --- 1. OUTER RING (Track) ---
        kernel.drawCircle(cx, cy, radius, 0x22FFFFFF);
        
        // --- 2. ACTIVE ARC (Value Representation) ---
        float normalized = (value - cfg.min) / (cfg.max - cfg.min + 1e-6f);
        float startAngle = -M_PI * 1.25f; // ~7:30 position
        float endAngle = startAngle + normalized * (M_PI * 1.5f);
        
        kernel.drawArc(cx, cy, radius, startAngle, endAngle, 3.0f, cfg.activeColor);

        // --- 3. KNOB CAP (Glassmorphic) ---
        float capRadius = radius * 0.8f;
        kernel.drawGlassRect(cx - capRadius, cy - capRadius, capRadius * 2, capRadius * 2, capRadius, 0x44FFFFFF);
        kernel.drawBrushedCircle(cx, cy, capRadius - 2.0f, 0xFF2A2A2C);
        
        // Indicator Line
        float ix = cx + std::cos(endAngle) * (capRadius - 5.0f);
        float iy = cy + std::sin(endAngle) * (capRadius - 5.0f);
        kernel.drawLine(cx, cy, ix, iy, 2.0f, 0xFFFFFFFF);

        // --- 4. LABEL & VALUE ---
        kernel.drawText(cfg.label, cx - 20, cy + radius + 15, 8, 0xFFAAAAAA);
        
        char valText[32];
        snprintf(valText, 32, "%.2f %s", value, cfg.unit.c_str());
        kernel.drawText(valText, cx - 25, cy + radius + 28, 7, 0xFFD1D5DB);
    }
};

} // namespace Aura::Graphics::UI
