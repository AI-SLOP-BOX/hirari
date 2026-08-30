#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

namespace Aura::Core::Mixing {

class ControlRoom {
public:
    struct SpeakerSet { std::string name; float gain = 1.0f; bool enabled = true; };
    struct CueMix { uint32_t id = 0; float gain = 1.0f; bool enabled = true; };

    bool addSpeakerSet(std::string name, float gain = 1.0f) {
        if (name.empty() || !std::isfinite(gain) || gain < 0.0f || gain > 4.0f) return false;
        m_speakers.push_back({std::move(name), gain, true});
        if (m_activeSpeaker >= m_speakers.size()) m_activeSpeaker = m_speakers.size() - 1;
        return true;
    }
    bool selectSpeakerSet(std::size_t index) noexcept {
        if (index >= m_speakers.size()) return false;
        m_activeSpeaker = index; return true;
    }
    void setDim(bool enabled) noexcept { m_dim = enabled; }
    bool isDimmed() const noexcept { return m_dim; }
    void setTalkback(bool enabled, float gain = 1.0f) noexcept {
        m_talkback = enabled;
        if (std::isfinite(gain)) m_talkbackGain = std::clamp(gain, 0.0f, 4.0f);
    }
    bool talkbackEnabled() const noexcept { return m_talkback; }
    float monitorGain() const noexcept {
        if (m_speakers.empty() || !m_speakers[m_activeSpeaker].enabled) return 0.0f;
        const float dimGain = m_dim ? 0.1f : 1.0f;
        return m_speakers[m_activeSpeaker].gain * dimGain;
    }
    void processMonitor(float* left, float* right, std::size_t frames) const noexcept {
        if (!left || !right) return;
        const float gain = monitorGain();
        for (std::size_t i = 0; i < frames; ++i) {
            left[i] *= gain;
            right[i] *= gain;
        }
    }
    bool upsertCueMix(uint32_t id, float gain, bool enabled = true) {
        if (id == 0 || !std::isfinite(gain) || gain < 0.0f || gain > 4.0f) return false;
        for (auto& cue : m_cues) if (cue.id == id) { cue = {id, gain, enabled}; return true; }
        m_cues.push_back({id, gain, enabled}); return true;
    }
    const std::vector<SpeakerSet>& speakerSets() const noexcept { return m_speakers; }
    const std::vector<CueMix>& cueMixes() const noexcept { return m_cues; }
    std::size_t activeSpeakerSet() const noexcept { return m_activeSpeaker; }

private:
    std::vector<SpeakerSet> m_speakers;
    std::vector<CueMix> m_cues;
    std::size_t m_activeSpeaker = 0;
    float m_talkbackGain = 1.0f;
    bool m_dim = false;
    bool m_talkback = false;
};

} // namespace Aura::Core::Mixing
