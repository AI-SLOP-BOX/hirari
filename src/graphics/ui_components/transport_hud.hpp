#pragma once
#include <string>
#include <vector>
#include <cmath>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::UI {

/**
 * @class TransportHUD
 * @brief Logic Pro style LCD and Transport Controls.
 */
class TransportHUD {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, uint64_t currentPos, bool isPlaying, double sampleRate = 44100.0, float bpm = 120.0f, bool midiActive = false, float cpuLoad = 0.15f) {
        // --- 1. OUTER HUD CONTAINER ---
        kernel.drawDropShadow(x, y, w, h, 6, 0x99000000);
        kernel.drawGradientRect(x, y, w, h, 0xFF323234, 0xFF252526);
        
        // --- 2. TRANSPORT BUTTONS ---
        float bX = x + 10.0f, bY = y + 8.0f;
        drawTransportControls(kernel, bX, bY, isPlaying);

        // --- 3. THE LCD DISPLAY (Logic Pro High-Precision LCD) ---
        float lcdW = 480.0f, lcdX = x + (w - lcdW) * 0.5f, lcdY = y + 5.0f, lcdH = h - 10.0f;
        kernel.drawDropShadow(lcdX, lcdY, lcdW, lcdH, 4.0f, 0xAA000000);
        kernel.drawRoundedRect(lcdX, lcdY, lcdW, lcdH, 6.0f, 0xFF0A0A0C); // Ebony Glass
        kernel.drawGradientRect(lcdX, lcdY, lcdW, 20.0f, 0x11FFFFFF, 0x00000000); // Top Shine
        
        // --- CALC POSITION (Logic Standard: Bar.Beat.Div.Tick) ---
        const double safeRate = (std::isfinite(sampleRate) && sampleRate > 0.0) ? sampleRate : 44100.0;
        const double safeBpm = (std::isfinite(bpm) && bpm > 0.0f) ? bpm : 120.0;
        const double beatTotal = (static_cast<double>(currentPos) / safeRate) * (safeBpm / 60.0);
        // 960 PPQN: derive every field from one integer tick position so
        // floating-point fmod rounding cannot make division/tick disagree.
        const uint64_t totalTicks = static_cast<uint64_t>(std::max(0.0, std::floor(beatTotal * 960.0)));
        constexpr uint64_t ticksPerBeat = 960;
        constexpr uint64_t ticksPerDivision = 240;
        constexpr uint64_t ticksPerBar = ticksPerBeat * 4;
        const uint64_t barTicks = totalTicks % ticksPerBar;
        uint32_t bar = static_cast<uint32_t>(totalTicks / ticksPerBar) + 1;
        uint32_t beat = static_cast<uint32_t>(barTicks / ticksPerBeat) + 1;
        uint32_t div = static_cast<uint32_t>((barTicks % ticksPerBeat) / ticksPerDivision) + 1;
        uint32_t tick = static_cast<uint32_t>(barTicks % ticksPerDivision);
        
        // --- CALC SMPTE (HH:MM:SS.ms) ---
        double totalSeconds = (double)currentPos / sampleRate;
        uint32_t hours = (uint32_t)(totalSeconds / 3600);
        uint32_t mins = (uint32_t)(std::fmod(totalSeconds, 3600)) / 60;
        uint32_t secs = (uint32_t)(std::fmod(totalSeconds, 60));
        uint32_t ms = (uint32_t)(std::fmod(totalSeconds * 1000.0, 1000.0));

        // --- 4. LCD CONTENT (Grid Layout) ---
        // --- POSITION GLOW (Neon Cyan) ---
        float colX = lcdX + 25.0f;
        char posStr[64]; snprintf(posStr, 64, "%03d:%01d:%01d:%03d", bar, beat, div, tick);
        kernel.drawNeonRect(colX, lcdY + 12, 160, 20, 2, 8.0f, 0x3322D3EE); // Subtle Text Glow
        kernel.drawText(posStr, colX, lcdY + 28, 24, 0xFF22D3EE); 
        kernel.drawText("POSITION", colX, lcdY + 41, 7, 0xFF555555);

        // Dividers
        kernel.drawLine(colX + 165, lcdY + 12, colX + 165, lcdY + lcdH - 12, 1.0f, 0x22FFFFFF);

        colX += 185.0f;
        char smpteStr[64]; snprintf(smpteStr, 64, "%02d:%02d:%02d.%02d", hours, mins, secs, ms/10);
        kernel.drawText(smpteStr, colX, lcdY + 28, 20, 0xFFD1D1D1); 
        kernel.drawText("SMPTE", colX, lcdY + 41, 7, 0xFF555555);
        
        kernel.drawLine(colX + 130, lcdY + 12, colX + 130, lcdY + lcdH - 12, 1.0f, 0x22FFFFFF);

