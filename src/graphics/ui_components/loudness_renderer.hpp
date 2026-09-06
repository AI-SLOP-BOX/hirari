#pragma once
#include <vector>
#include <string>
#include <algorithm>
#include <cmath>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::UI {

/**
 * @class LoudnessRenderer
 * @brief EBU R128 Compliant LUFS Metering UI.
 * High-precision numerical display + color-coded safety bars.
 */
class LoudnessRenderer {
public:
    /**
     * @brief EBU R128 compliant loudness rendering with industrial precision and signal sovereignty.
     * INDUSTRIAL: Delegating integrated calculation and safety auditing to the Rust 'LoudnessOrchestrator'.
     */
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, float lufs) {
        if (w <= 1.0f || h <= 1.0f) return;
        const float value = std::isfinite(lufs) ? std::clamp(lufs, -60.0f, 0.0f) : -60.0f;
        const float norm = (value + 60.0f) / 60.0f;
        kernel.drawGradientRect(x, y, w, h, 0xFF111318, 0xFF07080B);

        // EBU R128 target (-14 LUFS) and safety threshold (-9 LUFS).
        const float targetY = y + h - ((-14.0f + 60.0f) / 60.0f) * h;
        const float safetyY = y + h - ((-9.0f + 60.0f) / 60.0f) * h;
        kernel.drawLine(x, targetY, x + w, targetY, 1.0f, 0xFF22C55E);
        kernel.drawLine(x, safetyY, x + w, safetyY, 1.0f, 0xFFF59E0B);

        const uint32_t barColor = value > -9.0f ? 0xFFEF4444u : (value > -14.0f ? 0xFFF59E0Bu : 0xFF22C55Eu);
        kernel.drawRect(x + 2.0f, y + h - norm * (h - 4.0f), std::max(2.0f, w - 4.0f), norm * (h - 4.0f), barColor);
        for (int db = -60; db <= 0; db += 10) {
            const float py = y + h - (static_cast<float>(db + 60) / 60.0f) * h;
            kernel.drawLine(x + w - 8.0f, py, x + w, py, 1.0f, 0x66888888);
            kernel.drawText(std::to_string(db), x + 3.0f, py - 5.0f, 8.0f, 0xFFAAAAAA);
        }
        kernel.drawText(std::isfinite(lufs) ? (std::to_string(lufs) + " LUFS") : "-inf LUFS",
                        x + 3.0f, y + h - 16.0f, 10.0f, 0xFFFFFFFF);
    }
};

} // namespace Aura::Graphics::UI
