// VST3 sandbox adapter entry points for the Rust worker.
//
// This translation unit is only compiled when the build script detects the
// official Steinberg VST3 SDK (HIRARI_VST3_SDK); without it the Rust worker
// fails VST3 requests closed and the shim does not exist. The ABI mirrors
// the CLAP/AU instance adapters (hirari_<fmt>_instance_{load,destroy,reset},
// hirari_<fmt>_process_block, hirari_<fmt>_state_block) and the Rust worker
// calls these symbols directly.

#include "vst3_adapter.hpp"

#include <cstdint>
#include <cstring>

extern "C" {
// Rust-side mailbox state envelope (worker_runtime.rs): validates the
// version/checksum contract and publishes stateSize/stateVersion/
// stateChecksum on success.
uint8_t hirari_plugin_worker_publish_state(
    const uint8_t* state, uint32_t size, uint32_t* state_size, uint32_t* state_version,
    uint64_t* state_checksum);
uint8_t hirari_plugin_worker_validate_state(
    const uint8_t* state, const uint32_t* state_size, const uint32_t* state_version,
    const uint64_t* state_checksum);
}

extern "C" bool hirari_worker_vst3_supported() { return true; }

extern "C" void* hirari_worker_vst3_load(const char* path, double sampleRate,
                                         uint32_t maxFrames, bool* midiInstrument) {
    auto runtime = std::make_unique<Hirari::Core::Plugins::SandboxVST3::Runtime>();
    if (!runtime->load(path, sampleRate, maxFrames)) return nullptr;
    if (midiInstrument) *midiInstrument = runtime->isMidiInstrument();
    return runtime.release();
}

extern "C" uint32_t hirari_worker_vst3_latency(void* instance) {
    auto* runtime = static_cast<Hirari::Core::Plugins::SandboxVST3::Runtime*>(instance);
    return runtime ? runtime->latencySamples() : 0;
}

extern "C" void hirari_worker_vst3_destroy(void* instance) {
    delete static_cast<Hirari::Core::Plugins::SandboxVST3::Runtime*>(instance);
}

extern "C" bool hirari_worker_vst3_reset(void* instance, double, uint32_t, uint32_t) {
    auto* runtime = static_cast<Hirari::Core::Plugins::SandboxVST3::Runtime*>(instance);
    return runtime && runtime->reset();
}

extern "C" bool hirari_worker_vst3_process(void* instance, void* opaque, uint32_t channels,
                                           uint32_t frames) {
    auto* runtime = static_cast<Hirari::Core::Plugins::SandboxVST3::Runtime*>(instance);
    if (!runtime || !opaque) return false;
    return runtime->process(*static_cast<Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock*>(
                                opaque),
                            channels, frames);
}

extern "C" uint16_t hirari_worker_vst3_state(void* instance, void* opaque, uint8_t mode) {
    using Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone;
    using Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
    using Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
    auto* runtime = static_cast<Hirari::Core::Plugins::SandboxVST3::Runtime*>(instance);
    auto* shared = static_cast<Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock*>(opaque);
    if (!runtime || !shared) return static_cast<uint16_t>(kStateErrorPlugin) << 8;
    bool stateOk = false;
    uint8_t stateError = kStateErrorPlugin;
    if (mode == 1) {
        std::vector<uint8_t> state;
        stateOk = runtime->saveState(state);
        if (stateOk && state.size() <= sizeof(shared->state)) {
            std::memcpy(shared->state, state.data(), state.size());
            uint32_t state_size = 0;
            uint32_t state_version = 0;
            uint64_t state_checksum = 0;
            stateError = hirari_plugin_worker_publish_state(
                shared->state, static_cast<uint32_t>(state.size()),
                &state_size, &state_version, &state_checksum);
            shared->stateSize.store(state_size, std::memory_order_release);
            shared->stateVersion.store(state_version, std::memory_order_release);
            shared->stateChecksum.store(state_checksum, std::memory_order_release);
            if (stateError != kStateErrorNone) stateOk = false;
        } else if (stateOk) {
            stateOk = false;
            stateError = kStateErrorOversize;
        }
    } else if (mode == 2) {
        const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
        const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
        const uint64_t checksum = shared->stateChecksum.load(std::memory_order_acquire);
        stateError = hirari_plugin_worker_validate_state(
            shared->state, &size, &version, &checksum);
        if (stateError == kStateErrorNone) {
            stateOk = runtime->loadState(shared->state, size);
            if (!stateOk) stateError = kStateErrorPlugin;
        }
    }
    return static_cast<uint16_t>((static_cast<uint16_t>(stateError) << 8) |
                                 static_cast<uint16_t>(stateOk));
}
