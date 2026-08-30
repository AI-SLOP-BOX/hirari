#pragma once
#include "ui_view.hpp"
#include <string>
#include <vector>
#include <functional>

namespace Aura::Graphics::UI {

/**
 * @class PopupMenu
 * @brief Logic Pro 11 style Contextual Popup Menu.
 */
class PopupMenu : public View {
public:
    struct Item {
        std::string label;
        std::function<void()> callback;
        bool isSeparator = false;
    };

    PopupMenu() { m_visible = false; }

    void show(float x, float y, const std::vector<Item>& items) {
        m_items = items;
        // Logic Style: Menu width 220, height dynamic
        float h = 10;
        for (const auto& i : m_items) h += i.isSeparator ? 4 : 24;
        m_bounds = {x, y, 220, h};
        m_visible = true;
    }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel) override {
        if (!m_visible) return;
        auto b = m_bounds;

        // --- 1. PREMIUM GLASS MENU BACKDROP ---
        kernel.applyBlurEffect(b.x, b.y, b.w, b.h, 15.0f);
        kernel.drawRoundedRect(b.x, b.y, b.w, b.h, 6.0f, 0xEE1A1A1C);
        kernel.drawRoundedRect(b.x, b.y, b.w, b.h, 6.0f, 0x33FFFFFF); // Fine ridge

        float curY = b.y + 5;
        for (size_t i = 0; i < m_items.size(); ++i) {
            auto& item = m_items[i];
            if (item.isSeparator) {
                kernel.drawLine(b.x + 5, curY + 2, b.x + b.w - 5, curY + 2, 0.5f, 0x22FFFFFF);
                curY += 4;
            } else {
                // Hover effect (simulated)
                bool hover = m_hoverIdx >= 0 && i == static_cast<size_t>(m_hoverIdx);
                if (hover) {
                    kernel.drawRoundedRect(b.x + 4, curY, b.w - 8, 22, 4.0f, 0xFF3B82F6);
                }
                kernel.drawText(item.label, b.x + 12, curY + 16, 9, 0xFFD1D5DB);
                curY += 24;
            }
        }
    }

    bool onMouseDown(float x, float y) override {
        if (!m_visible) return false;
        if (!m_bounds.contains(x, y)) {
            m_visible = false;
            return true;
        }
        float curY = m_bounds.y + 5;
        for (size_t i = 0; i < m_items.size(); ++i) {
            auto& item = m_items[i];
            if (item.isSeparator) {
                curY += 4;
            } else {
                if (x >= m_bounds.x && x <= m_bounds.x + m_bounds.w && y >= curY && y < curY + 24) {
                    m_visible = false;
                    if (item.callback) item.callback();
                    return true;
                }
                curY += 24;
            }
        }
        m_visible = false; // Clicked menu padding, not an item.
        return true;
    }

private:
    std::vector<Item> m_items;
    int m_hoverIdx = -1;
};

} // namespace Aura::Graphics::UI
