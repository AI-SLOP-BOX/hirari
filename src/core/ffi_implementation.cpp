#include "bridge_shims.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include <cstring>

namespace Hirari::Core::Bridge {

    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine() {
        return std::make_unique<BridgeFFI::AudioEngine>();
    }

    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine_offline() {
        return std::make_unique<BridgeFFI::AudioEngine>(false);
    }

    const Engine::HirariUnifiedEngine& get_unified_engine(const BridgeFFI::AudioEngine& bridge) {
        return bridge.get_core();
    }

    bool push_command(const BridgeFFI::AudioEngine& engine, HirariCommand cmd, uint32_t tid,
                      float val, uint64_t ts, uint64_t expected_project_generation,
                      uint64_t expected_audio_generation) {
        // INDUSTRIAL: Branchless Boundary Validation
        uint32_t mask = (tid < 1024) ? 0xFFFFFFFF : 0;
        if (!mask) return false;
        return engine.push_command(static_cast<::Hirari::Core::CommandType>(cmd), tid, val, ts,
                                   expected_project_generation, expected_audio_generation);
    }

} // namespace Hirari::Core::Bridge
