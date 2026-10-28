#pragma once

#include <cstdint>
#include <filesystem>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include "../HirariPluginSDK.hpp"
#include "../rust_ffi.hpp"

namespace Hirari::Core::Plugins {

/** Thin path and blacklist adapter for the Rust-owned plug-in scan state. */
class PluginCacheManager {
public:
    static PluginCacheManager& getInstance() {
        static PluginCacheManager instance;
        return instance;
    }

    static std::filesystem::path getCachePath() {
        const char* path = hirari_plugin_cache_path();
        if (!path) throw std::runtime_error("failed to prepare the plug-in cache directory");
        return path;
    }

    static std::string cacheKey(const std::string& pluginPath) {
        size_t size = 0;
        const auto* input = reinterpret_cast<const uint8_t*>(pluginPath.data());
        if (!hirari_plugin_cache_key(input, pluginPath.size(), nullptr, 0, &size))
            return {};
        std::string key(size, '\0');
        size_t written = 0;
        if (!hirari_plugin_cache_key(input, pluginPath.size(),
                                     reinterpret_cast<uint8_t*>(key.data()),
                                     key.size(), &written) || written != size) {
            return {};
        }
        return key;
    }

    static std::string stateCacheKey(const Hirari::SDK::PluginDescriptor& descriptor) {
        return descriptor.isValid() ? descriptor.stateCacheKey() : std::string{};
    }

    static std::optional<uint64_t> fingerprintForPath(const std::filesystem::path& path) {
        const std::string pathText = path.generic_string();
        uint64_t fingerprint = 0;
        return hirari_plugin_fingerprint(pathText.data(), pathText.size(), &fingerprint)
            ? std::optional<uint64_t>(fingerprint)
            : std::nullopt;
    }

    bool isBlacklisted(const std::string& pluginPath) const {
        const std::string key = cacheKey(pluginPath);
        const auto fingerprint = fingerprintForPath(key);
        return hirari_plugin_blacklist_is_blocked(
            m_blacklistState, key.data(), key.size(), fingerprint.value_or(0),
            fingerprint.has_value());
    }

    uint32_t blacklistReason(const std::string& pluginPath) const {
        const std::string key = cacheKey(pluginPath);
        return hirari_plugin_blacklist_reason(m_blacklistState, key.data(), key.size());
    }

    std::vector<std::pair<std::string, uint32_t>> blacklistSnapshot() const {
        HirariPluginBlacklistRecord* records = nullptr;
        size_t count = 0;
        if (!hirari_plugin_blacklist_snapshot(m_blacklistState, &records, &count)) return {};
        struct SnapshotGuard {
            HirariPluginBlacklistRecord* records;
            size_t count;
            ~SnapshotGuard() { hirari_plugin_blacklist_snapshot_free(records, count); }
        } guard{records, count};

        std::vector<std::pair<std::string, uint32_t>> result;
        result.reserve(count);
        for (size_t index = 0; index < count; ++index) {
            const auto& record = records[index];
            result.emplace_back(
                std::string(reinterpret_cast<const char*>(record.path), record.path_size),
                record.reason);
        }
        return result;
    }

    bool shouldScan(const std::string& pluginPath) const {
        return !pluginPath.empty() && !isBlacklisted(pluginPath);
    }

    void recordScanFailure(const std::string& pluginPath, uint32_t reasonCode = 1) {
        if (pluginPath.empty()) return;
        const std::string key = cacheKey(pluginPath);
        const auto fingerprint = fingerprintForPath(pluginPath);
        (void)hirari_plugin_blacklist_set(
            m_blacklistState, key.data(), key.size(), reasonCode,
            fingerprint.value_or(0), fingerprint.has_value());
    }

    void clearScanFailure(const std::string& pluginPath) {
        const std::string key = cacheKey(pluginPath);
        (void)hirari_plugin_blacklist_remove(m_blacklistState, key.data(), key.size());
    }

    void recordScanSuccess(const std::string& pluginPath) {
        if (!pluginPath.empty()) clearScanFailure(pluginPath);
    }

private:
    PluginCacheManager() {
        const std::string path = getCachePath().string() + ".blacklist";
        m_blacklistState = hirari_plugin_blacklist_create(
            reinterpret_cast<const uint8_t*>(path.data()), path.size());
    }
    ~PluginCacheManager() { hirari_plugin_blacklist_destroy(m_blacklistState); }
    PluginCacheManager(const PluginCacheManager&) = delete;
    PluginCacheManager& operator=(const PluginCacheManager&) = delete;

    void* m_blacklistState = nullptr;
};

} // namespace Hirari::Core::Plugins