        // --- 5. LOGIC SIGNATURE AREA (Right Column) ---
        colX += 145.0f;
        char bpmStr[32]; snprintf(bpmStr, 32, "%.1f", bpm);
        kernel.drawText(bpmStr, colX, lcdY + 22, 14, 0xFF22D3EE);
        kernel.drawText("BPM", colX + kernel.measureText(bpmStr, 14) + 4, lcdY + 22, 7, 0xFF555555);
        
        kernel.drawText("4/4", colX, lcdY + 36, 12, 0xFFFFFFFF);
        kernel.drawText("/ Cmaj", colX + 35, lcdY + 36, 9, 0xFF888888);

        // --- 6. RESOURCE & MIDI ACTIVITY ---
        float resX = lcdX + lcdW - 75.0f;
        drawResourceBar(kernel, resX, lcdY + 14, 40, cpuLoad);
        kernel.drawText("CPU", resX + 43, lcdY + 14, 6, 0xFF555555);
        
        drawResourceBar(kernel, resX, lcdY + 30, 40, 0.05f);
        kernel.drawText("HD", resX + 43, lcdY + 30, 6, 0xFF555555);

        if (midiActive) {
            kernel.drawNeonRect(resX + 5, lcdY + 43, 6, 6, 3, 4.0f, 0xFFFDE047);
            kernel.drawCircle(resX + 8, lcdY + 46, 3, 0xFFFDE047); 
        }
        kernel.drawText("MIDI", resX + 18, lcdY + 46, 7, 0xFF555555);
    }

private:
    void drawTransportControls(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, bool isPlaying) {
        float bW = 40, bH = 30;
        
        // --- 1. REWIND / STOP (Logic style grey-interactive) ---
        uint32_t stopCol = isPlaying ? 0xFFBBBBBB : 0xFFFFFFFF;
        kernel.drawGradientRect(x, y, bW, bH, 0xFF4A4A4C, 0xFF252526);
        kernel.drawRoundedRect(x + 13, y + 8, 14, 14, 1.0f, stopCol); // Square Stop
        if (!isPlaying) kernel.drawNeonRect(x + 13, y + 8, 14, 14, 1.0f, 4.0f, 0x88FFFFFF); // REAL GLOW
        
        // --- 2. PLAY (Neon Logic Green) ---
        x += 42;
        uint32_t playCol = isPlaying ? 0xFF22C55E : 0xFF353538;
        kernel.drawGradientRect(x, y, bW, bH, 0xFF4A4A4C, 0xFF252526);
        
        // Lucide-style Triangle with Glow
        if (isPlaying) {
            kernel.drawNeonRect(x + 12, y + 5, 20, 20, 10.0f, 8.0f, 0xFF22C55E); // REAL NEON GLOW
            kernel.drawFilledTriangle(x + 15, y + 8, x + 15, y + 22, x + 28, y + 15, 0xFFFFFFFF);
        } else {
            kernel.drawFilledTriangle(x + 15, y + 8, x + 15, y + 22, x + 28, y + 15, 0xFF888888);
        }
        
        // --- 4. PANIC BUTTON (Safety Logic) ---
        x += 42;
        kernel.drawGradientRect(x, y, 32, bH, 0xFF4A4A4C, 0xFF252526);
        kernel.drawText("!", x + 12, y + 20, 14, 0xFFEF4444); // Red Warning
        kernel.drawRoundedRect(x, y, 32, bH, 2, 0x33FF0000); // Subtle red border
    }

private:
    void drawResourceBar(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float level) {
        float h = 8.0f;
        kernel.drawRoundedRect(x, y, w, h, 1.0f, 0xFF1A1A1C); // Background
        
        uint32_t col = (level > 0.8f) ? 0xFFEF4444 : (level > 0.6f) ? 0xFFFDE047 : 0xFF22D3EE;
        kernel.drawGradientRect(x, y, w * level, h, col, col & 0xAAFFFFFF);
        
        // Add segment lines
        for (int i = 1; i < 6; ++i) {
            float sx = x + (w / 6.0f) * i;
            kernel.drawLine(sx, y, sx, y + h, 1.0f, 0x33000000);
        }
    }
public:
    enum class Action { None, Stop, Play, Record, Panic, ToggleBPM, ToggleKey };

    Action hitTest(float mx, float my, float x, float y, float w, float h) {
        float bx = x + 10.0f, by = y + 8.0f;
        if (mx >= bx && mx < bx + 40 && my >= by && my < by + 30) return Action::Stop;
        bx += 42.0f;
        if (mx >= bx && mx < bx + 40 && my >= by && my < by + 30) return Action::Play;
        bx += 42.0f;
        if (mx >= bx && mx < bx + 40 && my >= by && my < by + 30) return Action::Record;
        bx += 42.0f;
        if (mx >= bx && mx < bx + 32 && my >= by && my < by + 30) return Action::Panic;

        return Action::None;
    }

};

} // namespace Aura::Graphics::UI
