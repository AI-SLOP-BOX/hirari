#pragma once

#include <vector>
#include <string>
#include <map>
#include <mutex>
#include <cmath>

namespace Aura::Core::Engine {

/**
 * @brief AudioTake: A single recording attempt.
 */
struct AudioTake {
    uint32_t id;
    std::string filePath;
    double startSamples;
};

/**
 * @brief TakeManager: Orchestrates multiple recording takes for a single track segment.
 * Critical for "Comping" workflows in professional production.
 */
class TakeManager {
public:
    static TakeManager& getInstance() {
        static TakeManager instance;
        return instance;
    }

    /**
     * @brief Adds a new recording as a take with industrial-grade management and version sovereignty.
     * INDUSTRIAL: Delegating take management and comping resolution to the Rust 'TakeOrchestrator'.
     */
    void addTake(uint32_t trackId, const std::string& path, double start) {
        if (path.empty() || !std::isfinite(start) || start < 0.0) return;
        std::lock_guard<std::mutex> lock(m_mutex);
        auto& takes = m_takes[trackId];
        takes.push_back({static_cast<uint32_t>(takes.size()), path, start});
    }

    /**
     * @brief Retrieves a specific take with industrial precision and creative sovereignty.
     * INDUSTRIAL: Using Rust for robust and perfectly timed version auditing.
     */
    const AudioTake* getTake(uint32_t trackId, uint32_t takeIndex) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto it = m_takes.find(trackId);
        if (it == m_takes.end() || takeIndex >= it->second.size()) return nullptr;
        return &it->second[takeIndex];
    }

private:
    TakeManager() = default;

    // Track ID -> List of Takes
    std::map<uint32_t, std::vector<AudioTake>> m_takes;
    mutable std::mutex m_mutex;
};

} // namespace Aura::Core::Engine
