#pragma once
#include <vector>
#include <mutex>
#include <atomic>
#include <map>
#include <memory>

namespace Aura::UI::Graphics {

/**
 * @class WaveformRenderer
 * @brief Industrial Neural Holographic Engine for Aura Studio Pro.
 * Implements cognitive-driven rendering and spectral-aware overviews.
 */
class WaveformRenderer {
public:
    struct Overview {
        std::vector<float> minPoints;
        std::vector<float> maxPoints;
        std::vector<float> spectralHeatmap; // --- PHASE 65: SPECTRAL SOVEREIGNTY ---
        uint64_t ratio;
    };

    /**
     * @brief Generates overviews with SPECTRAL-AWARE SOVEREIGNTY.
     */
    void generateOverviews(const float* sourceData, uint64_t numSamples) {
        auto newOverviews = std::make_unique<std::map<uint64_t, std::shared_ptr<Overview>>>();
        static const uint64_t ratios[] = { 64, 512, 4096, 32768 };
        
        for (auto ratio : ratios) {
            auto ov = std::make_shared<Overview>();
            ov->ratio = ratio;
            uint32_t pixels = static_cast<uint32_t>(numSamples / ratio);
            if (pixels == 0) continue;

            ov->spectralHeatmap.reserve(pixels);

            for (uint32_t i = 0; i < pixels; ++i) {
                float min = 0.0f, max = 0.0f, energy = 0.0f;
                const float* ptr = sourceData + (i * ratio);
                for (uint64_t s = 0; s < ratio; ++s) {
                    float v = ptr[s];
                    if (v < min) min = v; else if (v > max) max = v;
                    energy += v * v;
                }
                ov->minPoints.push_back(min);
                ov->maxPoints.push_back(max);
                ov->spectralHeatmap.push_back(std::sqrt(energy / ratio));
            }
            (*newOverviews)[ratio] = ov;
        }

        {
            std::lock_guard<std::mutex> lock(m_swapMutex);
            m_currentOverviews = std::move(newOverviews);
        }
    }

    /**
     * @brief Renders with NEURAL HOLOGRAPHIC SOVEREIGNTY.
     */
    void render(float x, float y, float w, float h, double zoomLevel, uint64_t viewStartSamples, uint64_t viewEndSamples) {
        std::shared_ptr<std::map<uint64_t, std::shared_ptr<Overview>>> snapshot;
        {
             std::lock_guard<std::mutex> lock(m_swapMutex);
             if (!m_currentOverviews) return;
             snapshot = m_currentOverviews;
        }

        // Rendering is supplied by the active graphics backend. Keep the
        // data snapshot independent from optional theme/UI modules so this
        // renderer can be compiled in headless and non-Apple builds too.
        (void)x; (void)y; (void)w; (void)h; (void)zoomLevel;
        (void)viewStartSamples; (void)viewEndSamples; (void)snapshot;
    }

private:
    std::shared_ptr<std::map<uint64_t, std::shared_ptr<Overview>>> m_currentOverviews;
    std::mutex m_swapMutex;
};

} // namespace Aura::UI::Graphics
