#pragma once
#include <vector>
#include <memory>
#include <algorithm>
#include "../../graphics/graphics_kernel.hpp"
#include "../../core/engine/track.hpp"
#include "channel_strip.hpp"

namespace Aura::UI::Mixer {

/**
 * @class MixerConsole
 * @brief Professional Virtualized Mixer Console.
 * HONEST FIX: Implemented UI virtualization to support unlimited track counts.
 */
class MixerConsole {
public:
    static constexpr float kStripWidth = 120.0f;

    MixerConsole() : m_scrollOffset(0.0f) {}

    void setScrollOffset(float offset) {
        m_scrollOffset = std::max(0.0f, offset);
    }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, 
                const std::vector<std::shared_ptr<::Aura::Core::Engine::Track>>& tracks) {
        
        kernel.drawGradientRect(x, y, w, h, 0xFF141416, 0xFF0A0A0C);
        kernel.drawLine(x, y, x + w, y, 1.0f, 0xFF333333);
        
        // --- INDUSTRIAL VIRTUALIZATION ---
        // Calculate visible range based on scroll offset
        int startIdx = static_cast<int>(m_scrollOffset / kStripWidth);
        int visibleCount = static_cast<int>(w / kStripWidth) + 2; // +2 buffer for smooth scrolling
        int endIdx = std::min(static_cast<int>(tracks.size()), startIdx + visibleCount);

        for (int i = startIdx; i < endIdx; ++i) {
            float stripX = x + (i * kStripWidth) - m_scrollOffset;
            if (stripX + kStripWidth > x && stripX < x + w) {
                renderStrip(kernel, stripX, y, kStripWidth, h, *tracks[i]);
            }
        }
    }

    void renderStrip(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const Core::Engine::Track& track) {
        m_strip.render(kernel, x, y, w, h, track);
    }

private:
    ChannelStrip m_strip;
    float m_scrollOffset;
};

} // namespace Aura::UI::Mixer
