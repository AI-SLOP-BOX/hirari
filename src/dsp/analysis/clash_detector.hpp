#pragma once

#include <cstddef>
#include <vector>

#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Analysis {

/** C++ value adapter for the Rust spectral overlap analysis. */
class MixClashDetector {
public:
    enum class AdviceCode { None, SidechainKickBass, NotchTrackB, ClarityOK };

    struct ClashInfo {
        float maskingIndex;
        float centerFreq;
        AdviceCode advice = AdviceCode::None;
    };

    static std::vector<ClashInfo> detectClashes(
        const float* specA, const float* specB, size_t size, double sampleRate = 44100.0) {
        std::vector<ClashInfo> results;
        const auto info = MixClashDetector{}.detect(specA, specB, size, sampleRate);
        if (info.maskingIndex > 0.65f) results.push_back(info);
        return results;
    }

    ClashInfo detect(const float* spectrumA, const float* spectrumB,
                     size_t size, double sampleRate) const {
        ClashInfo result{};
        uint8_t advice = 0;
        if (!hirari_mix_clash_detect(spectrumA, spectrumB, size, sampleRate,
                                     &result.maskingIndex, &result.centerFreq, &advice)) {
            return {};
        }
        result.advice = static_cast<AdviceCode>(advice);
        return result;
    }
};

} // namespace Hirari::DSP::Analysis
