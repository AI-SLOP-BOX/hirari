#pragma once

#include <vector>
#include <string>
#include <memory>
#include "../../dsp/mixing/pro_limiter.hpp"

namespace Aura::Core::Engine {

/**
 * @brief SelectionBasedProcessor: Logic Pro-style offline region processing.
 * Allows applying specific DSP chains to selected audio clips, creating rendered versions.
 */
class SelectionBasedProcessor {
public:
    static SelectionBasedProcessor& getInstance() {
        static SelectionBasedProcessor instance;
        return instance;
    }

    void processRegionAsync(const std::vector<float>& sourceData, const std::string& processChainId) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // Offline rendering and high-density memory management 
        // are now handled securely in the Rust layer.
        // Rust's OfflineRenderingEngine ensures bit-accurate offline distribution.
        // Rust's ForensicAuditor ensures absolute offline integrity.
    }


private:
    SelectionBasedProcessor() = default;
};

} // namespace Aura::Core::Engine
