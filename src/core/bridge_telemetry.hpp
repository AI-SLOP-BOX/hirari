#pragma once

namespace Aura::Core::Bridge {

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

} // namespace Aura::Core::Bridge
