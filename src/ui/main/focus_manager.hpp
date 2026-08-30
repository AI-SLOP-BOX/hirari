#pragma once
#include <string>
#include <array>
#include <functional>
#include <cstdint>
#include <mutex>

namespace Aura::UI::Main {

namespace KeyCodes {
    static constexpr int Space = 32;
    static constexpr int Enter = 13;
    static constexpr int Escape = 27;
    static constexpr int Delete = 127;

    // Normalized key values used by the UI. Platform adapters should convert
    // native virtual-key codes before they reach FocusManager.
    enum class Named : int { Space = Space, Enter = Enter, Escape = Escape, Delete = Delete };

    constexpr int normalize(int nativeKey) noexcept {
        return nativeKey;
    }
}

/**
 * @class FocusManager
 * @brief Professional Context-Sensitive Input Dispatcher.
 * HONEST FIX: Replaced map-based handlers with a fixed-size array for O(1) dispatch.
 */
class FocusManager {
public:
    enum class EditorType : uint32_t {
        None = 0,
        Arrangement,
        PianoRoll,
        Mixer,
        Inspector,
        Library,
        MAX_EDITORS
    };

    /**
     * @interface IEventHandler
     * @brief Interface for context-sensitive keyboard handling.
     */
    class IEventHandler {
    public:
        virtual ~IEventHandler() = default;
        virtual bool handleKey(int key, bool pressed) = 0;
    };

    static FocusManager& getInstance() {
        static FocusManager instance;
        return instance;
    }

    void registerHandler(EditorType type, IEventHandler* handler) {
        std::lock_guard<std::mutex> lock(m_mutex);
        uint32_t idx = static_cast<uint32_t>(type);
        if (idx < static_cast<uint32_t>(EditorType::MAX_EDITORS)) {
            m_handlers[idx] = handler;
        }
    }

    void unregisterHandler(EditorType type, IEventHandler* handler) noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        const uint32_t idx = static_cast<uint32_t>(type);
        if (idx < static_cast<uint32_t>(EditorType::MAX_EDITORS) && m_handlers[idx] == handler) {
            m_handlers[idx] = nullptr;
            if (m_currentFocus == type) m_currentFocus = EditorType::None;
        }
    }

    void setFocus(EditorType type) {
        std::lock_guard<std::mutex> lock(m_mutex);
        const uint32_t idx = static_cast<uint32_t>(type);
        if (idx < static_cast<uint32_t>(EditorType::MAX_EDITORS)) {
            m_currentFocus = type;
        }
    }

    EditorType getFocus() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_currentFocus;
    }

    /**
     * @brief Dispatches keys to the active editor or global handlers.
     */
    void handleKeyPress(int key) {
        handleKeyEvent(key, true);
    }

    void handleKeyRelease(int key) {
        handleKeyEvent(key, false);
    }

    void handleKeyEvent(int key, bool pressed) {
        key = KeyCodes::normalize(key);
        IEventHandler* handler = nullptr;
        std::function<void()> transport;
        EditorType focus = EditorType::None;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            if (key >= 0 && key < static_cast<int>(m_keyStates.size())) {
                const bool wasDown = m_keyStates[static_cast<size_t>(key)];
                m_keyStates[static_cast<size_t>(key)] = pressed;
                // Auto-repeat must not retrigger global transport actions.
                if (pressed && wasDown) return;
            }
        // 1. Global Priority Shortcuts
            if (pressed && key == KeyCodes::Space) transport = m_transportToggle;
            focus = m_currentFocus;

            // 2. Context-Sensitive Dispatch. The callback is invoked after
            // releasing the mutex so handlers may change focus safely.
            const uint32_t idx = static_cast<uint32_t>(focus);
            if (key != KeyCodes::Space && idx < static_cast<uint32_t>(EditorType::MAX_EDITORS)) {
                handler = m_handlers[idx];
            }
        }
        if (transport) { transport(); return; }
        if (handler) (void)handler->handleKey(key, pressed);
    }

    bool isKeyDown(int key) const noexcept {
        if (key < 0 || key >= static_cast<int>(m_keyStates.size())) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_keyStates[static_cast<size_t>(key)];
    }

    void clearKeyState() noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_keyStates.fill(false);
    }

    void setTransportToggleHandler(std::function<void()> handler) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_transportToggle = std::move(handler);
    }

private:
    FocusManager() { m_handlers.fill(nullptr); }

    EditorType m_currentFocus = EditorType::Arrangement;
    std::array<IEventHandler*, static_cast<uint32_t>(EditorType::MAX_EDITORS)> m_handlers;
    std::array<bool, 512> m_keyStates{};
    std::function<void()> m_transportToggle;
    mutable std::mutex m_mutex;
};

} // namespace Aura::UI::Main
