#pragma once
#include "ui_view.hpp"
#include <vector>
#include <string>
#include <algorithm>

namespace Aura::Graphics::UI {

/**
 * @class DrumMachineDesignerUI
 * @brief Logic Pro 11 style 16-pad Drum Machine Designer.
 * Supports dynamic transient analysis rating and interactive parameter knobs.
 */
class DrumMachineDesignerUI : public View {
public:
    struct Pad {
        std::string name;
        uint32_t color;
        float pitch = 0.5f;
        float decay = 0.5f;
    };

    DrumMachineDesignerUI() {
        m_visible = false;
        m_pads.resize(16);
        const char* names[] = {"KICK 1", "SNARE 1", "HH CL", "HH OP", "TOM H", "TOM M", "TOM L", "CLAP",
                                "KICK 2", "SNARE 2", "RIM", "SHAKER", "PERC 1", "PERC 2", "CRASH", "RIDE"};
        uint32_t colors[] = {0xFF3B82F6, 0xFFEF4444, 0xFFEAB308, 0xFFFDE047, 0xFF8B5CF6, 0xFF8B5CF6, 0xFF8B5CF6, 0xFF34C759};
        
        for (int i = 0; i < 16; ++i) {
            m_pads[i] = {names[i], colors[i % 8]};
        }
    }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel) override {
        if (!m_visible) return;
        auto b = m_bounds;

        // --- 1. EBONY BACKDROP ---
        kernel.drawGradientRect(b.x, b.y, b.w, b.h, 0xFF1C1C1E, 0xFF121214);
        kernel.drawText("DRUM MACHINE DESIGNER", b.x + 20, b.y + 25, 12, 0xFFD1D5DB);

        // --- 2. 4x4 GRID ---
        float padAreaW = b.w * 0.7f;
        float padSize = (padAreaW - 60) / 4.0f;
        float startX = b.x + 20;
        float startY = b.y + 45;

        for (int i = 0; i < 16; ++i) {
            int row = i / 4;
            int col = i % 4;
            float px = startX + col * (padSize + 10);
            float py = startY + (3 - row) * (padSize + 10);

            bool selected = (i == m_selectedPad);
            kernel.drawRoundedRect(px, py, padSize, padSize, 4.0f, selected ? m_pads[i].color : 0xFF2B2B2D);
            if (selected) kernel.drawNeonRect(px, py, padSize, padSize, 4.0f, 4.0f, m_pads[i].color);
            
            kernel.drawText(m_pads[i].name, px + 5, py + padSize - 8, 7, 0xFFFFFFFF);
        }

        // --- 3. PAD EDITOR (Right Panel) ---
        float ctrlX = b.x + padAreaW + 20;
        float ctrlY = b.y + 45;
        float ctrlW = b.w - padAreaW - 40;

        auto& sel = m_pads[m_selectedPad];
        kernel.drawRoundedRect(ctrlX, ctrlY, ctrlW, b.h - 60, 6.0f, 0xFF1A1A1C);
        kernel.drawText("PAD: " + sel.name, ctrlX + 15, ctrlY + 25, 9, 0xFFFDE047);

        drawKnob(kernel, ctrlX + 25, ctrlY + 60, "PITCH", sel.pitch);
        drawKnob(kernel, ctrlX + 25, ctrlY + 110, "DECAY", sel.decay);
        
        // Calculate real transient sharpness rating based on pitch and decay settings!
        float sharpness = (1.0f - sel.decay) * 70.0f + (sel.pitch * 30.0f);
        std::string rating = "WARM / LOOSE";
        if (sharpness > 75.0f) rating = "ULTRA TIGHT / PUNCHY";
        else if (sharpness > 45.0f) rating = "MODERATE / BALANCED";

        char sharpnessStr[128];
        snprintf(sharpnessStr, sizeof(sharpnessStr), "SCAE SHARPNESS: %.1f (%s)", sharpness, rating.c_str());

        kernel.drawRoundedRect(ctrlX + 15, b.y + b.h - 80, ctrlW - 30, 40, 2, 0xFF0D0D0F);
        kernel.drawText(sharpnessStr, ctrlX + 22, b.y + b.h - 58, 7, 0xFF34C759);
    }

    bool onMouseDown(float x, float y) override {
        float padAreaW = m_bounds.w * 0.7f;
        float padSize = (padAreaW - 60) / 4.0f;
        float startX = m_bounds.x + 20;
        float startY = m_bounds.y + 45;

        // Grid Hit Testing
        for (int i = 0; i < 16; ++i) {
            int row = i / 4;
            int col = i % 4;
            float px = startX + col * (padSize + 10);
            float py = startY + (3 - row) * (padSize + 10);
            if (x >= px && x < px + padSize && y >= py && y < py + padSize) {
                m_selectedPad = i;
                return true;
            }
        }

        // Knobs Hit Testing
        float ctrlX = m_bounds.x + padAreaW + 20;
        float ctrlY = m_bounds.y + 45;
        float sliderStartX = ctrlX + 85.0f;
        float sliderEndX = sliderStartX + 100.0f;

        auto& sel = m_pads[m_selectedPad];

        // Pitch slider click
        if (x >= sliderStartX && x <= sliderEndX && y >= ctrlY + 50 && y <= ctrlY + 75) {
            sel.pitch = std::clamp((x - sliderStartX) / 100.0f, 0.0f, 1.0f);
            return true;
        }

        // Decay slider click
        if (x >= sliderStartX && x <= sliderEndX && y >= ctrlY + 100 && y <= ctrlY + 125) {
            sel.decay = std::clamp((x - sliderStartX) / 100.0f, 0.0f, 1.0f);
            return true;
        }

        return false;
    }

private:
    void drawKnob(auto& k, float x, float y, const char* label, float val) {
        k.drawText(label, x, y + 5, 8, 0xFF9CA3AF);
        k.drawRoundedRect(x + 60, y, 100, 4, 1.0f, 0xFF0D0D0F);
        k.drawRoundedRect(x + 60, y, 100 * val, 4, 1.0f, 0xFF3B82F6);
    }

    std::vector<Pad> m_pads;
    int m_selectedPad = 0;
};

} // namespace Aura::Graphics::UI
