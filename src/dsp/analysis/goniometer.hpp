#pragma once

#include <array>
#include <cstddef>

namespace Hirari::DSP::Analysis {

/**
 * Transfer shape for the Rust-owned stereo scope. The engine reads correlation,
 * balance and the history window through `hirari_master_meter_goniometer`;
 * there is no C++-owned goniometer instance.
 */
class Goniometer {
public:
    static constexpr size_t kHistorySize = 1024;
    struct Data {
        float correlation = 1.0f;
        float balance = 0.0f;
        std::array<float, kHistorySize> xyHistoryL{};
        std::array<float, kHistorySize> xyHistoryR{};
    };
};

} // namespace Hirari::DSP::Analysis
