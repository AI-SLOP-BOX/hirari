#pragma once

#include <cstdint>
#include <functional>
#include <mutex>
#include <unordered_map>

namespace Aura::Core::Plugins {

// Platform UI code (Cocoa/Win32/Wayland/Qt/etc.) binds these callbacks at
// application startup.  The audio core only owns lifecycle and never touches
// a native window from the realtime thread.
class NativeEditorHost {
public:
    using OpenCallback = std::function<uint64_t(uint32_t, uint32_t, uintptr_t)>;
    using CloseCallback = std::function<bool(uint64_t)>;

    void bind(OpenCallback open, CloseCallback close) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_open = std::move(open);
        m_close = std::move(close);
    }

    void clear() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_open = {};
        m_close = {};
        m_sessions.clear();
    }

    bool open(uint32_t track, uint32_t plugin, uintptr_t parent, uint64_t& session) {
        OpenCallback openCallback;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            openCallback = m_open;
        }
        if (!openCallback) return false;
        // Native UI creation can synchronously call back into the host (for
        // example while Cocoa/Win32 negotiates the editor bounds). Never hold
        // the session mutex across that platform callback.
        const uint64_t handle = openCallback(track, plugin, parent);
        if (handle == 0) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_sessions[key(track, plugin)] = handle;
        session = handle;
        return true;
    }

    bool close(uint32_t track, uint32_t plugin) {
        uint64_t handle = 0;
        CloseCallback closeCallback;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            const auto it = m_sessions.find(key(track, plugin));
            if (it == m_sessions.end()) return false;
            handle = it->second;
            closeCallback = m_close;
        }
        const bool ok = !closeCallback || closeCallback(handle);
        if (ok) {
            std::lock_guard<std::mutex> lock(m_mutex);
            m_sessions.erase(key(track, plugin));
        }
        return ok;
    }

    // Records a session created directly by a native plugin runtime. This is
    // used when the host has no wrapper callback but the processor can attach
    // its own VST3/AU view to the supplied parent surface.
    void adopt(uint32_t track, uint32_t plugin, uint64_t session) {
        if (session == 0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_sessions[key(track, plugin)] = session;
    }

    bool forget(uint32_t track, uint32_t plugin) {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_sessions.erase(key(track, plugin)) != 0;
    }

    bool embedded(uint32_t track, uint32_t plugin) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_sessions.find(key(track, plugin)) != m_sessions.end();
    }

    uint64_t session(uint32_t track, uint32_t plugin) const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_sessions.find(key(track, plugin));
        return it == m_sessions.end() ? 0 : it->second;
    }

private:
    static uint64_t key(uint32_t track, uint32_t plugin) noexcept {
        return (static_cast<uint64_t>(track) << 32u) | plugin;
    }
    mutable std::mutex m_mutex;
    OpenCallback m_open;
    CloseCallback m_close;
    std::unordered_map<uint64_t, uint64_t> m_sessions;
};

} // namespace Aura::Core::Plugins
