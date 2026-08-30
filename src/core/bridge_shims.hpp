#pragma once
#include <vector>
#include <string>
#include <memory>
#include "audio_engine.hpp"
#include "analysis_hub.hpp"
#include "aura_unified_engine.hpp"
#include "rust/cxx.h"
#include "bridge_types.hpp"

namespace Aura::Core::Bridge {

    struct BridgeEvent;
    struct StructureNode;
    struct BridgeClash;
    struct BridgeLoudness;
    enum class AuraCommand : uint32_t;

    // --- LIFECYCLE ---
    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine();
    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine_offline();
    std::unique_ptr<BridgeFFI::AnalysisHub> new_analysis_hub(const BridgeFFI::AudioEngine& engine);

    const Engine::AuraUnifiedEngine& get_unified_engine(const BridgeFFI::AudioEngine& bridge);

    rust::Slice<const float> get_track_peaks_l(const Engine::AuraUnifiedEngine& engine);
    rust::Slice<const float> get_track_peaks_r(const Engine::AuraUnifiedEngine& engine);
    bool pop_event(const Engine::AuraUnifiedEngine& engine, BridgeEvent& ev);
    
    bool push_command(const BridgeFFI::AudioEngine& engine, AuraCommand cmd, uint32_t tid,
                      float val, uint64_t ts, uint64_t expected_project_generation,
                      uint64_t expected_audio_generation);

    rust::Vec<BridgeClash> get_spectral_clash_v_ffi(const BridgeFFI::AnalysisHub& hub);
    BridgeLoudness get_master_loudness_v_ffi(const BridgeFFI::AnalysisHub& hub);
    rust::String get_song_structure_json_ffi(const BridgeFFI::AnalysisHub& hub);

    void report_aura_log(uint32_t level, rust::Str msg) noexcept;
    void initialize_gpu();
}


namespace Aura::Core::BridgeFFI {
    void AURA_LOG(uint32_t level, const std::string& msg);
}
