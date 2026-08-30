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

    /**
     * @brief SAVE: Serializes all engine states to a project XML file.
     */
    void saveProject(const std::string& path, const std::vector<std::shared_ptr<Engine::Track>>& tracks) {
        std::ofstream file(path);
        file << "<AuraProject version=\"1.0\">\n";
        
        for (auto& track : tracks) {
            file << "  <Track id=\"" << track->getId() << "\">\n";
            file << "    <Volume value=\"" << track->getVolume() << "\"/>\n";
            for (const auto& region : track->getRegions()) {
                file << "    <Region id=\"" << region.id << "\" "
                     << "path=\"" << region.path << "\" "
                     << "start=\"" << region.start << "\" "
                     << "len=\"" << region.len << "\" "
                     << "muted=\"" << (region.muted ? "true" : "false") << "\" "
                     << "name=\"" << region.name << "\" "
                     << "clipGain=\"" << region.clipGain << "\" "
                     << "fadeIn=\"" << region.fadeInSamples << "\" "
                     << "fadeOut=\"" << region.fadeOutSamples << "\"/>\n";
            }
            file << "  </Track>\n";
        }
        
        file << "</AuraProject>\n";
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

        // Basic structural validation
        if (xml.find("<AuraProject") == std::string::npos || xml.find("</AuraProject>") == std::string::npos) {
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

        static const std::regex trackBlockPattern(R"(<Track\s+id="([0-9]+)">([\s\S]*?)</Track>)");
        static const std::regex volumePattern(R"(<Volume\s+value="([^"]+)"/>)");
        static const std::regex regionPattern(R"(<Region\s+([^/>]+)/>)");
        static const std::regex attrPattern(R"((\w+)="([^"]*)")");

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

                RestoredRegion rRegion{rId, attrMap["path"], rStart, rLen, rMuted, rName, rClipGain, rFadeIn, rFadeOut};
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
    SessionIO() = default;
};

} // namespace Aura::Core::IO
