#include "plugin_sandbox_protocol.hpp"
#if defined(HIRARI_ENABLE_VST3_SDK)
#include "vst3_sandbox_adapter.hpp"
#endif

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <vector>

// The worker process itself is Rust (`hirari-plugin-protocol/src/worker_main.rs`):
// argument parsing, process limits, mailbox mapping, adapter selection, the
// readiness handshake, the control loop and teardown all live there. Only the
// VST3 hooks below remain in C++ because the official VST3 interfaces are C++.

extern "C" int hirari_plugin_worker_main(int argc, const char* const* argv);
extern "C" uint8_t hirari_plugin_worker_publish_state(
    const uint8_t* data, uint32_t size, void* sizeOut, void* versionOut, void* checksumOut);
extern "C" uint8_t hirari_plugin_worker_validate_state(
    const uint8_t* data, const void* sizeIn, const void* versionIn, const void* checksumIn);

using Hirari::Core::Plugins::SandboxProtocol::SharedAudioBlock;

// Stable field order shared with `WorkerMailboxView` in worker_main.rs. Keeping
// this as opaque pointers lets Rust address the mailbox without duplicating the
// full `SharedAudioBlock` layout.
struct HirariPluginWorkerMailboxView {
    void* fields[18];
};

extern "C" size_t hirari_plugin_worker_shared_bytes() {
    return sizeof(SharedAudioBlock);
}

extern "C" void hirari_plugin_worker_mailbox_view(void* opaque,
                                                  HirariPluginWorkerMailboxView* out) {
    if (!opaque || !out) return;
    auto* shared = static_cast<SharedAudioBlock*>(opaque);
    out->fields[0] = &shared->protocolVersion;
    out->fields[1] = &shared->activeSampleRate;
    out->fields[2] = &shared->activeChannels;
    out->fields[3] = &shared->midiInstrument;
    out->fields[4] = &shared->outputMidiEvents;
    out->fields[5] = &shared->outputMidiDropped;
    out->fields[6] = shared->outputMidi;
    out->fields[7] = shared->input[0];
    out->fields[8] = shared->output[0];
    out->fields[9] = shared->midi;
    out->fields[10] = &shared->midiEvents;
    out->fields[11] = shared->parameterChange;
    out->fields[12] = &shared->parameterChanges;
    out->fields[13] = &shared->extendedMidi;
    out->fields[14] = shared->state;
    out->fields[15] = &shared->stateSize;
    out->fields[16] = &shared->stateVersion;
    out->fields[17] = &shared->stateChecksum;
}

#if defined(HIRARI_ENABLE_VST3_SDK)

using Hirari::Core::Plugins::SandboxProtocol::kStateErrorNone;
using Hirari::Core::Plugins::SandboxProtocol::kStateErrorOversize;
using Hirari::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
using Hirari::Core::Plugins::SandboxVST3::Runtime;

extern "C" bool hirari_worker_vst3_supported() { return true; }

extern "C" void* hirari_worker_vst3_load(const char* path, double sampleRate,
                                         uint32_t maxFrames, bool* midiInstrument) {
    auto runtime = std::make_unique<Runtime>();
    if (!runtime->load(path, sampleRate, maxFrames)) return nullptr;
    if (midiInstrument) *midiInstrument = runtime->isMidiInstrument();
    return runtime.release();
}

extern "C" void hirari_worker_vst3_destroy(void* instance) {
    delete static_cast<Runtime*>(instance);
}

extern "C" bool hirari_worker_vst3_reset(void* instance, double, uint32_t, uint32_t) {
    auto* runtime = static_cast<Runtime*>(instance);
    return runtime && runtime->reset();
}

extern "C" bool hirari_worker_vst3_process(void* instance, void* opaque, uint32_t channels,
                                           uint32_t frames) {
    auto* runtime = static_cast<Runtime*>(instance);
    if (!runtime || !opaque) return false;
    return runtime->process(*static_cast<SharedAudioBlock*>(opaque), channels, frames);
}

// VST3 state handling needs the shared mailbox and the Rust state envelope, so
// the hook resolves both here and reports the same packed result the loop reads.
extern "C" uint16_t hirari_worker_vst3_state(void* instance, void* opaque, uint8_t mode) {
    auto* runtime = static_cast<Runtime*>(instance);
    auto* shared = static_cast<SharedAudioBlock*>(opaque);
    if (!runtime || !shared) return static_cast<uint16_t>(kStateErrorPlugin) << 8;
    bool stateOk = false;
    uint8_t stateError = kStateErrorPlugin;
    if (mode == 1) {
        std::vector<uint8_t> state;
        stateOk = runtime->saveState(state);
        if (stateOk && state.size() <= sizeof(shared->state)) {
            std::memcpy(shared->state, state.data(), state.size());
            stateError = hirari_plugin_worker_publish_state(
                shared->state, static_cast<uint32_t>(state.size()),
                &shared->stateSize, &shared->stateVersion, &shared->stateChecksum);
            if (stateError != kStateErrorNone) stateOk = false;
        } else if (stateOk) {
            stateOk = false;
            stateError = kStateErrorOversize;
        }
    } else if (mode == 2) {
        const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
        stateError = hirari_plugin_worker_validate_state(
            shared->state, &shared->stateSize, &shared->stateVersion, &shared->stateChecksum);
        if (stateError == kStateErrorNone) {
            stateOk = runtime->loadState(shared->state, size);
            if (!stateOk) stateError = kStateErrorPlugin;
        }
    }
    return static_cast<uint16_t>((static_cast<uint16_t>(stateError) << 8) |
                                 static_cast<uint16_t>(stateOk));
}

#else

// A build without the Steinberg SDK still reports the format as unavailable so
// the launcher fails closed during capability negotiation.
extern "C" bool hirari_worker_vst3_supported() { return false; }
extern "C" void* hirari_worker_vst3_load(const char*, double, uint32_t, bool*) { return nullptr; }
extern "C" void hirari_worker_vst3_destroy(void*) {}
extern "C" bool hirari_worker_vst3_reset(void*, double, uint32_t, uint32_t) { return false; }
extern "C" bool hirari_worker_vst3_process(void*, void*, uint32_t, uint32_t) { return false; }
extern "C" uint16_t hirari_worker_vst3_state(void*, void*, uint8_t) { return 0; }

#endif

// The helper is intentionally a separate executable: plugin loading and
// format-specific ABI code belong here, never in the DAW.
int main(int argc, char** argv) {
    return hirari_plugin_worker_main(argc, const_cast<const char* const*>(argv));
}
