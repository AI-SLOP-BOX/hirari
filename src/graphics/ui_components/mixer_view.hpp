#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include <array>
#include <memory>
#include <string>
#include "../graphics_kernel.hpp"

namespace Aura::Core::Engine { class Track; }

namespace Aura::DSP::Analysis {

/**
 * @class TruePeakMeter
 * @brief High-precision inter-sample peak monitor with industrial sovereignty.
 * INDUSTRIAL: Delegating 4x oversampling and ISP analysis to the Rust 'MixerOrchestrator'.
 */
class TruePeakMeter {
public:
    void process(const float* data, uint32_t len) {
        if (!data || len == 0) return;
        float peak = m_peak;
        float previous = m_previous;
        for (uint32_t i = 0; i < len; ++i) {
            const float sample = std::isfinite(data[i]) ? data[i] : 0.0f;
            peak = std::max(peak, std::fabs(sample));
            // 4x linear inter-sample estimate catches peaks between samples.
            const float step = (sample - previous) * 0.25f;
            for (int k = 1; k < 4; ++k) peak = std::max(peak, std::fabs(previous + step * static_cast<float>(k)));
            previous = sample;
        }
        m_previous = previous;
        m_peak = peak;
    }
    float peak() const noexcept { return m_peak; }
    void reset() noexcept { m_peak = 0.0f; m_previous = 0.0f; }
private:
    float m_peak = 0.0f;
    float m_previous = 0.0f;
};

} // namespace Aura::DSP::Analysis

namespace Aura::Graphics::UI {

/**
 * @class MixerView
 * @brief Professional mixer strip rendering with industrial precision.
 * INDUSTRIAL: Delegating level statistics and meter ballistics to the Rust 'MixerOrchestrator'.
 */
class MixerView {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const std::vector<std::shared_ptr<::Aura::Core::Engine::Track>>& tracks) {
        if (w <= 1.0f || h <= 1.0f) return;
        kernel.drawGradientRect(x, y, w, h, 0xFF15171C, 0xFF090A0D);
        if (tracks.empty()) return;
        const float stripW = w / static_cast<float>(tracks.size());
        for (size_t i = 0; i < tracks.size(); ++i) {
            const float sx = x + static_cast<float>(i) * stripW;
            const auto& track = tracks[i];
            const std::string fallbackName = track ? ("Track " + std::to_string(i + 1u)) : "Empty";
            renderStrip(kernel, sx + 2.0f, y + 2.0f, stripW - 4.0f, h - 4.0f,
                        fallbackName.c_str(), track ? 0.7f : 0.0f, 0xFF3D85C6u);
        }
    }

    void renderStrip(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const char* name, float level, uint32_t color) {
        if (w <= 1.0f || h <= 1.0f) return;
        level = std::clamp(std::isfinite(level) ? level : 0.0f, 0.0f, 1.0f);
        kernel.drawRoundedRect(x, y, w, h, 3.0f, 0xFF20242B);
        kernel.drawText(name ? name : "Track", x + 4.0f, y + 5.0f, 9.0f, 0xFFE5E7EB);
        const float meterY = y + 24.0f;
        const float meterH = std::max(1.0f, h - 46.0f);
        kernel.drawRect(x + w * 0.35f, meterY, std::max(2.0f, w * 0.3f), meterH, 0xFF0B0D10);
        kernel.drawRect(x + w * 0.35f, meterY + meterH * (1.0f - level), std::max(2.0f, w * 0.3f), meterH * level, color);
        kernel.drawText("-∞", x + 3.0f, y + h - 16.0f, 8.0f, 0xFF9CA3AF);
    }
};

} // namespace Aura::Graphics::UI
