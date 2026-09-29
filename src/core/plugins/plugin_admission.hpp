#pragma once

#include <filesystem>
#include <string>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Plugins::PluginAdmission {

inline std::string formatForPath(const std::filesystem::path& path) {
    const std::string nativePath = path.string();
    switch (hirari_plugin_format_for_path(nativePath.c_str())) {
        case 1: return "VST3";
        case 2: return "AU";
        case 3: return "CLAP";
        default: return {};
    }
}

inline bool isSafeCandidate(const std::filesystem::path& path,
                            const std::string& expectedFormat = {}) {
    const std::string nativePath = path.string();
    return hirari_plugin_is_safe_candidate(nativePath.c_str(), expectedFormat.c_str());
}

} // namespace Hirari::Core::Plugins::PluginAdmission
