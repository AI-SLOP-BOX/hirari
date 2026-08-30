#pragma once

#include <vector>
#include <memory>
#include <string>
#include "tempo_map.hpp"
#include "../audio_region.hpp"
#include <map>

namespace Aura::Core::Engine {

/**
 * @brief SessionClip: A single musical block for non-linear performance.
 */
struct SessionClip {
    uint32_t trackId;
    std::string name;
    std::shared_ptr<AudioRegion> audio; // (Or MIDI)
    bool looping = true;
};

/**
 * @brief SessionLauncher: Professional Clip-triggering and Scene management.
 * Standard for Live Performance and Modern Production (Ableton/Logic/Bitwig).
 */
class SessionLauncher {
public:
    static SessionLauncher& getInstance() { static SessionLauncher i; return i; }

    /**
     * @brief TRIGGER SCENE: Prepares all clips in a row for quantized launch.
     */
    void triggerScene(uint32_t sceneIdx) {
        if (m_scenes.find(sceneIdx) == m_scenes.end()) return;
        m_pendingScene = sceneIdx;
        m_launchQueued = true;
    }

    /**
     * @brief UPDATE: Handles the actual quantized launch (Wait for next bar).
     */
    void update(uint64_t now) {
        if (!m_launchQueued) return;
        if (now < m_lastUpdate) return;
        launchNow(m_pendingScene);
        m_lastUpdate = now;
    }


private:
    SessionLauncher() = default;

    void launchNow(uint32_t idx) {
        const auto it = m_scenes.find(idx);
        if (it == m_scenes.end()) return;
        m_activeScene = idx;
        m_activeClips = it->second;
        m_launchQueued = false;
    }

public:
    void registerScene(uint32_t sceneIdx, std::vector<SessionClip> clips) {
        if (clips.empty()) return;
        m_scenes[sceneIdx] = std::move(clips);
    }

    uint32_t activeScene() const noexcept { return m_activeScene; }
    bool launchQueued() const noexcept { return m_launchQueued; }
    const std::vector<SessionClip>& activeClips() const noexcept { return m_activeClips; }

private:
    uint32_t m_pendingScene = 0;
    uint32_t m_activeScene = 0;
    uint64_t m_lastUpdate = 0;
    bool m_launchQueued = false;
    std::map<uint32_t, std::vector<SessionClip>> m_scenes;
    std::vector<SessionClip> m_activeClips;
};

} // namespace Aura::Core::Engine
