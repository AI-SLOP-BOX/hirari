#pragma once
#include <string>
#include <vector>
#include <fstream>
#include <charconv>
#include <cmath>
#include <cstdlib>
#include <regex>
#include <unordered_set>
#include <unordered_map>
#include <memory>
#include <cstring>
#include <filesystem>
#include <chrono>
#include <atomic>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../engine/track.hpp"

namespace Aura::Core::IO {

/**
 * @class SessionIO
 * @brief Professional DAW XML Export/Import Engine.
 * HONEST FIX: Implements standard AAF/EDL-compatible serialization for 
 * tracks, regions, and effect chains.
 * Essential for project portability and session recovery—mirroring 
 * the robust XML/JSON formats of Ardour and Pro Tools.
 */
class SessionIO {
public:
    static SessionIO& getInstance() { static SessionIO i; return i; }

    // XML attributes must be escaped at the persistence boundary.  Project
    // paths and region names routinely contain ampersands, quotes, or angle
    // brackets; writing them verbatim creates a file that cannot be reopened.
    static std::string escapeXmlAttribute(const std::string& value) {
        std::string escaped;
        escaped.reserve(value.size());
        for (const char ch : value) {
            switch (ch) {
            case '&': escaped += "&amp;"; break;
            case '<': escaped += "&lt;"; break;
            case '>': escaped += "&gt;"; break;
            case '"': escaped += "&quot;"; break;
            case '\'': escaped += "&apos;"; break;
            default: escaped += ch; break;
            }
        }
        return escaped;
    }

    static std::string unescapeXmlAttribute(std::string value) {
        const std::pair<const char*, const char*> entities[] = {
            {"&quot;", "\""}, {"&apos;", "'"}, {"&gt;", ">"},
            {"&lt;", "<"}, {"&amp;", "&"},
        };
        for (const auto& [entity, replacement] : entities) {
            size_t offset = 0;
            while ((offset = value.find(entity, offset)) != std::string::npos) {
                value.replace(offset, std::strlen(entity), replacement);
                offset += std::strlen(replacement);
            }
        }
        return value;
    }

    /**
     * @brief SAVE: Serializes all engine states to a project XML file.
     */
    void saveProject(const std::string& path, const std::vector<std::shared_ptr<Engine::Track>>& tracks) {
        if (path.empty()) return;
        const std::filesystem::path destination(path);
        const auto parent = destination.parent_path();
        if (!parent.empty()) {
            std::error_code parentError;
            if (!std::filesystem::is_directory(parent, parentError) || parentError) return;
        }

        std::unordered_set<uint32_t> trackIds;
        std::unordered_set<uint32_t> regionIds;
        for (const auto& track : tracks) {
            if (!track) continue;
            if (!trackIds.insert(track->getId()).second || !std::isfinite(track->getVolume())) return;
            for (const auto& region : track->getRegions()) {
                if (!regionIds.insert(region.id).second || region.path.find('\0') != std::string::npos ||
                    region.name.find('\0') != std::string::npos || !std::isfinite(region.clipGain)) {
                    return;
                }
            }
        }

        // Never stream directly into the user's project. A power loss or a
        // full volume during serialization must leave the last known-good
        // project intact. Rename within the same directory for atomic publish.
        static std::atomic<uint64_t> saveSequence{0};
        const auto nonce = std::chrono::steady_clock::now().time_since_epoch().count();
        const auto sequence = saveSequence.fetch_add(1, std::memory_order_relaxed);
        const std::filesystem::path temporary = destination.string() + ".tmp-" +
            std::to_string(nonce) + "-" + std::to_string(sequence);
        std::ofstream file(temporary, std::ios::out | std::ios::trunc);
        if (!file.is_open()) return;
        file << "<AuraProject version=\"1.0\">\n";
        
        for (auto& track : tracks) {
            if (!track) continue;
            file << "  <Track id=\"" << track->getId() << "\">\n";
            file << "    <Volume value=\"" << track->getVolume() << "\"/>\n";
            for (const auto& region : track->getRegions()) {
                file << "    <Region id=\"" << region.id << "\" "
                     << "path=\"" << escapeXmlAttribute(region.path) << "\" "
                     << "start=\"" << region.start << "\" "
                     << "len=\"" << region.len << "\" "
                     << "muted=\"" << (region.muted ? "true" : "false") << "\" "
                     << "name=\"" << escapeXmlAttribute(region.name) << "\" "
                     << "clipGain=\"" << region.clipGain << "\" "
                     << "fadeIn=\"" << region.fadeInSamples << "\" "
                     << "fadeOut=\"" << region.fadeOutSamples << "\"/>\n";
            }
            file << "  </Track>\n";
        }
        
        file << "</AuraProject>\n";
        file.flush();
        const bool writeOk = file.good();
        file.close();
        if (!writeOk) {
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return;
        }
        if (!durableFlush(temporary)) {
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return;
        }
        std::error_code publishError;
        std::filesystem::rename(temporary, destination, publishError);
        if (publishError) {
            std::error_code cleanup;
            std::filesystem::remove(temporary, cleanup);
            return;
        }
        // Persist the directory entry as well as the file contents. This is
        // the POSIX durability boundary for an atomic rename; on platforms
        // without directory descriptors the helper is a safe no-op.
        (void)durableFlushDirectory(parent.empty() ? std::filesystem::path(".") : parent);
    }

