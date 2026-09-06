#pragma once
#include <vector>
#include <cmath>
#include <algorithm>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::UI {

/**
 * @class SpectrogramView
 * @brief High-fidelity 2D Spectrogram (Time-Frequency Heatmap).
 * HONEST FIX: Replaces 1D Spectrum with a professional 2D analysis tool 
 * mapping Magnitude to Color (Fire/Spectra palette).
 * Supports Log-Freq Y-axis for accurate musical visualization.
 */
class SpectrogramView {
public:
    struct SpectrogramData {
        std::vector<std::vector<float>> samples; // [time][freq]
        float minFreq = 20.0f, maxFreq = 20000.0f;
    };
    // Source compatibility for early clients that used the misspelled name.
    using SpectrogamData = SpectrogramData;

    struct Selection {
        float t0 = 0.0f, t1 = 0.0f;
        float f0 = 0.0f, f1 = 0.0f;
        bool valid = false;
    };

    void beginSelection(float px, float py) noexcept {
        m_lassoActive = true;
        m_lassoX = px; m_lassoY = py;
        m_lassoW = 0.0f; m_lassoH = 0.0f;
    }

    void updateSelection(float px, float py) noexcept {
        if (!m_lassoActive) return;
        m_lassoW = px - m_lassoX;
        m_lassoH = py - m_lassoY;
    }

