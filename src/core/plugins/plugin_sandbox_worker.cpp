#include "plugin_sandbox_protocol.hpp"

#include <cstddef>
#include <cstdint>

// The worker process itself is Rust (`hirari-plugin-protocol/src/worker_main.rs`):
// argument parsing, process limits, mailbox mapping, adapter selection, the
// readiness handshake, the control loop and teardown all live there. This
// translation unit only exposes the `SharedAudioBlock` layout, whose field
// order both sides must agree on, plus the VST3 hooks the launcher links.

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

// VST3 hosting is not implemented in this worker, so the format stays
// unavailable and the launcher fails closed during capability negotiation.
extern "C" bool hirari_worker_vst3_supported() { return false; }
extern "C" void* hirari_worker_vst3_load(const char*, double, uint32_t, bool*) { return nullptr; }
extern "C" void hirari_worker_vst3_destroy(void*) {}
extern "C" bool hirari_worker_vst3_reset(void*, double, uint32_t, uint32_t) { return false; }
extern "C" bool hirari_worker_vst3_process(void*, void*, uint32_t, uint32_t) { return false; }
extern "C" uint16_t hirari_worker_vst3_state(void*, void*, uint8_t) { return 0; }

// The helper is intentionally a separate executable: plugin loading never
// happens in the DAW process, so a crashing third-party binary cannot take the
// sequencer down with it.
int main(int argc, char** argv) {
    return hirari_plugin_worker_main(argc, const_cast<const char* const*>(argv));
}
