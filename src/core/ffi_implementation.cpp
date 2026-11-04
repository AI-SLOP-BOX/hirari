#include "bridge_shims.hpp"
#include "engine/track.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include <cstring>

using HirariAudioWeakHandle = std::weak_ptr<void>;
using HirariAudioStrongHandle = Hirari::Core::Engine::SharedAudioStorage;

// Import-cache weak handles. Rust stores the opaque pointer and asks for a
// strong lock, liveness, or release; the buffer never crosses the FFI.
extern "C" void* hirari_audio_import_weak_lock(void* weak_audio) {
    auto* weak = static_cast<HirariAudioWeakHandle*>(weak_audio);
    auto locked = weak ? weak->lock() : nullptr;
    return locked ? new HirariAudioStrongHandle(std::move(locked)) : nullptr;
}
extern "C" bool hirari_audio_import_weak_alive(void* weak_audio) {
    auto* weak = static_cast<HirariAudioWeakHandle*>(weak_audio);
    return weak && !weak->expired();
}
extern "C" void hirari_audio_import_weak_release(void* weak_audio) {
    delete static_cast<HirariAudioWeakHandle*>(weak_audio);
}

// Import buffers are built by Rust: the handle is the Rust-owned storage
// itself, so these helpers only move its lifetime across the boundary.
// `out_storage` receives the storage Rust resizes and fills.
extern "C" void* hirari_audio_import_buffer_create(void** out_storage) {
    auto storage = Hirari::Core::Engine::shareAudioStorage(
        hirari_audio_buffer_storage_create());
    if (out_storage) *out_storage = storage.get();
    return new HirariAudioStrongHandle(std::move(storage));
}
extern "C" void* hirari_audio_import_buffer_storage(void* strong_audio) {
    auto* audio = static_cast<HirariAudioStrongHandle*>(strong_audio);
    return audio ? audio->get() : nullptr;
}
extern "C" void* hirari_audio_import_buffer_weak(void* strong_audio) {
    auto* audio = static_cast<HirariAudioStrongHandle*>(strong_audio);
    return audio && *audio ? new HirariAudioWeakHandle(*audio) : nullptr;
}
extern "C" void hirari_audio_import_buffer_release(void* strong_audio) {
    delete static_cast<HirariAudioStrongHandle*>(strong_audio);
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

} // namespace Hirari::Core::Bridge