    Selection endSelection(float x, float y, float w, float h,
                           float minFreq = 20.0f, float maxFreq = 20000.0f,
                           float durationSec = 1.0f) noexcept {
        Selection out{};
        if (!m_lassoActive || !std::isfinite(x) || !std::isfinite(y) ||
            !std::isfinite(w) || !std::isfinite(h) || w <= 0.0f || h <= 0.0f ||
            !std::isfinite(minFreq) || !std::isfinite(maxFreq) ||
            !std::isfinite(durationSec) || durationSec <= 0.0f) {
            m_lassoActive = false;
            return out;
        }
        minFreq = std::clamp(minFreq, 1.0f, 192000.0f);
        maxFreq = std::clamp(maxFreq, minFreq + 1.0f, 384000.0f);
        durationSec = std::clamp(durationSec, 1.0e-6f, 86'400.0f);
        if (!std::isfinite(m_lassoX) || !std::isfinite(m_lassoY) ||
            !std::isfinite(m_lassoW) || !std::isfinite(m_lassoH)) {
            m_lassoActive = false;
            return out;
        }
        const float x0 = std::clamp(std::min(m_lassoX, m_lassoX + m_lassoW), x, x + w);
        const float x1 = std::clamp(std::max(m_lassoX, m_lassoX + m_lassoW), x, x + w);
        const float y0 = std::clamp(std::min(m_lassoY, m_lassoY + m_lassoH), y, y + h);
        const float y1 = std::clamp(std::max(m_lassoY, m_lassoY + m_lassoH), y, y + h);
        const float logMin = std::log10(minFreq);
        const float logMax = std::log10(maxFreq);
        const auto toFreq = [&](float py) {
            const float norm = std::clamp(1.0f - (py - y) / h, 0.0f, 1.0f);
            return std::pow(10.0f, logMin + norm * (logMax - logMin));
        };
        out.t0 = std::clamp((x0 - x) / w * durationSec, 0.0f, durationSec);
        out.t1 = std::clamp((x1 - x) / w * durationSec, 0.0f, durationSec);
        out.f0 = std::min(toFreq(y1), toFreq(y0));
        out.f1 = std::max(toFreq(y1), toFreq(y0));
        out.valid = out.t1 > out.t0 && out.f1 > out.f0;
        m_lassoActive = false;
        if (out.valid) {
            m_lastSelection = out;
            m_selectionDuration = durationSec;
        }
        return out;
    }

    const Selection& lastSelection() const noexcept { return m_lastSelection; }
    Selection takeSelection() noexcept {
        const Selection selected = m_lastSelection;
        clearSelection();
        return selected;
    }
    void clearSelection() noexcept {
        m_lastSelection = Selection{};
        m_lassoActive = false;
        m_lassoW = 0.0f;
        m_lassoH = 0.0f;
        m_selectionDuration = 1.0f;
    }

    /**
     * @brief High-fidelity spectrogram rendering with industrial precision and visual sovereignty.
     * INDUSTRIAL: Delegating data processing and color mapping to the Rust 'SpectrogramOrchestrator'.
     */
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, const SpectrogramData& data) {
        if (w <= 0.0f || h <= 0.0f || data.samples.empty()) return;
        const float minFreq = std::max(1.0f, std::isfinite(data.minFreq) ? data.minFreq : 20.0f);
        const float maxFreq = std::max(minFreq + 1.0f, std::isfinite(data.maxFreq) ? data.maxFreq : 20000.0f);
        kernel.drawGradientRect(x, y, w, h, 0xFF090B12, 0xFF030407);
        kernel.pushScissor(x, y, w, h);

        const size_t timeBins = std::min<size_t>(data.samples.size(), 1024u);
        for (size_t tx = 0; tx < timeBins; ++tx) {
            const auto& column = data.samples[(tx * data.samples.size()) / timeBins];
            if (column.empty()) continue;
            const size_t freqBins = std::min<size_t>(column.size(), 512u);
            const float cellW = w / static_cast<float>(timeBins);
            for (size_t fy = 0; fy < freqBins; ++fy) {
                const float magnitude = std::isfinite(column[(fy * column.size()) / freqBins])
                    ? column[(fy * column.size()) / freqBins] : 0.0f;
                const float dbNorm = std::clamp((20.0f * std::log10(std::max(magnitude, 1.0e-6f)) + 100.0f) / 100.0f, 0.0f, 1.0f);
                if (dbNorm < 0.015f) continue;
                // Log-frequency placement matches the frequency ruler used by
                // professional spectral editors.
                const float freqNorm = static_cast<float>(fy) / static_cast<float>(std::max<size_t>(1, freqBins - 1));
                const float logNorm = (std::log10(minFreq + freqNorm * (maxFreq - minFreq)) - std::log10(minFreq)) /
                                      (std::log10(maxFreq) - std::log10(minFreq));
                const float cellH = h / static_cast<float>(freqBins);
                kernel.drawRect(x + static_cast<float>(tx) * cellW,
                                y + h - (logNorm + 1.0f / static_cast<float>(freqBins)) * h,
                                cellW + 1.0f, std::max(1.0f, cellH), valToColor(dbNorm));
            }
        }
        kernel.popScissor();
        if (m_lastSelection.valid && w > 0.0f && h > 0.0f) {
            const float sx0 = std::clamp(x +
                (m_lastSelection.t0 / std::max(m_selectionDuration, 1.0e-6f)) * w, x, x + w);
            const float sx1 = std::clamp(x +
                (m_lastSelection.t1 / std::max(m_selectionDuration, 1.0e-6f)) * w, x, x + w);
            const float logMin = std::log10(minFreq), logMax = std::log10(maxFreq);
            const auto toY = [&](float frequency) {
                const float norm = std::clamp((std::log10(std::clamp(frequency, minFreq, maxFreq)) - logMin) /
                    std::max(logMax - logMin, 1.0e-6f), 0.0f, 1.0f);
                return y + (1.0f - norm) * h;
            };
            const float sy0 = toY(m_lastSelection.f1);
            const float sy1 = toY(m_lastSelection.f0);
            kernel.drawRect(std::min(sx0, sx1), std::min(sy0, sy1),
                            std::fabs(sx1 - sx0), std::fabs(sy1 - sy0), 0x3322AAFF);
        }
        if (m_lassoActive) {
            const float sx = std::min(m_lassoX, m_lassoX + m_lassoW);
            const float sy = std::min(m_lassoY, m_lassoY + m_lassoH);
            kernel.drawRect(sx, sy, std::fabs(m_lassoW), std::fabs(m_lassoH), 0x55FFFFFF);
        }
    }

private:
    uint32_t valToColor(float val) {
        val = std::clamp(std::isfinite(val) ? val : 0.0f, 0.0f, 1.0f);
        // Black -> violet -> red -> amber -> white spectral palette.
        const float r = std::clamp(255.0f * (val * 2.4f - 0.15f), 0.0f, 255.0f);
        const float g = std::clamp(255.0f * (val * 1.7f - 0.55f), 0.0f, 255.0f);
        const float b = std::clamp(255.0f * (1.15f - val * 1.45f), 20.0f, 255.0f);
        return 0xFF000000u | (static_cast<uint32_t>(r) << 16u) |
               (static_cast<uint32_t>(g) << 8u) | static_cast<uint32_t>(b);
    }

    bool m_lassoActive = false;
    float m_lassoX, m_lassoY, m_lassoW, m_lassoH;
    Selection m_lastSelection{};
    float m_selectionDuration = 1.0f;
};

} // namespace Aura::Graphics::UI
