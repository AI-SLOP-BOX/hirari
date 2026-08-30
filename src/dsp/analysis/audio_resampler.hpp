#pragma once

#include <vector>
#include <cmath>
#include <numbers>
#include <algorithm>
#include <limits>

namespace Aura::DSP::Analysis {

/**
 * @brief AudioResampler: High-quality Sample Rate Converter (SRC).
 * Ensures audio consistency when importing assets with different sample rates.
 * Uses Cubic Hermite Spline for production-grade sonic fidelity.
 */
class AudioResampler {
public:
    /**
     * @brief Resamples a source buffer from one rate to another.
     */
    static std::vector<float> process(const std::vector<float>& source, double sourceRate, double targetRate) {
        constexpr std::size_t kMaxOutputSamples = 100'000'000;
        if (source.empty()) return {};
        if (!std::isfinite(sourceRate) || !std::isfinite(targetRate)
            || sourceRate <= 0.0 || targetRate <= 0.0) {
            return {};
        }
        if (std::abs(sourceRate - targetRate) < 0.1) return source; // No conversion needed

        double ratio = sourceRate / targetRate;
        if (!std::isfinite(ratio) || ratio <= 0.0) return {};
        const double targetSizeF = static_cast<double>(source.size()) / ratio;
        if (!std::isfinite(targetSizeF) || targetSizeF <= 0.0
            || targetSizeF > static_cast<double>(kMaxOutputSamples)) return {};
        size_t targetSize = static_cast<size_t>(targetSizeF);
        if (targetSize == 0) return {};
        std::vector<float> output;
        output.resize(targetSize);

        for (size_t i = 0; i < targetSize; ++i) {
            double sourcePos = i * ratio;
            output[i] = interpolateCubic(source, std::min(sourcePos, static_cast<double>(source.size() - 1)));
        }
        return output;
    }

private:
    /**
     * @brief 4-Point Cubic Hermite Interpolation for low aliasing and high clarity.
     */
    static float interpolateCubic(const std::vector<float>& buffer, double pos) {
        if (buffer.empty() || !std::isfinite(pos)) return 0.0f;
        pos = std::clamp(pos, 0.0, static_cast<double>(buffer.size() - 1));
        int i1 = static_cast<int>(pos);
        int i0 = (i1 > 0) ? i1 - 1 : 0;
        int i2 = (i1 < (int)buffer.size() - 1) ? i1 + 1 : (int)buffer.size() - 1;
        int i3 = (i2 < (int)buffer.size() - 1) ? i2 + 1 : i2;

        float f = static_cast<float>(pos - i1);
        float a = (-0.5f * buffer[i0] + 1.5f * buffer[i1] - 1.5f * buffer[i2] + 0.5f * buffer[i3]);
        float b = (buffer[i0] - 2.5f * buffer[i1] + 2.0f * buffer[i2] - 0.5f * buffer[i3]);
        float c = (-0.5f * buffer[i0] + 0.5f * buffer[i2]);
        float d = buffer[i1];

        const float value = a * f * f * f + b * f * f + c * f + d;
        return std::isfinite(value) ? value : 0.0f;
    }
};

} // namespace Aura::DSP::Analysis
