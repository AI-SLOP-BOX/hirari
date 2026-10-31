#include "bridge_shims.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include "dsp/spatial/metal_audio_kernel.hpp"
#include <cstring>

using HirariAudioWeakHandle = std::weak_ptr<::Hirari::Core::AudioBuffer>;

// Import-cache weak handles. Rust stores the opaque pointer and asks for a
// strong lock, liveness, or release; the buffer never crosses the FFI.
extern "C" void* hirari_audio_import_weak_lock(void* weak_audio) {
    auto* weak = static_cast<HirariAudioWeakHandle*>(weak_audio);
    auto locked = weak ? weak->lock() : nullptr;
    return locked ? new std::shared_ptr<::Hirari::Core::AudioBuffer>(std::move(locked)) : nullptr;
}
extern "C" bool hirari_audio_import_weak_alive(void* weak_audio) {
    auto* weak = static_cast<HirariAudioWeakHandle*>(weak_audio);
    return weak && !weak->expired();
}
extern "C" void hirari_audio_import_weak_release(void* weak_audio) {
    delete static_cast<HirariAudioWeakHandle*>(weak_audio);
}

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

    rust::Vec<float> get_track_peaks_l_owned(const Engine::HirariUnifiedEngine& engine) {
        rust::Vec<float> result;
        for (int attempt = 0; attempt < 4; ++attempt) {
            Engine::HirariUnifiedEngine::TelemetryData snapshot{};
            if (!engine.copy_telemetry(engine.get_active_telemetry_idx(), snapshot)) continue;
            result.clear();
            result.reserve(snapshot.count);
            for (uint32_t i = 0; i < snapshot.count; ++i) result.push_back(snapshot.peaksL[i]);
            return result;
        }
        return result;
    }

    rust::Vec<float> get_track_peaks_r_owned(const Engine::HirariUnifiedEngine& engine) {
        rust::Vec<float> result;
        for (int attempt = 0; attempt < 4; ++attempt) {
            Engine::HirariUnifiedEngine::TelemetryData snapshot{};
            if (!engine.copy_telemetry(engine.get_active_telemetry_idx(), snapshot)) continue;
            result.clear();
            result.reserve(snapshot.count);
            for (uint32_t i = 0; i < snapshot.count; ++i) result.push_back(snapshot.peaksR[i]);
            return result;
        }
        return result;
    }

    bool pop_event(const Engine::HirariUnifiedEngine& engine, BridgeEvent& ev) {
        ::Hirari::Core::EngineEvent nativeEv;
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

    bool push_command(const BridgeFFI::AudioEngine& engine, HirariCommand cmd, uint32_t tid,
                      float val, uint64_t ts, uint64_t expected_project_generation,
                      uint64_t expected_audio_generation) {
        // INDUSTRIAL: Branchless Boundary Validation
        uint32_t mask = (tid < 1024) ? 0xFFFFFFFF : 0;
        if (!mask) return false;
        return engine.push_command(static_cast<::Hirari::Core::CommandType>(cmd), tid, val, ts,
                                   expected_project_generation, expected_audio_generation);
    }

    bool initialize_gpu_with_status() {
        return ::Hirari::DSP::Spatial::MetalAudioKernel::getInstance().initialize();
    }

} // namespace Hirari::Core::Bridge
