#pragma once
#include <string>
#include <map>
#include <memory>
#include <mutex>
#include "../mmap_audio_source.hpp"

namespace Hirari::IO::Assets {

/**
 * @class AssetResolver
 * @brief THE REGISTRAR: Ensures single-instance loading of audio files.
 * SOLVES: Point 4 of the audit. Prevents RAM explosion by sharing 
 * the SAME memory buffer across multiple tracks/regions using the same asset.
 */
class AssetResolver {
public:
    static AssetResolver& getInstance() { static AssetResolver i; return i; }
    
    /**
     * @brief Resolves a path to a shared AudioSource.
     * HONEST FIX: Uses a weak_ptr-based cache to automatically 
     * purge buffers when NO tracks are using them (Smart Lifecycle).
     */
    std::shared_ptr<Core::MMapAudioSource> resolveSource(const std::string& path) {
        std::lock_guard<std::mutex> lock(m_mutex);
        
        if (auto existing = m_cache[path].lock()) {
            return existing;
        }
        
        auto newSource = std::make_shared<Core::MMapAudioSource>(path);
        m_cache[path] = newSource;
        return newSource;
    }
    
private:
    AssetResolver() = default;
    
    std::map<std::string, std::weak_ptr<Core::MMapAudioSource>> m_cache;
    std::mutex m_mutex;
};

} // namespace Hirari::IO::Assets
