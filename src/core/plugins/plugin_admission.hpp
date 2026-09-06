#pragma once

#include <cctype>
#include <filesystem>
#include <string>

namespace Aura::Core::Plugins::PluginAdmission {

inline std::string formatForPath(const std::filesystem::path& path) {
    std::string extension = path.extension().string();
    for (char& character : extension) {
        character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
    }
    if (extension == ".vst3") return "VST3";
    if (extension == ".component") return "AU";
    if (extension == ".clap") return "CLAP";
    return {};
}

inline bool isSafeCandidate(const std::filesystem::path& path,
                            const std::string& expectedFormat = {}) {
    std::error_code ec;
    const auto status = std::filesystem::symlink_status(path, ec);
    if (ec || std::filesystem::is_symlink(status)) return false;
    if (!std::filesystem::is_directory(status) && !std::filesystem::is_regular_file(status)) {
        return false;
    }
    if (expectedFormat.empty()) return true;
    std::string normalizedExpected = expectedFormat;
    for (char& character : normalizedExpected) {
        character = static_cast<char>(std::toupper(static_cast<unsigned char>(character)));
    }
    return formatForPath(path) == normalizedExpected;
}

} // namespace Aura::Core::Plugins::PluginAdmission
