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
    struct StructureNode;
    struct BridgeClash;
    struct BridgeLoudness;
    enum class HirariCommand : uint32_t;

    // --- LIFECYCLE ---
    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine();
    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine_offline();

    const Engine::HirariUnifiedEngine& get_unified_engine(const BridgeFFI::AudioEngine& bridge);

    rust::Slice<const float> get_track_peaks_l(const Engine::HirariUnifiedEngine& engine);
    rust::Slice<const float> get_track_peaks_r(const Engine::HirariUnifiedEngine& engine);
    rust::Vec<float> get_track_peaks_l_owned(const Engine::HirariUnifiedEngine& engine);
    rust::Vec<float> get_track_peaks_r_owned(const Engine::HirariUnifiedEngine& engine);
    bool pop_event(const Engine::HirariUnifiedEngine& engine, BridgeEvent& ev);

    // Engine-side analysis accessors. Declared here (not on the engine class)
    // so the Rust `ffi` module can call them as free functions; implemented in
    // hirari-core-bridge/src/analysis_bridge_accessors.cpp.
    uint32_t get_active_telemetry_idx(const Engine::HirariUnifiedEngine& engine);
    rust::Vec<float> get_spectrum(const Engine::HirariUnifiedEngine& engine, uint32_t idx);
    BridgeLoudness get_master_meter_data(const Engine::HirariUnifiedEngine& engine);
    rust::String get_intelligence_dashboard_json(const Engine::HirariUnifiedEngine& engine);
    rust::String get_song_structure_json(const Engine::HirariUnifiedEngine& engine);
    
    bool push_command(const BridgeFFI::AudioEngine& engine, HirariCommand cmd, uint32_t tid,
                      float val, uint64_t ts, uint64_t expected_project_generation,
                      uint64_t expected_audio_generation);

    void report_hirari_log(uint32_t level, rust::Str msg) noexcept;
    void initialize_gpu();
    bool initialize_gpu_with_status();
}


namespace Hirari::Core::BridgeFFI {
    void HIRARI_LOG(uint32_t level, const std::string& msg);
}
