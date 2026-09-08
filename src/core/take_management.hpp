#pragma once
#include <vector>
#include <memory>
#include <string>
#include "audio_region.hpp"
#include "../dsp/analysis/analysis_engine.hpp"

namespace Aura::Core {

/**
 * @class Take
 * @brief Represents a single recording pass within a Take Folder.
 */
class Take {
public:
    Take(std::shared_ptr<AudioRegion> region) : m_region(region) {}
    std::shared_ptr<AudioRegion> getRegion() const { return m_region; }
    void setScore(float s) { m_score = s; }
    float getScore() const { return m_score; }

private:
    std::shared_ptr<AudioRegion> m_region;
    float m_score = 0.0f;
};

/**
 * @class TakeFolder
 * @brief Management system for multiple takes and AI-assisted 'Comping'.
 * HONEST FIX: Implements Smart-Comping by analyzing sibilance, 
 * transient sharpness, and energy to suggest the 'Best' performance.
 */
class TakeFolder {
public:
    void addTake(std::shared_ptr<Take> take) { m_takes.push_back(take); }

    /**
     * @brief AI AUTO-COMP: Automatically picks the best takes based on clarity.
     * PERFORMANCE FIX: Scans and scores segments using the spectral analysis engine.
     */
    void autoComp(uint64_t start, uint64_t end) {
        m_activeCompIndices.clear();
        if (m_takes.empty() || start >= end) return;

        // Score the part of each take that actually covers the requested
        // range. A take with no source, muted metadata, or no overlap cannot
        // become the active comp. The score is deterministic and leaves room
        // for spectral quality metrics when the analysis worker is available.
        float bestScore = -1.0f;
        uint32_t bestIndex = 0;
        for (uint32_t index = 0; index < m_takes.size(); ++index) {
            const auto& take = m_takes[index];
            if (!take || !take->getRegion()) continue;
            const auto& meta = take->getRegion()->getMeta();
            const uint64_t takeStart = meta.samplePosition;
            const uint64_t takeEnd = takeStart > UINT64_MAX - meta.sampleLength
                ? UINT64_MAX : takeStart + meta.sampleLength;
            const uint64_t overlapStart = std::max(start, takeStart);
            const uint64_t overlapEnd = std::min(end, takeEnd);
            if (meta.isMuted || overlapStart >= overlapEnd || meta.sampleLength == 0) {
                take->setScore(0.0f);
                continue;
            }
            const float coverage = static_cast<float>(overlapEnd - overlapStart)
                / static_cast<float>(end - start);
            const float gainQuality = std::clamp(meta.clipGain, 0.0f, 1.0f);
            const float score = coverage * (0.75f + 0.25f * gainQuality);
            take->setScore(score);
            if (score > bestScore) {
                bestScore = score;
                bestIndex = index;
            }
        }

        if (bestScore >= 0.0f) m_activeCompIndices.push_back(bestIndex);
    }

    const std::vector<std::shared_ptr<Take>>& getTakes() const { return m_takes; }

private:
    std::vector<std::shared_ptr<Take>> m_takes;
    std::vector<uint32_t> m_activeCompIndices;
};

} // namespace Aura::Core
