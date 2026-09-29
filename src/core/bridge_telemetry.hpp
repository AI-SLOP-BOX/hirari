#pragma once

namespace Hirari::Core::Bridge {

struct BridgeClash {
    float frequency;
    float severity;
};

struct BridgeLoudness {
    float integrated;
    float short_term;
    float true_peak_l;
    float true_peak_r;
    float correlation;
};

} // namespace Hirari::Core::Bridge
