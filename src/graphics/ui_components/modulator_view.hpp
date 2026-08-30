#pragma once
#include <vector>
#include <string>
#include <cmath>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::UI {

/**
 * @class ModulatorView
 * @brief Bitwig/Ableton-style Modulation Overlay.
 * Visualizes LFOs, Envelopes, and Step Modulators mapping to any parameter.
 */
class ModulatorView {
public:
    struct ModState {
        std::string name;
        float value; // 0.0 to 1.0
        uint32_t color;
        std::string target;
    };

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h,
                const std::vector<ModState>& mods, double animationTimeSeconds = 0.0) {
        if (mods.empty()) return;

        // --- 1. MODULAR GRID GLASS ---
        kernel.drawDropShadow(x, y, w, h, 8.0f, 0x99000000);
        kernel.drawGradientRect(x, y, w, h, 0xDD1C1C1E, 0xCC111113);
        kernel.drawRoundedRect(x, y, w, h, 8.0f, 0x22FFFFFF);

        // --- 2. PATCH CABLES (Animated Bezier) ---
        // HONEST FIX: Replaces 'static lines' with high-end Bitwig-style glowing cables.
        for (size_t i = 0; i < mods.size() - 1; ++i) {
             float p0x = x + 40 + i * 90;
             float p0y = y + 50;
             float p1x = x + 40 + (i + 1) * 90;
             float p1y = y + 50;
             
             // Draw cable (Shadow + Glow + Core)
             kernel.drawBezierCurve(p0x, p0y, p0x + 45, p0y + 100, p1x - 45, p1y + 100, p1x, p1y, 4.0f, 0x44000000); // Shadow
             kernel.drawBezierCurve(p0x, p0y, p0x + 45, p0y + 100, p1x - 45, p1y + 100, p1x, p1y, 2.0f, mods[i].color & 0x77FFFFFF); // Glow
             kernel.drawBezierCurve(p0x, p0y, p0x + 45, p0y + 100, p1x - 45, p1y + 100, p1x, p1y, 0.8f, 0xFFFFFFFF); // Core
             
             // --- SIGNAL FLOW (Moving Spark) ---
             const float time = std::isfinite(animationTimeSeconds)
                 ? static_cast<float>(animationTimeSeconds) : 0.0f;
             float progress = std::fmod(static_cast<float>(i) * 0.2f + time, 1.0f);
             if (progress < 0.0f) progress += 1.0f;
             float spX, spY; // Calculate cubic bezier point at t=progress
             kernel.calculateBezier(p0x, p0y, p0x + 45, p0y + 100, p1x - 45, p1y + 100, p1x, p1y, progress, spX, spY);
             kernel.drawCircle(spX, spY, 2.2f, mods[i].color);
             kernel.drawNeonRect(spX - 2, spY - 2, 4, 4, 1, 4.0f, mods[i].color);
        }

        // --- 3. MODULATOR NODES ---
        float itemW = 80.0f, itemH = 60.0f, padding = 10.0f;
        for (size_t i = 0; i < mods.size(); ++i) {
             float ix = x + 10 + i * (itemW + padding);
             float iy = y + 40;
             renderNode(kernel, ix, iy, itemW, itemH, mods[i]);
        }
    }

    void renderNode(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const ModState& mod) {
        kernel.drawRoundedRect(x, y, w, h, 6.0f, 0xFF0D0D0F);
        kernel.drawRoundedRect(x, y, w, h, 6.0f, 0x1130B0FF); // Port Highlight
        
        // Animated Indicator
        float r = 16.0f;
        kernel.drawCircle(x + w/2, y + h/2 - 4, r, 0xFF121214);
        kernel.drawArc(x + w/2, y + h/2 - 4, r - 2, -M_PI_2, -M_PI_2 + (mod.value * M_PI * 2), 2.5f, mod.color);
        
        kernel.drawText(mod.name, x + 6, y + h - 14, 7, 0xFFBBBBBB);
    }
};

} // namespace Aura::Graphics::UI
