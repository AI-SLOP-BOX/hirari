#pragma once
#include <atomic>

namespace Aura::Core::Mixing {

/**
 * @class GlobalNormalizer
 * @brief Autonomous project-wide loudness targeting.
 */
class GlobalNormalizer {
public:
    static GlobalNormalizer& getInstance() {
        static GlobalNormalizer instance;
        return instance;
    }

    /**
     * @brief Returns the necessary gain offset to hit the target LUFS.
     */
    float getGainAdjustment(float currentLUFS, float targetLUFS = -14.0f) {
        return std::pow(10.0f, (targetLUFS - currentLUFS) / 20.0f);
    }

private:
    GlobalNormalizer() = default;
};

} // namespace Aura::Core::Mixing
