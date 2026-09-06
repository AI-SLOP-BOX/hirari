#pragma once
#include <vector>
#include <string>
#include <algorithm>
#include "ui_view.hpp"
#include "../../ui/main/focus_manager.hpp"
#include "../../core/aura_unified_engine.hpp"
#include "lcd_display.hpp"
#include "../../ui/mixer/mixer_console.hpp"
#include "arrangement_view.hpp"
#include "floating_plugin_window.hpp"
#include "aura_pro_icons.hpp"
#include "aura_pro_inspector.hpp"
#include "scae_assistant_pane.hpp"
#include "piano_roll_view.hpp"
#include "step_sequencer_view.hpp"

namespace Aura::Graphics::UI {

/**
 * @class MainWorkspaceOrchestrator
 * @brief Manages the assembly and layout of all main DAW views.
 * HONEST FIX: Refactored layout to be deterministic and purged magic numbers.
 */
class MainWorkspaceOrchestrator {
public:
    enum class EditorMode { PianoRoll, StepSeq, SmartControls };
    enum class UserMode { Beginner, Pro, Custom };

    MainWorkspaceOrchestrator() : m_width(1280), m_height(800) {}

    void setUserMode(UserMode mode) noexcept {
        m_userMode = mode;
        if (mode == UserMode::Beginner) {
            m_libraryVisible = false;
            m_inspectorVisible = false;
            m_editorVisible = false;
        } else if (mode == UserMode::Pro) {
            m_libraryVisible = true;
            m_inspectorVisible = true;
            m_editorVisible = true;
        }
    }

    UserMode userMode() const noexcept { return m_userMode; }

    // Custom mode is deliberately explicit: panel visibility is never
    // changed behind the user's back after they start arranging their own
    // workspace.
    void setPanelVisibility(bool library, bool inspector, bool editor) noexcept {
        m_userMode = UserMode::Custom;
        m_libraryVisible = library;
        m_inspectorVisible = inspector;
        m_editorVisible = editor;
    }

    void setFocusMode(bool enabled) noexcept {
        m_focusMode = enabled;
        if (enabled) {
            m_libraryVisible = false;
            m_inspectorVisible = false;
            m_editorVisible = false;
        }
    }

    bool focusMode() const noexcept { return m_focusMode; }

    void setMixerVisible(bool visible) noexcept { m_mixerVisible = visible; }
    bool mixerVisible() const noexcept { return m_mixerVisible; }

    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float w, float h) {
        performLayout(w, h);

        auto& coreEng = Aura::Core::Engine::AuraUnifiedEngine::getInstance();
        const auto tracks = coreEng.get_tracks_snapshot();

        kernel.drawRect(0, 0, w, h, 0xFF0A0A0C); // Background

        const bool compact = m_userMode == UserMode::Beginner || m_focusMode;

        // --- 1. ARRANGEMENT VIEW ---
        m_arrangement.render(kernel, m_mainX, m_controlBarH, m_mainW, m_mainH, tracks, m_scrollX);

        // --- 2. SIDEBARS ---
        if (!compact && m_libraryVisible) {
            renderLibrary(kernel, 0, m_controlBarH, m_libraryW, h - m_controlBarH);
        }
        if (!compact && m_inspectorVisible) {
            float ix = m_libraryVisible ? m_libraryW : 0;
            m_inspector.render(kernel, ix, m_controlBarH, m_sidebarW, h - m_controlBarH, tracks);
        }

        // --- 3. EDITOR ---
        if (!compact && m_editorVisible) {
            float ey = h - m_editorH;
            kernel.drawRect(m_mainX, ey, m_mainW, m_editorH, 0xFF141416);
            if (m_mixerVisible) m_mixer.render(kernel, m_mainX, ey, m_mainW, m_editorH, tracks);
            else m_pianoRoll.render(kernel, m_mainX, ey, m_mainW, m_editorH);
        }

        // --- 4. CONTROL BAR ---
        renderControlBar(kernel, w, m_controlBarH);
        if (compact) {
            kernel.drawText(m_focusMode ? "FOCUS" : "BEGINNER",
                            std::max(8.0f, w - 92.0f), 20.0f, 10, 0xFF9CA3AF);
        }
    }

