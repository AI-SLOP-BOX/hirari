#pragma once
#include <vector>
#include <string>
#include <algorithm>
#include <cmath>
#include "../graphics_kernel.hpp"
#include "../../dsp/analysis/goniometer.hpp"

namespace Aura::Graphics::UI {

/**
 * @class GoniometerRenderer
 * @brief Professional Stereo Field Analyzer (Phase Scope).
 * Visualizes phase relationship and energy distribution using a rotated X-Y plot.
 */
class GoniometerRenderer {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const DSP::Analysis::Goniometer::Data& data) {
        // --- 1. CIRCULAR SCOPE BACKDROP ---
        float cx = x + w / 2.0f, cy = y + h / 2.0f;
        float radius = std::min(w, h) * 0.45f;
        
        kernel.drawGradientRect(x, y, w, h, 0xFF0A0A0C, 0xFF141416);
        kernel.drawCircle(cx, cy, radius, 0x11FFFFFF); // Outer guide
        
        // Guide Lines (M/S, L/R)
        kernel.drawLine(cx - radius, cy, cx + radius, y + h / 2, 1.0f, 0x22FFFFFF); // Side Axis
        kernel.drawLine(cx, cy - radius, cx, cy + radius, 1.0f, 0x22FFFFFF); // Mid Axis
        kernel.drawText("M", cx - 4, cy - radius - 12, 8, 0xFF666666);
        kernel.drawText("S", cx + radius + 4, cy - 4, 8, 0xFF666666);

        // --- 2. THE NATIVE PHOSPHOR SCOPE (GPU ACCELERATED) ---
        // RADICAL SIMPLIFICATION: Offload Lissajous rendering to Metal/Vulkan shader.
        kernel.drawGoniometer(cx - radius, cy - radius, radius * 2.0f, radius * 2.0f, data.xyHistoryL.data(), data.xyHistoryR.data(), DSP::Analysis::Goniometer::kHistorySize);

        // --- 3. CORRELATION METER (-1 to +1) ---
        float mY = y + h - 15, mW = w - 40, mX = x + 20;
        kernel.drawRoundedRect(mX, mY, mW, 4, 2.0f, 0xFF121214);
        
        float corrPos = mX + mW/2 + (data.correlation * mW/2);
        kernel.drawGradientRect(mX + mW/2, mY, (data.correlation * mW/2), 4, 0xFF22D3EE, 0xFF22D3EE);
        kernel.drawCircle(corrPos, mY + 2, 4, 0xFFFFFFFF);
        kernel.drawText("PHASE", mX, mY - 10, 7, 0xFF666666);
    }
};

} // namespace Aura::Graphics::UI
