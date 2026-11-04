#pragma once
#include <vector>
#include <string>
#include <memory>
#include "audio_engine.hpp"
#include "hirari_unified_engine.hpp"
#include "rust/cxx.h"
#include "bridge_types.hpp"

namespace Hirari::Core::Bridge {

    struct BridgeEvent;
    enum class HirariCommand : uint32_t;

    // --- LIFECYCLE ---
    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine();
    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine_offline();

    const Engine::HirariUnifiedEngine& get_unified_engine(const BridgeFFI::AudioEngine& bridge);

    bool pop_event(const Engine::HirariUnifiedEngine& engine, BridgeEvent& ev);

    bool push_command(const BridgeFFI::AudioEngine& engine, HirariCommand cmd, uint32_t tid,
                      float val, uint64_t ts, uint64_t expected_project_generation,
                      uint64_t expected_audio_generation);

    void report_hirari_log(uint32_t level, rust::Str msg) noexcept;
}

namespace Hirari::Core::BridgeFFI {
    void HIRARI_LOG(uint32_t level, const std::string& msg);
}
