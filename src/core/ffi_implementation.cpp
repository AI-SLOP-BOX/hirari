#include "bridge_shims.hpp"
#include "aura-core-bridge/src/lib.rs.h"
#include "dsp/spatial/metal_audio_kernel.hpp"
#include <cstring>

namespace Aura::Core::Bridge {

    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine() {
        return std::make_unique<BridgeFFI::AudioEngine>();
    }

    std::unique_ptr<BridgeFFI::AudioEngine> new_audio_engine_offline() {
        return std::make_unique<BridgeFFI::AudioEngine>(false);
    }

    std::unique_ptr<BridgeFFI::AnalysisHub> new_analysis_hub(const BridgeFFI::AudioEngine& engine) {
        return std::make_unique<BridgeFFI::AnalysisHub>(engine.get_core_shared());
    }

    const Engine::AuraUnifiedEngine& get_unified_engine(const BridgeFFI::AudioEngine& bridge) {
        return bridge.get_core();
    }

    rust::Slice<const float> get_track_peaks_l(const Engine::AuraUnifiedEngine& engine) {
        return engine.get_track_peaks_l();
    }
    rust::Slice<const float> get_track_peaks_r(const Engine::AuraUnifiedEngine& engine) {
        return engine.get_track_peaks_r();
    }

    rust::Vec<float> get_track_peaks_l_owned(const Engine::AuraUnifiedEngine& engine) {
        rust::Vec<float> result;
        for (int attempt = 0; attempt < 4; ++attempt) {
            Engine::AuraUnifiedEngine::TelemetryData snapshot{};
            if (!engine.copy_telemetry(engine.get_active_telemetry_idx(), snapshot)) continue;
            result.clear();
            result.reserve(snapshot.count);
            for (uint32_t i = 0; i < snapshot.count; ++i) result.push_back(snapshot.peaksL[i]);
            return result;
        }
        return result;
    }

    rust::Vec<float> get_track_peaks_r_owned(const Engine::AuraUnifiedEngine& engine) {
        rust::Vec<float> result;
        for (int attempt = 0; attempt < 4; ++attempt) {
            Engine::AuraUnifiedEngine::TelemetryData snapshot{};
            if (!engine.copy_telemetry(engine.get_active_telemetry_idx(), snapshot)) continue;
            result.clear();
            result.reserve(snapshot.count);
            for (uint32_t i = 0; i < snapshot.count; ++i) result.push_back(snapshot.peaksR[i]);
            return result;
        }
        return result;
    }

    bool pop_event(const Engine::AuraUnifiedEngine& engine, BridgeEvent& ev) {
        ::Aura::Core::EngineEvent nativeEv;
        if (engine.pop_event(nativeEv)) {
            ev.timestamp = nativeEv.timestamp;
            ev.event_type = nativeEv.type;
            ev.track_id = nativeEv.trackId;
            ev.value = nativeEv.value;
            std::memcpy(ev.label.data(), nativeEv.label, sizeof(ev.label));
            ev.label[sizeof(ev.label) - 1] = 0;
            return true;
        }
        return false;
    }

    bool push_command(const BridgeFFI::AudioEngine& engine, AuraCommand cmd, uint32_t tid,
                      float val, uint64_t ts, uint64_t expected_project_generation,
                      uint64_t expected_audio_generation) {
        // INDUSTRIAL: Branchless Boundary Validation
        uint32_t mask = (tid < 1024) ? 0xFFFFFFFF : 0;
        if (!mask) return false;
        return engine.push_command(static_cast<::Aura::Core::CommandType>(cmd), tid, val, ts,
                                   expected_project_generation, expected_audio_generation);
    }

    rust::Vec<BridgeClash> get_spectral_clash_v_ffi(const BridgeFFI::AnalysisHub& hub) {
        auto clashes = hub.get_spectral_clash_v();
        rust::Vec<BridgeClash> result;
        for (const auto& c : clashes) {
            result.push_back({static_cast<float>(c.bin), c.intensity});
        }
        return result;
    }

    BridgeLoudness get_master_loudness_v_ffi(const BridgeFFI::AnalysisHub& hub) {
        auto n = hub.get_master_loudness_v();
        return {n.integrated, n.short_term, n.true_peak_l, n.true_peak_r, n.correlation};
    }

    rust::String get_song_structure_json_ffi(const BridgeFFI::AnalysisHub& hub) {
        return hub.get_song_structure_json();
    }

    rust::Vec<float> get_mixer_levels_v(const BridgeFFI::AnalysisHub& hub) {
        return hub.get_mixer_levels_v();
    }
    rust::Vec<float> get_spectral_data_v(const BridgeFFI::AnalysisHub& hub) {
        return hub.get_spectral_data_v();
    }

    bool initialize_gpu_with_status() {
        return ::Aura::DSP::Spatial::MetalAudioKernel::getInstance().initialize();
    }

} // namespace Aura::Core::Bridge
