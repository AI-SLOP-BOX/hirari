#pragma once

#include <cstdint>
#include <functional>
#include <mutex>
#include <utility>

#include "../rust_ffi.hpp"

namespace Hirari::Core::Plugins {

// Platform UI code binds these callbacks at startup; the (track, plug-in)
// session table and its key packing live in Rust (`native_editor_sessions.rs`).
// The audio core only owns lifecycle and never touches a window on the RT thread.
class NativeEditorHost {
public:
    using OpenCallback = std::function<uint64_t(uint32_t, uint32_t, uintptr_t)>;
    using CloseCallback = std::function<bool(uint64_t)>;

    NativeEditorHost() : m_sessions(hirari_native_editor_sessions_create()) {}
    ~NativeEditorHost() { hirari_native_editor_sessions_destroy(m_sessions); }
    NativeEditorHost(const NativeEditorHost&) = delete;
    NativeEditorHost& operator=(const NativeEditorHost&) = delete;

    void bind(OpenCallback open, CloseCallback close) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_open = std::move(open);
        m_close = std::move(close);
    }

    void clear() {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_open = {};
        m_close = {};
        hirari_native_editor_sessions_clear(m_sessions);
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
        hirari_native_editor_sessions_adopt(m_sessions, track, plugin, handle);
        session = handle;
        return true;
    }

    bool close(uint32_t track, uint32_t plugin) {
        const uint64_t handle = hirari_native_editor_sessions_session(m_sessions, track, plugin);
        if (handle == 0) return false;
        CloseCallback closeCallback;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            closeCallback = m_close;
        }
        const bool ok = !closeCallback || closeCallback(handle);
        if (ok) hirari_native_editor_sessions_forget(m_sessions, track, plugin);
        return ok;
    }

    // Records a session created directly by a native plugin runtime. This is
    // used when the host has no wrapper callback but the processor can attach
    // its own VST3/AU view to the supplied parent surface.
    void adopt(uint32_t track, uint32_t plugin, uint64_t session) {
        hirari_native_editor_sessions_adopt(m_sessions, track, plugin, session);
    }

    bool forget(uint32_t track, uint32_t plugin) {
        return hirari_native_editor_sessions_forget(m_sessions, track, plugin);
    }

    bool embedded(uint32_t track, uint32_t plugin) const {
        return hirari_native_editor_sessions_embedded(m_sessions, track, plugin);
    }

    uint64_t session(uint32_t track, uint32_t plugin) const noexcept {
        return hirari_native_editor_sessions_session(m_sessions, track, plugin);
    }

private:
    mutable std::mutex m_mutex;
    OpenCallback m_open;
    CloseCallback m_close;
    void* m_sessions = nullptr;
};

} // namespace Hirari::Core::Plugins