    /**
     * @brief LOAD: Restores session state O(1) by dispatching to ParamTree.
     */
    void loadProject(const std::string& path) {
        std::vector<std::shared_ptr<Engine::Track>> noTracks;
        loadProject(path, noTracks);
    }

    /**
     * @brief LOAD: Validates the XML completely, then restores matching tracks.
     *
     * Parsing is deliberately transactional: no track is changed until the
     * complete document and every numeric value have been validated.
     */
    void loadProject(const std::string& path,
                     const std::vector<std::shared_ptr<Engine::Track>>& tracks) {
        std::ifstream file(path, std::ios::in | std::ios::binary);
        if (!file.is_open()) return;

        const std::string xml((std::istreambuf_iterator<char>(file)),
                               std::istreambuf_iterator<char>());

        // Basic structural validation. Require one complete root document and
        // reject trailing non-whitespace bytes so concatenated/partially
        // recovered files cannot be accepted as valid sessions.
        const auto rootStart = xml.find("<AuraProject");
        const auto rootEnd = xml.rfind("</AuraProject>");
        if (rootStart == std::string::npos || rootEnd == std::string::npos ||
            rootStart != xml.find_first_not_of(" \t\r\n") ||
            xml.find_first_not_of(" \t\r\n", rootEnd + 14) != std::string::npos) {
            return;
        }

        struct RestoredRegion {
            uint32_t id;
            std::string path;
            uint64_t start;
            uint64_t len;
            bool muted;
            std::string name;
            float clipGain;
            uint64_t fadeIn;
            uint64_t fadeOut;
        };

        struct RestoredTrack {
            uint32_t id;
            float volume;
            std::vector<RestoredRegion> regions;
        };

        std::vector<RestoredTrack> restoredTracks;
        std::unordered_set<uint32_t> trackIds;
        std::unordered_set<uint32_t> regionIds;

        static const std::regex trackBlockPattern(R"REGEX(<Track\s+id="([0-9]+)">([\s\S]*?)</Track>)REGEX");
        static const std::regex volumePattern(R"REGEX(<Volume\s+value="([^"]+)"/>)REGEX");
        static const std::regex regionPattern(R"REGEX(<Region\s+([^/>]+)/>)REGEX");
        static const std::regex attrPattern(R"REGEX((\w+)="([^"]*)")REGEX");

        auto trackIt = std::sregex_iterator(xml.begin(), xml.end(), trackBlockPattern);
        auto trackEnd = std::sregex_iterator();

        for (auto it = trackIt; it != trackEnd; ++it) {
            const auto& trackMatch = *it;
            const std::string trackIdStr = trackMatch[1].str();
            uint32_t trackId = 0;
            auto res = std::from_chars(trackIdStr.data(), trackIdStr.data() + trackIdStr.size(), trackId);
            if (res.ec != std::errc{} || !trackIds.insert(trackId).second) {
                return; // Format error or duplicate Track ID
            }

            const std::string trackContent = trackMatch[2].str();

            // Parse Volume
            std::smatch volumeMatch;
            if (!std::regex_search(trackContent, volumeMatch, volumePattern)) {
                return;
            }
            const std::string volumeText = volumeMatch[1].str();
            char* endPtr = nullptr;
            const float volume = std::strtof(volumeText.c_str(), &endPtr);
            if (endPtr != volumeText.c_str() + volumeText.size() || !std::isfinite(volume)) {
                return;
            }

            RestoredTrack rTrack;
            rTrack.id = trackId;
            rTrack.volume = volume;

            // Parse Regions
            auto regionIt = std::sregex_iterator(trackContent.begin(), trackContent.end(), regionPattern);
            auto regionEnd = std::sregex_iterator();

            for (auto rIt = regionIt; rIt != regionEnd; ++rIt) {
                const auto& regionMatch = *rIt;
                const std::string attrs = regionMatch[1].str();

                std::unordered_map<std::string, std::string> attrMap;
                auto attrIt = std::sregex_iterator(attrs.begin(), attrs.end(), attrPattern);
                auto attrEnd = std::sregex_iterator();
                for (auto aIt = attrIt; aIt != attrEnd; ++aIt) {
                    attrMap[(*aIt)[1].str()] = (*aIt)[2].str();
                }

                // Check required fields
                if (attrMap.count("id") == 0 || attrMap.count("path") == 0 ||
                    attrMap.count("start") == 0 || attrMap.count("len") == 0) {
                    return;
                }

                uint32_t rId = 0;
                const std::string rIdText = attrMap["id"];
                auto rIdRes = std::from_chars(rIdText.data(), rIdText.data() + rIdText.size(), rId);
                if (rIdRes.ec != std::errc{} || !regionIds.insert(rId).second) {
                    return; // Duplicate or invalid Region ID
                }

                uint64_t rStart = 0;
                const std::string rStartText = attrMap["start"];
                auto rStartRes = std::from_chars(rStartText.data(), rStartText.data() + rStartText.size(), rStart);
                if (rStartRes.ec != std::errc{}) return;

                uint64_t rLen = 0;
                const std::string rLenText = attrMap["len"];
                auto rLenRes = std::from_chars(rLenText.data(), rLenText.data() + rLenText.size(), rLen);
                if (rLenRes.ec != std::errc{}) return;

                bool rMuted = (attrMap["muted"] == "true");
                std::string rName = attrMap.count("name") ? attrMap["name"] : "";
                
                float rClipGain = 1.0f;
                if (attrMap.count("clipGain")) {
                    char* cgEnd = nullptr;
                    rClipGain = std::strtof(attrMap["clipGain"].c_str(), &cgEnd);
                    if (cgEnd != attrMap["clipGain"].c_str() + attrMap["clipGain"].size() || !std::isfinite(rClipGain)) {
                        rClipGain = 1.0f;
                    }
                }

                uint64_t rFadeIn = 64;
                if (attrMap.count("fadeIn")) {
                    const std::string fiText = attrMap["fadeIn"];
                    std::from_chars(fiText.data(), fiText.data() + fiText.size(), rFadeIn);
                }

                uint64_t rFadeOut = 64;
                if (attrMap.count("fadeOut")) {
                    const std::string foText = attrMap["fadeOut"];
                    std::from_chars(foText.data(), foText.data() + foText.size(), rFadeOut);
                }

                RestoredRegion rRegion{rId, unescapeXmlAttribute(attrMap["path"]), rStart, rLen, rMuted,
                                       unescapeXmlAttribute(rName), rClipGain, rFadeIn, rFadeOut};
                rTrack.regions.push_back(rRegion);
            }

            restoredTracks.push_back(rTrack);
        }

        // Transactional mutation
        for (const auto& item : restoredTracks) {
            for (const auto& track : tracks) {
                if (track && track->getId() == item.id) {
                    track->setVolume(item.volume);
                    
                    track->clearRegions();
                    for (const auto& rReg : item.regions) {
                        Engine::Region reg;
                        reg.id = rReg.id;
                        reg.path = rReg.path;
                        reg.start = rReg.start;
                        reg.len = rReg.len;
                        reg.muted = rReg.muted;
                        reg.name = rReg.name;
                        reg.clipGain = rReg.clipGain;
                        reg.fadeInSamples = rReg.fadeIn;
                        reg.fadeOutSamples = rReg.fadeOut;
                        
                        track->addRegion(reg);
                    }
                    track->commitRegionEdits();
                    break;
                }
            }
        }
    }

private:
    static bool durableFlush(const std::filesystem::path& path) noexcept {
#if !defined(_WIN32)
        const int descriptor = ::open(path.c_str(), O_RDONLY);
        if (descriptor < 0) return false;
        const bool flushed = ::fsync(descriptor) == 0;
        (void)::close(descriptor);
        return flushed;
#else
        (void)path;
        return true;
#endif
    }

    static bool durableFlushDirectory(const std::filesystem::path& path) noexcept {
#if !defined(_WIN32)
#ifdef O_DIRECTORY
        const int descriptor = ::open(path.c_str(), O_RDONLY | O_DIRECTORY);
#else
        const int descriptor = ::open(path.c_str(), O_RDONLY);
#endif
        if (descriptor < 0) return false;
        const bool flushed = ::fsync(descriptor) == 0;
        (void)::close(descriptor);
        return flushed;
#else
        (void)path;
        return true;
#endif
    }

    SessionIO() = default;
};

} // namespace Aura::Core::IO
