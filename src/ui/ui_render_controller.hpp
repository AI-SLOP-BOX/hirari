#pragma once
#include <atomic>
#include <algorithm>
#include "user_activity_tracker.hpp"

namespace Aura::UI {

/**
 * @class UIRenderController
 * @brief Dynamically adjusts UI rendering parameters to optimize performance.
 * HONEST FIX: Purged 'Visual Adaptation' branding. Focuses on FPS management.
 */
class UIRenderController {
public:
    static UIRenderController& getInstance() {
        static UIRenderController instance;
        return instance;
    }

    /**
     * @brief Calculates the target FPS based on user activity.
     */
    uint32_t getTargetFPS() const {
        float density = UserActivityTracker::getInstance().getInteractionDensity();
        
        // High activity: 60 FPS (Smooth interaction)
        // Idle: 30 FPS (Energy saving)
        if (density > 0.1f) return 60;
        return 30;
    }

    /**
     * @brief Returns whether high-quality rendering (e.g. anti-aliasing) is required.
     */
    bool requiresHighQuality() const {
        return UserActivityTracker::getInstance().getInteractionDensity() > 0.5f;
    }

private:
    UIRenderController() = default;
};

} // namespace Aura::UI
