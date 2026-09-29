#pragma once
#include <map>
#include <string>
#include "../../io/wav_loader_utils.hpp"
#include <filesystem>

namespace Hirari::IO::Assets {

/**
 * @brief FrozenBufferManager: Professional track freezing with disk persistence.
 * Saves frozen tracks as .frozen files to reduce both CPU and RAM pressure.
 */
class FrozenBufferManager {
public:
    static FrozenBufferManager& getInstance() {
        static FrozenBufferManager instance;
        return instance;
    }

    /**
     * @brief Freezes a track by rendering it to a disk cache.
     */
    void freezeTrack(uint32_t trackId, const std::vector<std::vector<float>>& data) {
        std::string cachePath = "cache/frozen_" + std::to_string(trackId) + ".wav";
        std::filesystem::create_directories("cache");
        
        if (IO::WavSaver::save(cachePath, data, 44100)) {
            m_frozenPaths[trackId] = cachePath;
        }
    }

    bool isFrozen(uint32_t trackId) const {
        return m_frozenPaths.find(trackId) != m_frozenPaths.end();
    }

    std::string getFrozenPath(uint32_t trackId) const {
        auto it = m_frozenPaths.find(trackId);
        return (it != m_frozenPaths.end()) ? it->second : "";
    }

private:
    FrozenBufferManager() = default;
    std::map<uint32_t, std::string> m_frozenPaths;
};

} // namespace Hirari::IO::Assets
