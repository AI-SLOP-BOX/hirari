#pragma once

#include <cstdint>
#include <string>

#include "../rust_ffi.hpp"

namespace Hirari::Core::Plugins {

/** C++ engine adapter for Rust-owned third-party plug-in compatibility state. */
class PluginCompatibilityRegistry final {
public:
    PluginCompatibilityRegistry() : m_state(hirari_plugin_registry_create()) {}
    ~PluginCompatibilityRegistry() { hirari_plugin_registry_destroy(m_state); }

    PluginCompatibilityRegistry(const PluginCompatibilityRegistry&) = delete;
    PluginCompatibilityRegistry& operator=(const PluginCompatibilityRegistry&) = delete;

    void markScanFailure(const std::string& id, const std::string& error) {
        (void)hirari_plugin_registry_record_scan_failure(
            m_state, reinterpret_cast<const uint8_t*>(id.data()), id.size(),
            reinterpret_cast<const uint8_t*>(error.data()), error.size());
    }

    void markCrash(const std::string& id) {
        (void)hirari_plugin_registry_record_crash(
            m_state, reinterpret_cast<const uint8_t*>(id.data()), id.size());
    }

    void setBlacklisted(const std::string& id, bool value) {
        (void)hirari_plugin_registry_set_blacklisted(
            m_state, reinterpret_cast<const uint8_t*>(id.data()), id.size(), value);
    }

    std::string snapshotJson() const {
        uint8_t* bytes = nullptr;
        size_t size = 0;
        if (!hirari_plugin_registry_snapshot_json(m_state, &bytes, &size)) return "[]";
        struct SnapshotGuard {
            uint8_t* bytes;
            size_t size;
            ~SnapshotGuard() { hirari_plugin_registry_snapshot_json_free(bytes, size); }
        } guard{bytes, size};
        return std::string(reinterpret_cast<const char*>(bytes), size);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Plugins
