#pragma once
#include <string>
#include <vector>
#include <map>
#include "../graphics_kernel.hpp"
#include "../../core/engine/track.hpp"

namespace Aura::Graphics::UI {

/**
 * @class ProfessionalInspector
 * @brief High-Fidelity Inspector for Track and Region parameters.
 * Mirrors Logic Pro 11 ebony-table aesthetics.
 */
class ProfessionalInspector {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y,
                float w, float h,
                const std::vector<std::shared_ptr<Core::Engine::Track>>& tracks) {
        if (tracks.empty() || !tracks.front()) {
            kernel.drawGradientRect(x, y, w, std::min(22.0f, h), 0xFF353538, 0xFF212123);
            kernel.drawText("Inspector: No track selected", x + 10, y + 16, 9.5f, 0xFFF1F5F9);
            return;
        }
        render(kernel, x, y, w, h, *tracks.front());
    }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const Core::Engine::Track& track) {
        
        // --- 1. SECTION: REGION SETTINGS ---
        float currentY = y + 10;
        renderHeader(kernel, x, currentY, w, "Region: Audio Region 1");
        currentY += 24;
        
        renderRow(kernel, x, currentY, w, "Mute", track.isMuted() ? "On" : "Off"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Quantize", "1/16 Note"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Transpose", "0"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Velocity", "0"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Delay", "0 ms"); currentY += 20;
        
        // --- 2. SECTION: TRACK SETTINGS ---
        currentY += 15;
        renderHeader(kernel, x, currentY, w, "Track: " + track.getName());
        currentY += 24;
        
        renderRow(kernel, x, currentY, w, "Icon", "Default"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Channel", "Input 1"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Freeze Mode", "Pre-Fader"); currentY += 20;
        renderRow(kernel, x, currentY, w, "Flex Mode", "Polyphonic"); currentY += 20;
        
        // --- 3. SECTION: MIXER STRIP (CHANNEL) ---
        // This is handled by m_mixer.renderStrip in AuraProUI
    }

private:
    void renderHeader(::Aura::Graphics::Platform::IGraphicsKernel& k, float x, float y, float w, std::string title) {
        k.drawGradientRect(x, y, w, 22, 0xFF353538, 0xFF212123);
        k.drawRect(x, y + 21, w, 1.2f, 0xFF000000);
        k.drawText(title, x + 10, y + 16, 9.5f, 0xFFF1F5F9);
    }
    
    void renderRow(::Aura::Graphics::Platform::IGraphicsKernel& k, float x, float y, float w, std::string label, std::string value) {
        k.drawRect(x, y, w, 20, 0xFF1C1C1E);
        k.drawLine(x, y + 19, x + w, y + 19, 0.5f, 0xFF000000);
        k.drawText(label, x + 12, y + 14, 8.5f, 0xFF94A3B8);
        k.drawText(value, x + w - k.measureText(value, 8.5f) - 12, y + 14, 8.5f, 0xFFE2E8F0);
    }
};

} // namespace Aura::Graphics::UI
