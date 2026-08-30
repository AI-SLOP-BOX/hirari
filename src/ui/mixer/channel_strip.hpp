#pragma once
#include <string>
#include <vector>
#include <optional>
#include "../../graphics/graphics_kernel.hpp"
#include "../../core/engine/track.hpp"

namespace Aura::UI::Mixer {

enum class ControlId {
    None,
    Fader,
    Pan,
    Mute,
    Solo,
    FXSlot
};

struct HitResult {
    ControlId id = ControlId::None;
    int index = -1;
};

/**
 * @class ChannelStrip
 * @brief High-Fidelity Interactive Mixer Component.
 * HONEST FIX: Implemented hit-testing for professional interaction.
 */
class ChannelStrip {
public:
    struct Layout {
        float pad;
        float fxTop;
        float fxHeight;
        float slotHeight;
        float panY;
        float panRadius;
        float fAreaY;
        float fAreaBottom;
        float fLeft;
        float fRight;
    };

    static Layout layout(float w, float h) noexcept {
        const float pad = std::max(4.0f, h * 0.025f);
        const float fxHeight = std::clamp(h * 0.18f, 72.0f, 142.0f);
        const float fAreaY = h * 0.38f;
        return {pad, pad, fxHeight, fxHeight / 6.0f, h * 0.31f, 15.0f,
                fAreaY, h - h * 0.12f, w * 0.16f, w * 0.84f};
    }

    /**
     * @brief Identifies the control at the given local coordinates.
     */
    HitResult hitTest(float lx, float ly, float w, float h) const {
        // --- 1. FX SLOTS (Y: 10 to 142) ---
        const Layout l = layout(w, h);
        if (ly >= l.fxTop && ly <= l.fxTop + l.fxHeight) {
            int idx = static_cast<int>((ly - l.fxTop) / l.slotHeight);
            if (idx >= 0 && idx < 6) return { ControlId::FXSlot, idx };
        }

        // --- 2. PAN DIAL (Y: 170 +/- 15) ---
        if (std::abs(ly - l.panY) < l.panRadius && std::abs(lx - w/2.0f) < l.panRadius) {
            return { ControlId::Pan };
        }

        // --- 3. VOLUME FADER (X: 16 to 50, Y: 200 to h-45) ---
        if (lx >= l.fLeft && lx <= l.fRight && ly >= l.fAreaY && ly <= l.fAreaBottom) {
            return { ControlId::Fader };
        }

        return { ControlId::None };
    }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const Core::Engine::Track& track) {
        // --- RENDER LOGIC ---
        // (Same high-fidelity rendering as before, ensuring consistency with hitTest)
        kernel.drawGradientRect(x, y, w, h, 0xFF1C1C1E, 0xFF141416);
        
        const Layout l = layout(w, h);
        const float slotX = x + std::max(4.0f, w * 0.08f);
        const float slotW = w - 2.0f * (slotX - x);
        for (int i = 0; i < 6; ++i) {
            kernel.drawRoundedRect(slotX, y + l.fxTop + i * l.slotHeight,
                                   slotW, std::max(1.0f, l.slotHeight - 2.0f),
                                   3.0f, 0xFF0D0D0F);
        }

        kernel.drawBrushedCircle(x + w/2, y + l.panY, 12, 0xFF4B4B4E);

        const float fAreaH = l.fAreaBottom - l.fAreaY;
        kernel.drawRect(x + l.fLeft, y + l.fAreaY, 4, fAreaH, 0xFF050505);

        // Fader Cap
        float vol = track.getVolume();
        float capY = y + l.fAreaY + fAreaH * (1.0f - std::clamp(vol, 0.0f, 1.0f));
        kernel.drawGradientRect(x + l.fLeft - 16, capY - 14, 34, 28, 0xFFE5E7EB, 0xFF9CA3AF);
        
        // Track Name
        kernel.drawRect(x, y + h - 30, w, 30, 0xFF111113);
        kernel.drawText(track.getName(), x + 8, y + h - 10, 10, 0xFFF1F5F9);
    }
};

} // namespace Aura::UI::Mixer
