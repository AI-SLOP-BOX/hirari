#pragma once
#include <string>
#include <vector>
#include <map>
#include <algorithm>

namespace Aura::Core::Midi {

/**
 * @enum ArticulationType
 * @brief Common musical articulations.
 */
enum class ArticulationType {
    Legato,
    Staccato,
    Pizzicato,
    Marcato,
    Tremolo
};

/**
 * @class ArticulationOrchestratorKernel
 * @brief Manages instrument-specific articulation rules.
 */
class ArticulationOrchestratorKernel {
public:
    struct Rule {
        float minVelocity;
        float maxVelocity;
        float minDuration;
        float maxDuration;
        ArticulationType type;
    };

    static ArticulationOrchestratorKernel& getInstance() {
        static ArticulationOrchestratorKernel instance;
        return instance;
    }

    void clearRules() {
        m_rules.clear();
    }

    void addRule(const Rule& rule) {
        m_rules.push_back(rule);
    }

    /**
     * @brief Maps a detected gesture to an articulation type based on registered rules.
     */
    ArticulationType resolveArticulation(float velocity, float duration) {
        for (const auto& rule : m_rules) {
            if (velocity >= rule.minVelocity && velocity <= rule.maxVelocity &&
                duration >= rule.minDuration && duration <= rule.maxDuration) {
                return rule.type;
            }
        }

        // Fallback default rules
        if (duration < 0.12f) {
            if (velocity > 0.75f) return ArticulationType::Staccato;
            return ArticulationType::Pizzicato;
        }
        if (velocity > 0.85f) return ArticulationType::Marcato;
        if (duration > 1.5f && velocity < 0.4f) return ArticulationType::Tremolo;
        
        return ArticulationType::Legato;
    }

private:
    ArticulationOrchestratorKernel() {
        // Populate standard default rules
        m_rules.push_back({0.8f, 1.0f, 0.0f, 0.1f, ArticulationType::Staccato});
        m_rules.push_back({0.0f, 0.5f, 0.0f, 0.15f, ArticulationType::Pizzicato});
        m_rules.push_back({0.85f, 1.0f, 0.1f, 1.0f, ArticulationType::Marcato});
        m_rules.push_back({0.0f, 0.4f, 1.5f, 10.0f, ArticulationType::Tremolo});
        m_rules.push_back({0.0f, 1.0f, 0.1f, 10.0f, ArticulationType::Legato});
    }

    std::vector<Rule> m_rules;
};

} // namespace Aura::Core::Midi
