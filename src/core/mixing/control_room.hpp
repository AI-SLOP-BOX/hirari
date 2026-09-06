#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>
#include <atomic>
#include <mutex>

namespace Aura::Core::Mixing {

class ControlRoom {
public:
    struct SpeakerSet { std::string name; float gain = 1.0f; bool enabled = true; };
    struct CueMix { uint32_t id = 0; float gain = 1.0f; bool enabled = true; };

    // A fresh engine must remain audible before the UI creates custom
    // monitor sets.  The built-in Main output mirrors a conventional DAW's
    // default Control Room monitor and avoids an accidental -inf output.
    ControlRoom() {
        m_speakers.push_back({"Main", 1.0f, true});
        publishMonitorGain();
    }
    void resetForProject() {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        m_speakers.clear();
        m_speakers.push_back({"Main", 1.0f, true});
        m_cues.clear();
        m_activeSpeaker = 0;
        m_dim = false;
        m_talkback = false;
        m_talkbackGain = 1.0f;
        m_rtTalkbackEnabled.store(false, std::memory_order_release);
        m_rtTalkbackGain.store(1.0f, std::memory_order_release);
        publishMonitorGain();
    }

    bool addSpeakerSet(std::string name, float gain = 1.0f) {
        if (name.empty() || name.size() > 128 || name.find('\0') != std::string::npos ||
            !std::isfinite(gain) || gain < 0.0f || gain > 4.0f) return false;
        std::lock_guard<std::mutex> lock(m_stateMutex);
        m_speakers.push_back({std::move(name), gain, true});
        if (m_activeSpeaker >= m_speakers.size()) m_activeSpeaker = m_speakers.size() - 1;
        publishMonitorGain();
        return true;
    }
    bool selectSpeakerSet(std::size_t index) noexcept {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        if (index >= m_speakers.size()) return false;
        m_activeSpeaker = index; publishMonitorGain(); return true;
    }
    bool removeSpeakerSet(std::size_t index) {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        if (index >= m_speakers.size() || m_speakers.size() <= 1) return false;
        m_speakers.erase(m_speakers.begin() + static_cast<std::ptrdiff_t>(index));
        if (m_activeSpeaker > index) --m_activeSpeaker;
        else if (m_activeSpeaker >= m_speakers.size()) m_activeSpeaker = m_speakers.size() - 1;
        publishMonitorGain();
        return true;
    }
    bool renameSpeakerSet(std::size_t index, std::string name) {
        if (name.empty() || name.size() > 128 || name.find('\0') != std::string::npos) return false;
        std::lock_guard<std::mutex> lock(m_stateMutex);
        if (index >= m_speakers.size()) return false;
        m_speakers[index].name = std::move(name);
        return true;
    }
    bool setSpeakerGain(std::size_t index, float gain) {
        if (!std::isfinite(gain) || gain < 0.0f || gain > 4.0f) return false;
        std::lock_guard<std::mutex> lock(m_stateMutex);
        if (index >= m_speakers.size()) return false;
        m_speakers[index].gain = gain;
        publishMonitorGain();
        return true;
    }
    bool setSpeakerEnabled(std::size_t index, bool enabled) {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        if (index >= m_speakers.size()) return false;
        m_speakers[index].enabled = enabled;
        publishMonitorGain();
        return true;
    }
    void setDim(bool enabled) noexcept { std::lock_guard<std::mutex> lock(m_stateMutex); m_dim = enabled; publishMonitorGain(); }
    bool isDimmed() const noexcept { std::lock_guard<std::mutex> lock(m_stateMutex); return m_dim; }
    void setTalkback(bool enabled, float gain = 1.0f) noexcept {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        m_talkback = enabled;
        if (std::isfinite(gain)) m_talkbackGain = std::clamp(gain, 0.0f, 4.0f);
        m_rtTalkbackEnabled.store(enabled, std::memory_order_release);
        m_rtTalkbackGain.store(m_talkbackGain, std::memory_order_release);
    }
    bool talkbackEnabled() const noexcept { return m_rtTalkbackEnabled.load(std::memory_order_acquire); }
    float monitorGain() const noexcept {
        return m_rtMonitorGain.load(std::memory_order_acquire);
    }
    void processMonitor(float* left, float* right, std::size_t frames) const noexcept {
        if (!left || !right) return;
        // The audio callback only reads this atomic; speaker-set vectors are
        // control-plane state and may be edited concurrently by the UI.
        const float gain = std::clamp(m_rtMonitorGain.load(std::memory_order_acquire), 0.0f, 4.0f);
        for (std::size_t i = 0; i < frames; ++i) {
            const float inL = std::isfinite(left[i]) ? left[i] : 0.0f;
            const float inR = std::isfinite(right[i]) ? right[i] : 0.0f;
            left[i] = std::clamp(inL * gain, -16.0f, 16.0f);
            right[i] = std::clamp(inR * gain, -16.0f, 16.0f);
        }
    }