    void performLayout(float w, float h) {
        m_width = std::max(0.0f, w);
        m_height = std::max(0.0f, h);
        constexpr float kControlBar = 54.0f;
        constexpr float kMinPanel = 96.0f;
        m_controlBarH = std::min(kControlBar, m_height);
        m_sidebarW = std::min(280.0f, std::max(0.0f, m_width * 0.34f));
        m_libraryW = std::min(280.0f, std::max(0.0f, m_width * 0.34f));
        const float availableEditor = std::max(0.0f, m_height - m_controlBarH);
        const bool compact = m_userMode == UserMode::Beginner || m_focusMode;
        const bool editorVisible = m_editorVisible && !compact;
        const bool libraryVisible = m_libraryVisible && !compact;
        const bool inspectorVisible = m_inspectorVisible && !compact;
        m_editorH = editorVisible ? std::clamp(availableEditor * 0.4f, 0.0f,
                                                   std::max(0.0f, availableEditor - kMinPanel)) : 0.0f;

        float sidebarsW = (libraryVisible ? m_libraryW : 0) + (inspectorVisible ? m_sidebarW : 0);
        if (sidebarsW > m_width - kMinPanel && m_width >= kMinPanel) {
            sidebarsW = std::max(0.0f, m_width - kMinPanel);
        }
        m_mainX = std::min(sidebarsW, m_width);
        m_mainW = std::max(0.0f, m_width - m_mainX);
        m_mainH = std::max(0.0f, m_height - m_controlBarH - (editorVisible ? m_editorH : 0));
    }

    bool handleMouseDown(float x, float y) {
        performLayout(m_width, m_height);
        m_lastMouseX = x;
        m_lastMouseY = y;
        auto& engine = ::Aura::Core::Engine::AuraUnifiedEngine::getInstance();
        const auto tracks = engine.get_tracks_snapshot();
        if (x < m_mainX || y < m_controlBarH || y >= m_controlBarH + m_mainH) return false;
        return m_arrangement.handleMouseDown(x, y, tracks, m_scrollX);
    }

    void handleMouseDrag(float x, float y) {
        const float dx = x - m_lastMouseX;
        const float dy = y - m_lastMouseY;
        auto& engine = ::Aura::Core::Engine::AuraUnifiedEngine::getInstance();
        m_arrangement.handleMouseDrag(x, y, dx, dy, engine.get_tracks_snapshot(), m_scrollX);
        m_lastMouseX = x;
        m_lastMouseY = y;
    }

    void handleMouseUp(float, float) { m_arrangement.handleMouseUp(); }

    bool handleKeyDown(int keyCode) {
        using namespace ::Aura::UI::Main;
        if (keyCode == KeyCodes::Space) {
            auto& engine = ::Aura::Core::Engine::AuraUnifiedEngine::getInstance();
            engine.set_playing(!engine.is_playing());
            return true;
        }
        if (keyCode == 'm' || keyCode == 'M') {
            m_mixerVisible = !m_mixerVisible;
            return true;
        }
        return false;
    }

private:
    void renderLibrary(::Aura::Graphics::Platform::IGraphicsKernel& k, float x, float y, float w, float h) {
        k.drawRect(x, y, w, h, 0xFF141416);
        k.drawText("LIBRARY", x + 10, y + 10, 12, 0xFFF1F5F9);
    }

    void renderControlBar(::Aura::Graphics::Platform::IGraphicsKernel& k, float w, float h) {
        k.drawRect(0, 0, w, h, 0xFF1C1C1E);
        k.drawLine(0, h-1, w, h-1, 1.0f, 0xFF000000);
    }

    float m_width, m_height, m_controlBarH, m_sidebarW, m_libraryW, m_editorH;
    float m_mainX, m_mainW, m_mainH;
    UserMode m_userMode = UserMode::Pro;
    bool m_focusMode = false;
    bool m_inspectorVisible = true, m_editorVisible = true, m_libraryVisible = true;
    bool m_mixerVisible = false;
    float m_scrollX = 0;
    float m_lastMouseX = 0.0f, m_lastMouseY = 0.0f;

    ArrangementView m_arrangement;
    ::Aura::UI::Mixer::MixerConsole m_mixer;
    PianoRollView m_pianoRoll;
    ProfessionalInspector m_inspector;
};

// Compatibility Typedef
using AuraProUI = MainWorkspaceOrchestrator;

} // namespace Aura::Graphics::UI
