#pragma once
#include <map>
#include <string>

namespace Aura::Core::Mixing {

/**
 * @class QualitativeMetricEngine
 * @brief Calculates "Aesthetic Scores" using neural inference and fuzzy logic.
 */
class QualitativeMetricEngine {
public:
    static QualitativeMetricEngine& getInstance() {
        static QualitativeMetricEngine instance;
        return instance;
    }

    /**
     * @brief Calculates qualitative scores based on extracted features.
     */
    std::map<std::string, float> calculateScores(const AestheticFeatures& features) {
        std::map<std::string, float> scores;
        scores["Clarity"] = features.transientClarity * 100.0f;
        scores["Warmth"] = (1.0f - features.spectralBalance) * 100.0f;
        scores["Punch"] = features.dynamicComplexity * 100.0f;
        scores["Width"] = features.stereoWidth * 100.0f;
        return scores;
    }

private:
    QualitativeMetricEngine() = default;
};

} // namespace Aura::Core::Mixing
