#pragma once

#include <cstddef>
#include <cstdint>
#include <string_view>

#include "../rust_ffi.hpp"

extern "C" bool hirari_plugin_scan_cache_should_scan(const uint8_t* path, size_t pathSize);
extern "C" bool hirari_plugin_scan_cache_record_failure(const uint8_t* path, size_t pathSize,
                                                        uint32_t reason);
extern "C" bool hirari_plugin_scan_cache_record_success(const uint8_t* path, size_t pathSize);

namespace Hirari::Core::Plugins {

/**
 * Path-based view of the Rust-owned plug-in scan cache.
 *
 * The canonical cache key, content fingerprint, blacklist record and its
 * persistence all live in Rust (plugin_blacklist.rs). Callers hand over the
 * on-disk path only; the admission rule ("scan unless a recorded failure still
 * matches the current fingerprint") is applied there.
 */
namespace PluginScanCache {

inline bool shouldScan(std::string_view path) {
    return hirari_plugin_scan_cache_should_scan(
        reinterpret_cast<const uint8_t*>(path.data()), path.size());
}

inline void recordScanFailure(std::string_view path, uint32_t reason = 1) {
    (void)hirari_plugin_scan_cache_record_failure(
        reinterpret_cast<const uint8_t*>(path.data()), path.size(), reason);
}

inline void recordScanSuccess(std::string_view path) {
    (void)hirari_plugin_scan_cache_record_success(
        reinterpret_cast<const uint8_t*>(path.data()), path.size());
}

} // namespace PluginScanCache

} // namespace Hirari::Core::Plugins
