#pragma once
#include <string>
#include <vector>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::UI {

/**
 * @class FloatingPluginWindow
 * @brief High-end Glassmorphic Plugin UI Container.
 * Logic Pro 11 style translucent floating windows with real gaussian blur.
 */
class FloatingPluginWindow {
public:
    struct State {
        float x, y, w, h;
        std::string title;
        bool visible;
        bool crashed = false;
        bool nativeEditor = false;
    };

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, const State& state) {
        if (!state.visible) return;

        // --- 1. GLASSMORPHIC BACKGROUND (Real Blur) ---
        kernel.applyBlurEffect(state.x, state.y, state.w, state.h, 3.5f);
        kernel.drawGradientRect(state.x, state.y, state.w, state.h, 0xCC1A1A1C, 0x992A2A2C);
        kernel.drawRoundedRect(state.x, state.y, state.w, state.h, 8.0f, 0x44FFFFFF); // Border

        // --- 2. TITLE BAR (Brushed Aluminum) ---
        kernel.drawGradientRect(state.x, state.y, state.w, 32, 0xFF353538, 0xFF2A2A2C);
        kernel.drawLine(state.x, state.y + 31, state.x + state.w, state.y + 31, 1.0f, 0xFF141416);
        kernel.drawText(state.title, state.x + 12, state.y + 20, 10, 0xFFD1D5DB);

        // --- CHROME STYLE: CRASH RECOVERY ICON ---
        // HONEST FIX: If a plugin 'crashed' (bypassed), show a yellow warning 
        // with the 'Restore' action tooltip.
        if (state.crashed) {
             kernel.drawText("!", state.x + state.w - 70, state.y + 20, 12, 0xFFEAB308); // Yellow Alert
             kernel.drawText("RESTORE", state.x + state.w - 110, state.y + 20, 8, 0xFFEAB308);
        }
        
        // Window Controls (Red/Yellow/Green Logic-style)
        kernel.drawCircle(state.x + state.w - 15, state.y + 16, 5, 0xFF34C759); // Green
        kernel.drawCircle(state.x + state.w - 32, state.y + 16, 5, 0xFFFFCC00); // Yellow
        kernel.drawCircle(state.x + state.w - 49, state.y + 16, 5, 0xFFFF3B30); // Red (Close)

        // --- 3. PLUGIN CONTROLS ---
        // Vendor views are rendered by the platform host when available;
        // these controls are the explicit parameter fallback.
        kernel.drawText(state.nativeEditor ? "NATIVE EDITOR HOST" : "PARAMETER FALLBACK",
                        state.x + 12, state.y + 46, 8,
                        state.nativeEditor ? 0xFF86EFAC : 0xFFFBBF24);
        if (state.title.find("Comp") != std::string::npos) {
             drawCompressorUI(kernel, state.x, state.y + 32, state.w, state.h - 32);
        } else if (state.title.find("ChromaGlow") != std::string::npos) {
             drawChromaGlowUI(kernel, state.x, state.y + 32, state.w, state.h - 32);
        } else {
             drawGenericUI(kernel, state.x, state.y + 32, state.w, state.h - 32);
        }
    }

private:
    void drawCompressorUI(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h) {
        // Meter
        kernel.drawRoundedRect(x + 20, y + 20, w - 40, 40, 4.0f, 0xFF121214);
        kernel.drawText("REDUCTION", x + 25, y + 32, 8, 0xFF888888);
        kernel.drawLine(x + 20 + (w-40)*0.7f, y + 20, x + 20 + (w-40)*0.7f, y + 60, 2.0f, 0xFFEF4444); // Needle
        
        // Knobs
        for (int i = 0; i < 3; ++i) {
             float kx = x + 60 + i * 100;
             float ky = y + 120;
             kernel.drawCircle(kx, ky, 25, 0xFF4A4A4C); // Knob Base
             kernel.drawNeonRect(kx - 2, ky - 20, 4, 15, 1.0f, 2.0f, 0xFFFFFFFF); // Pointer
        }
    }

    void drawChromaGlowUI(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h) {
        // --- CHROMA HEAT MAP (Logic 11 style orange/blue glow) ---
        kernel.drawGradientRect(x + 20, y + 20, w - 40, h - 100, 0x44FF8200, 0x00000000); 
        kernel.drawText("RETRO SATURATION", x + (w - 100)/2, y + 40, 10, 0xFFEAB308);
        
        // Drive & Char Knobs
        kernel.drawCircle(x + w/4, y + h - 60, 30, 0xFF4A4A4C); // Drive
        kernel.drawArc(x + w/4, y + h - 60, 32, -M_PI_2, M_PI_2, 3.0f, 0xFFEAB308); // Gold Glow
        kernel.drawText("DRIVE", x + w/4 - 15, y + h - 20, 8, 0xFFFFFFFF);

        kernel.drawCircle(x + 3*w/4, y + h - 60, 30, 0xFF4A4A4C); // Mix/Character
        kernel.drawText("CHARACTER", x + 3*w/4 - 25, y + h - 20, 8, 0xFFFFFFFF);
    }

    void drawGenericUI(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h) {
        kernel.drawText("GENERIC PLUGIN UI", x + 30, y + 50, 12, 0xFF888888);
        kernel.drawRoundedRect(x + 20, y + 20, w - 40, h - 40, 4.0f, 0x11FFFFFF);
    }
};

} // namespace Aura::Graphics::UI
