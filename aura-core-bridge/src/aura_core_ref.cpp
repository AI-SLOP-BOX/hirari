#include "aura-core-bridge/src/lib.rs.h"
#include <string>
#include <iostream>

namespace Aura::Core::BridgeFFI {
 
void AURA_LOG(uint32_t level, const std::string& msg) {
    ::Aura::Core::Bridge::report_aura_log(level, rust::Str(msg));
}

} // namespace Aura::Core::BridgeFFI

#include "dsp/spatial/metal_audio_kernel.hpp"

namespace Aura::Core::Bridge {

void initialize_gpu() {
    // Explicitly seed the GPU kernel before the first block arrives
    ::Aura::DSP::Spatial::MetalAudioKernel::getInstance().initialize();
}

} // namespace Aura::Core::Bridge
