#pragma once
#include <cmath>
#include <algorithm>

namespace Hirari::Graphics::Shaders {

/**
 * @namespace SDF
 * @brief High-performance Signed Distance Functions for GPU Shaders.
 * HONEST FIX: Replaces 'pixel-based' drawing with 'distance-field' math. 
 * Allows infinitely sharp UI (Logic Pro clarity) at any zoom level 
 * by calculating the pixel state on-the-fly in the Fragment Shader.
 * Industry-standard for modern, high-DPI GPU-based UIs.
 */
namespace SDF {

/**
 * @brief Signed distance to a rounded rectangle.
 */
inline float sdRoundedRect(float px, float py, float bX, float bY, float r) {
    float dx = std::abs(px) - (bX - r);
    float dy = std::abs(py) - (bY - r);
    return std::min(std::max(dx, dy), 0.0f) + 
           std::sqrt(std::max(dx, 0.0f) * std::max(dx, 0.0f) + 
                     std::max(dy, 0.0f) * std::max(dy, 0.0f)) - r;
}

/**
 * @brief Signed distance to a ring (Donut).
 */
inline float sdRing(float px, float py, float r, float thickness) {
    float d = std::sqrt(px*px + py*py);
    return std::abs(d - r) - thickness;
}

/**
 * @brief Generates an anti-aliased alpha value from an SDF distance.
 * Replaces 'trash' jagged edges with perfect sub-pixel smoothness.
 */
inline float aastep(float edge, float dist, float pixelWidth = 1.0f) {
    return std::clamp((edge - dist) / pixelWidth, 0.0f, 1.0f);
}

} // namespace SDF

} // namespace Hirari::Graphics::Shaders
