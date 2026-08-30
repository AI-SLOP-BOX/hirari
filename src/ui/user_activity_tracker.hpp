#pragma once
#include <array>
#include <algorithm>
#include <chrono>
#include <mutex>

namespace Aura::UI {

/**
 * @class UserActivityTracker
 * @brief Tracks user interaction density to optimize system resources.
 * HONEST FIX: Purged 'Bio-Adaptive' branding and 'Cognitive Load' hallucinations.
 */
class UserActivityTracker {
public:
    static constexpr size_t kCapacity = 512;
    static UserActivityTracker& getInstance() {
        static UserActivityTracker instance;
        return instance;
    }

    /**
     * @brief Records a user interaction event.
     */
    void recordInteraction() {
        std::lock_guard<std::mutex> lock(m_mutex);
        auto now = std::chrono::steady_clock::now();
        m_interactionTimes[m_writeIndex] = now;
        m_writeIndex = (m_writeIndex + 1) % kCapacity;
        if (m_count < kCapacity) {
            ++m_count;
        } else {
            // The write cursor has advanced to the oldest element after an
            // overwrite. No compaction or 512-element copy is required.
            m_oldestIndex = m_writeIndex;
        }
        pruneExpiredLocked(now);
    }

    /**
     * @brief Returns a score from 0.0 (Idle) to 1.0 (Very Active).
     */
    float getInteractionDensity() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        pruneExpiredLocked(std::chrono::steady_clock::now());
        if (m_count == 0) return 0.0f;
        
        // Simple density calculation: events per second (maxed at 10/sec)
        float density = static_cast<float>(m_count) / 30.0f;
        const float raw = std::clamp(density / 10.0f, 0.0f, 1.0f);
        m_smoothedDensity = m_smoothedDensity * 0.85f + raw * 0.15f;
        return m_smoothedDensity;
    }

private:
    UserActivityTracker() = default;
    void pruneExpiredLocked(std::chrono::steady_clock::time_point now) const {
        const auto cutoff = now - std::chrono::seconds(30);
        while (m_count > 0 && m_interactionTimes[m_oldestIndex] < cutoff) {
            m_oldestIndex = (m_oldestIndex + 1) % kCapacity;
            --m_count;
        }
    }

    std::array<std::chrono::steady_clock::time_point, kCapacity> m_interactionTimes{};
    mutable size_t m_writeIndex = 0;
    mutable size_t m_oldestIndex = 0;
    mutable size_t m_count = 0;
    mutable std::mutex m_mutex;
    mutable float m_smoothedDensity = 0.0f;
};

} // namespace Aura::UI