    // Optional talkback source is mixed only into the monitor path. The
    // rendered master remains untouched, matching a dedicated control-room
    // talkback circuit.
    void processMonitorWithTalkback(float* left, float* right, const float* talkback,
                                    std::size_t frames) const noexcept {
        processMonitor(left, right, frames);
        if (!left || !right || !talkback ||
            !m_rtTalkbackEnabled.load(std::memory_order_acquire)) return;
        const float gain = std::clamp(m_rtTalkbackGain.load(std::memory_order_acquire), 0.0f, 4.0f);
        for (std::size_t i = 0; i < frames; ++i) {
            const float sample = std::isfinite(talkback[i]) ? std::clamp(talkback[i], -16.0f, 16.0f) * gain : 0.0f;
            left[i] = std::clamp((std::isfinite(left[i]) ? left[i] : 0.0f) + sample, -16.0f, 16.0f);
            right[i] = std::clamp((std::isfinite(right[i]) ? right[i] : 0.0f) + sample, -16.0f, 16.0f);
        }
    }
    bool upsertCueMix(uint32_t id, float gain, bool enabled = true) {
        if (id == 0 || !std::isfinite(gain) || gain < 0.0f || gain > 4.0f) return false;
        std::lock_guard<std::mutex> lock(m_stateMutex);
        for (auto& cue : m_cues) if (cue.id == id) { cue = {id, gain, enabled}; return true; }
        m_cues.push_back({id, gain, enabled}); return true;
    }
    bool removeCueMix(uint32_t id) {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        const auto before = m_cues.size();
        m_cues.erase(std::remove_if(m_cues.begin(), m_cues.end(),
                                    [id](const CueMix& cue) { return cue.id == id; }),
                     m_cues.end());
        return m_cues.size() != before;
    }
    bool setCueMixEnabled(uint32_t id, bool enabled) {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        for (auto& cue : m_cues) if (cue.id == id) { cue.enabled = enabled; return true; }
        return false;
    }
    float cueGain(uint32_t id) const noexcept {
        // Cue reads are control-plane by design; callers that need RT audio
        // should publish the selected gain into their own atomic snapshot.
        std::lock_guard<std::mutex> lock(m_stateMutex);
        for (const auto& cue : m_cues)
            if (cue.id == id) return cue.enabled ? cue.gain : 0.0f;
        return 0.0f;
    }
    bool validate() const noexcept {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        if (m_speakers.empty() || m_activeSpeaker >= m_speakers.size()) return false;
        for (const auto& speaker : m_speakers)
            if (speaker.name.empty() || speaker.name.size() > 128 ||
                !std::isfinite(speaker.gain) || speaker.gain < 0.0f || speaker.gain > 4.0f)
                return false;
        for (const auto& cue : m_cues)
            if (cue.id == 0 || !std::isfinite(cue.gain) || cue.gain < 0.0f || cue.gain > 4.0f)
                return false;
        return true;
    }
    const std::vector<SpeakerSet>& speakerSets() const noexcept { return m_speakers; }
    std::vector<SpeakerSet> speakerSetsSnapshot() const { std::lock_guard<std::mutex> lock(m_stateMutex); return m_speakers; }
    const std::vector<CueMix>& cueMixes() const noexcept { return m_cues; }
    std::vector<CueMix> cueMixesSnapshot() const { std::lock_guard<std::mutex> lock(m_stateMutex); return m_cues; }
    std::size_t activeSpeakerSet() const noexcept {
        std::lock_guard<std::mutex> lock(m_stateMutex);
        return m_activeSpeaker;
    }

private:
    void publishMonitorGain() noexcept {
        float gain = 0.0f;
        if (!m_speakers.empty() && m_activeSpeaker < m_speakers.size() &&
            m_speakers[m_activeSpeaker].enabled) {
            gain = m_speakers[m_activeSpeaker].gain * (m_dim ? 0.1f : 1.0f);
        }
        m_rtMonitorGain.store(gain, std::memory_order_release);
    }
    std::vector<SpeakerSet> m_speakers;
    std::vector<CueMix> m_cues;
    std::size_t m_activeSpeaker = 0;
    float m_talkbackGain = 1.0f;
    bool m_dim = false;
    bool m_talkback = false;
    std::atomic<float> m_rtMonitorGain{0.0f};
    std::atomic<float> m_rtTalkbackGain{1.0f};
    std::atomic<bool> m_rtTalkbackEnabled{false};
    mutable std::mutex m_stateMutex;
};

} // namespace Aura::Core::Mixing
