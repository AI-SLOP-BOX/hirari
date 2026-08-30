#pragma once
#include "ui_view.hpp"
#include "../../scae/AuraAISuite.hpp"
#include <atomic>
#include <thread>
#include <cstring>

namespace Aura::Graphics::UI {

/**
 * @class StemSplitterUI
 * @brief Logic Pro 11 style Stem Splitter (Source Separation).
 * Separation into Vocals, Drums, Bass, and Other.
 */
class StemSplitterUI : public View {
public:
    StemSplitterUI() {
        m_visible = false;
    }

    ~StemSplitterUI() {
        m_cancel.store(true, std::memory_order_release);
        if (m_worker.joinable()) m_worker.join();
    }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel) override {
        if (!m_visible) return;
        if (m_completed.load(std::memory_order_acquire) && m_worker.joinable()) {
            // The worker has already published completion. Joining here is
            // non-blocking in practice and permits a later split operation.
            m_worker.join();
        }
        auto b = m_bounds;

        // --- 1. GLASS OVERLAY ---
        kernel.applyBlurEffect(b.x, b.y, b.w, b.h, 20.0f);
        kernel.drawRoundedRect(b.x + b.w*0.5f - 250, b.y + b.h*0.5f - 180, 500, 360, 16.0f, 0xEE111115);
        kernel.drawRoundedRect(b.x + b.w*0.5f - 250, b.y + b.h*0.5f - 180, 500, 360, 16.0f, 0x33FFFFFF);

        float cx = b.x + b.w * 0.5f;
        float cy = b.y + b.h * 0.5f;

        // --- 2. HEADER: STEM SPLITTER ---
        kernel.drawText("STEM SPLITTER", cx - 80, cy - 140, 18, 0xFFFFFFFF);
        kernel.drawText("Choose the stems you want to extract from the region.", cx - 140, cy - 115, 9, 0xFF9CA3AF);

        // --- 3. THE 4 STEMS (Icons) ---
        float iconSize = 80.0f;
        float spacing = 20.0f;
        float startX = cx - (iconSize * 2 + spacing * 1.5f);
        
        const char* labels[] = {"VOCALS", "DRUMS", "BASS", "OTHER"};
        uint32_t colors[] = {0xFF30B0FF, 0xFFEAB308, 0xFF34C759, 0xFFF87171};

        for (int i = 0; i < 4; ++i) {
            float ix = startX + i * (iconSize + spacing);
            float iy = cy - 40;
            
            bool selected = (m_selectedMask & (1 << i));
            kernel.drawRoundedRect(ix, iy, iconSize, iconSize, 8.0f, selected ? colors[i] : 0xFF1F2937);
            kernel.drawText(labels[i], ix + (iconSize - strlen(labels[i])*6)/2, iy + iconSize + 15, 8, selected ? 0xFFFFFFFF : 0xFF6B7280);
            
            // Checkmark if selected
            if (selected) {
                 kernel.drawCircle(ix + iconSize - 10, iy + 10, 6, 0xFFFFFFFF);
                 kernel.drawCircle(ix + iconSize - 10, iy + 10, 4, colors[i]);
            }
        }

        // --- 4. ACTION BUTTON ---
        float btnW = 180, btnH = 36;
        float btnX = cx - btnW*0.5f, btnY = cy + 100;
        
        if (m_completed.exchange(false, std::memory_order_acq_rel)) {
            m_isProcessing.store(false, std::memory_order_release);
            m_visible = false;
        }
        if (m_isProcessing.load(std::memory_order_acquire)) {
            kernel.drawRoundedRect(btnX, btnY, btnW, btnH, 6.0f, 0xFF1F2937);
            kernel.drawText("SPLITTING...", btnX + 55, btnY + 22, 10, 0xFFFFFFFF);
            kernel.drawRect(btnX, btnY + btnH - 2,
                            btnW * m_progress.load(std::memory_order_relaxed), 2, 0xFF30B0FF);
        } else {
            kernel.drawRoundedRect(btnX, btnY, btnW, btnH, 6.0f, 0xFF3B82F6);
            kernel.drawText(m_error.load(std::memory_order_acquire)
                                ? "UNAVAILABLE"
                                : "SPLIT STEMS",
                            btnX + 55, btnY + 22, 10, 0xFFFFFFFF);
        }
    }

    bool onMouseDown(float x, float y) override {
        auto b = m_bounds;
        float cx = b.x + b.w * 0.5f;
        float cy = b.y + b.h * 0.5f;
        
        // Handle Icon Selection
        float iconSize = 80.0f, spacing = 20.0f;
        float startX = cx - (iconSize * 2 + spacing * 1.5f);
        for (int i = 0; i < 4; ++i) {
            float ix = startX + i * (iconSize + spacing);
            float iy = cy - 40;
            if (x >= ix && x <= ix + iconSize && y >= iy && y <= iy + iconSize) {
                m_selectedMask ^= (1 << i);
                return true;
            }
        }

        // Handle Split Button
        float btnW = 180, btnH = 36;
        float btnX = cx - btnW*0.5f, btnY = cy + 100;
        if (x >= btnX && x <= btnX + btnW && y >= btnY && y <= btnY + btnH) {
            startSeparation();
            return true;
        }

        // Click outside panel to close
        if (x < b.x + b.w*0.5f - 250 || x > b.x + b.w*0.5f + 250 ||
            y < b.y + b.h*0.5f - 180 || y > b.y + b.h*0.5f + 180) {
            m_visible = false;
        }
        
        return true; 
    }

    void startSeparation() {
        if (m_worker.joinable()) return;
        // The actual separation pipeline requires a selected region/sample
        // buffer and a destination track. This view currently has neither;
        // never simulate progress or report completion without those inputs.
        m_error.store(true, std::memory_order_release);
        m_progress.store(0.0f, std::memory_order_relaxed);
    }

private:
    uint8_t m_selectedMask = 0x0F; // All selected by default
    std::atomic<bool> m_isProcessing{false};
    std::atomic<float> m_progress{0.0f};
    std::atomic<bool> m_completed{false};
    std::atomic<bool> m_cancel{false};
    std::atomic<bool> m_error{false};
    std::thread m_worker;
};

} // namespace Aura::Graphics::UI
