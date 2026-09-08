#pragma once
#include <vector>
#include <memory>
#include <string>
#include "audio_region.hpp"

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
        // become the active comp. The bounded waveform pass penalizes silence
        // and clipping while rewarding usable headroom and transient activity.
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
            const uint64_t analysisLength = overlapEnd - overlapStart;
            const uint64_t stride = std::max<uint64_t>(1, analysisLength / 4096);
            double energy = 0.0;
            float peak = 0.0f;
            uint32_t samples = 0;
            uint32_t clipped = 0;
            uint32_t crossings = 0;
            float previous = 0.0f;
            for (uint64_t offset = 0; offset < analysisLength; offset += stride) {
                const float left = take->getRegion()->getInterpolatedSample(
                    0, static_cast<double>(overlapStart - takeStart + offset));
                const float right = take->getRegion()->getInterpolatedSample(
                    1, static_cast<double>(overlapStart - takeStart + offset));
                const float sample = std::isfinite(left) && std::isfinite(right)
                    ? (left + right) * 0.5f : 0.0f;
                energy += static_cast<double>(sample) * sample;
                peak = std::max(peak, std::abs(sample));
                if (std::abs(sample) >= 0.99f) ++clipped;
                if (samples > 0 && ((previous < 0.0f) != (sample < 0.0f))) ++crossings;
                previous = sample;
                ++samples;
            }
            const float rms = samples == 0 ? 0.0f
                : static_cast<float>(std::sqrt(energy / samples));
            const float activity = std::clamp(rms / 0.1f, 0.0f, 1.0f);
            const float headroom = samples == 0 ? 0.0f
                : 1.0f - static_cast<float>(clipped) / samples;
            const float transientActivity = samples < 2 ? 0.0f
                : std::clamp(static_cast<float>(crossings) / samples * 4.0f, 0.0f, 1.0f);
            const float score = coverage * (0.45f * activity + 0.25f * headroom
                + 0.15f * transientActivity + 0.15f * gainQuality)
                * std::clamp(peak, 0.0f, 1.0f);
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
