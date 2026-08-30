#pragma once
#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <fstream>
#include <filesystem>
#include <string>
#include <unordered_set>
#include <utility>
#include <vector>
#include "plugin_sandbox.hpp"
#include "plugin_admission.hpp"
#include "plugin_cache_manager.hpp"

namespace Aura::Core::Plugins {

/**
 * @class PluginScanner
 * @brief High-performance 3rd-party Plugin Discovery Engine.
 * HONEST FIX: Implements standard macOS paths for AudioUnits (/Library/Audio/Plug-Ins/Components).
 * Necessary for professional DAWs to integrate user-owned VST/AU instruments 
 * into the Aura signal path.
 */
class PluginScanner {
public:
    struct PluginInfo {
        std::string name;
        std::string path;
        // Keep the discovered ABI explicit. Callers must not infer AU/VST3/
        // CLAP from a display name or repeat extension parsing downstream.
        std::string format;
        std::string manufacturer;
        std::string version;
        uint64_t fingerprint = 0;
    };

    /**
     * @brief SCAN: Searches the filesystem for available plugins (AU, VST3, CLAP).
     */
    std::vector<PluginInfo> scanSystem() const {
        std::vector<PluginInfo> plugins;
        std::vector<std::pair<std::string, std::string>> searchPaths = {
            {"/Library/Audio/Plug-Ins/Components", "AU"},
            {"/Library/Audio/Plug-Ins/VST3", "VST3"},
            {"/Library/Audio/Plug-Ins/CLAP", "CLAP"},
            {"~/Library/Audio/Plug-Ins/Components", "AU"},
            {"~/Library/Audio/Plug-Ins/VST3", "VST3"},
            {"~/Library/Audio/Plug-Ins/CLAP", "CLAP"}
        };

        std::unordered_set<std::string> discovered;
        for (const auto& [p, format] : searchPaths) {
            const auto dir = expandUserPath(std::filesystem::path(p));
            std::error_code ec;
            if (!std::filesystem::is_directory(dir, ec) || ec) continue;

            std::filesystem::directory_iterator it(dir, ec);
            if (ec) continue;
            const std::filesystem::directory_iterator end;
            for (; it != end; it.increment(ec)) {
                if (ec) {
                    ec.clear();
                    continue;
                }
                const auto& entry = *it;
                const auto entryPath = entry.path();
                const bool usable = PluginAdmission::isSafeCandidate(entryPath, format);
                if (!usable) continue;

                const auto identity = normalizedPath(entryPath);
                if (identity.empty() || !discovered.insert(identity).second) continue;

                PluginInfo info;
                // Keep the scanner's public identity aligned with the cache
                // and admission layers. Returning a raw relative/alias path
                // here makes the same binary look different to downstream
                // cache invalidation and host registration.
                info.path = identity;
                info.format = format;
                info.name = readBundleValue(entryPath, "CFBundleDisplayName");
                if (info.name.empty()) info.name = readBundleValue(entryPath, "CFBundleName");
                if (info.name.empty()) info.name = entryPath.stem().string();
                info.manufacturer = readBundleValue(entryPath, "Manufacturer");
                if (info.manufacturer.empty()) info.manufacturer = readBundleValue(entryPath, "Vendor");
                if (info.manufacturer.empty()) {
                    info.manufacturer = vendorFromBundleIdentifier(
                        readBundleValue(entryPath, "CFBundleIdentifier"));
                }
                info.version = readBundleValue(entryPath, "CFBundleShortVersionString");
                if (info.version.empty()) info.version = readBundleValue(entryPath, "CFBundleVersion");
                const auto fingerprint = PluginCacheManager::fingerprintForPath(entryPath);
                if (!fingerprint || *fingerprint == 0) continue;
                info.fingerprint = *fingerprint;
                plugins.push_back(std::move(info));
            }
        }
        std::sort(plugins.begin(), plugins.end(), [](const PluginInfo& left, const PluginInfo& right) {
            return left.path < right.path;
        });
        return plugins;
    }

private:
    static std::filesystem::path expandUserPath(const std::filesystem::path& input) {
        const auto value = input.string();
        if (value != "~" && value.rfind("~/", 0) != 0) return input;

        const char* home = std::getenv("HOME");
        if (home == nullptr || *home == '\0') return input;
        if (value == "~") return std::filesystem::path(home);
        return std::filesystem::path(home) / value.substr(2);
    }

    static std::string normalizedPath(const std::filesystem::path& path) {
        std::error_code ec;
        auto normalized = std::filesystem::weakly_canonical(path, ec);
        if (ec) {
            ec.clear();
            normalized = std::filesystem::absolute(path, ec);
            if (ec) normalized = path.lexically_normal();
        }
        return normalized.lexically_normal().string();
    }

    static std::filesystem::path infoPlistPath(const std::filesystem::path& bundle) {
        std::error_code ec;
        if (!std::filesystem::is_directory(bundle, ec) || ec) return {};
        const auto plist = bundle / "Contents" / "Info.plist";
        if (std::filesystem::is_regular_file(plist, ec) && !ec) return plist;
        return {};
    }

    static std::string readBundleValue(const std::filesystem::path& bundle,
                                       const std::string& key) {
        const auto plist = infoPlistPath(bundle);
        if (plist.empty()) return {};

        std::ifstream stream(plist, std::ios::binary);
        if (!stream) return {};
        constexpr std::streamsize maxPlistBytes = 1024 * 1024;
        std::string text(static_cast<std::size_t>(maxPlistBytes), '\0');
        stream.read(text.data(), maxPlistBytes);
        text.resize(static_cast<std::size_t>(stream.gcount()));

        const auto keyToken = "<key>" + key + "</key>";
        const auto keyPos = text.find(keyToken);
        if (keyPos == std::string::npos) return {};
        const auto valueStart = text.find("<string>", keyPos + keyToken.size());
        if (valueStart == std::string::npos) return {};
        const auto contentStart = valueStart + 8;
        const auto contentEnd = text.find("</string>", contentStart);
        if (contentEnd == std::string::npos) return {};

        auto value = text.substr(contentStart, contentEnd - contentStart);
        const auto first = value.find_first_not_of(" \t\r\n");
        const auto last = value.find_last_not_of(" \t\r\n");
        if (first == std::string::npos) return {};
        return value.substr(first, last - first + 1);
    }

    static std::string vendorFromBundleIdentifier(const std::string& identifier) {
        if (identifier.empty()) return {};
        std::vector<std::string> parts;
        std::size_t begin = 0;
        while (begin < identifier.size()) {
            const auto end = identifier.find('.', begin);
            const auto length = end == std::string::npos ? identifier.size() - begin : end - begin;
            if (length != 0) parts.emplace_back(identifier.substr(begin, length));
            if (end == std::string::npos) break;
            begin = end + 1;
        }
        if (parts.size() >= 2 && (parts[0] == "com" || parts[0] == "org" || parts[0] == "net")) {
            return parts[1];
        }
        return parts.size() > 1 ? parts.front() : identifier;
    }
};

} // namespace Aura::Core::Plugins
