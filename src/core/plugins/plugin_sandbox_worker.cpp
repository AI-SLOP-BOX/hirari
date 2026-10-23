#include "plugin_sandbox_protocol.hpp"
#if defined(__APPLE__)
#include "au_sandbox_adapter.hpp"
#endif
#if defined(HIRARI_ENABLE_VST3_SDK)
#include "vst3_sandbox_adapter.hpp"
#endif

#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <unistd.h>
#include <string>
#include <algorithm>
#if !defined(_WIN32)
#include <dlfcn.h>
#endif

struct HirariPluginWorkerBackend {
    bool (*reset)(void*);
    uint16_t (*state)(void*, uint8_t);
    bool (*process)(void*, void*, uint32_t, uint32_t);
};
struct HirariPluginWorkerFaultConfig {
    uint64_t crashAfterBlocks;
    uint32_t delayMilliseconds;
};
struct HirariPluginWorkerLaunchOptions {
    int32_t controlFd;
    int32_t statusFd;
    int32_t sharedFd;
    const char* pluginPath;
    const char* sharedName;
    uint32_t requestedFormat;
    double sampleRate;
    uint32_t minFrames;
    uint32_t maxFrames;
    uint32_t channels;
    uint64_t crashAfterBlocks;
    uint32_t delayMilliseconds;
    uint8_t pathIsClap;
    uint8_t pathIsVst3;
    uint8_t pathIsAudioUnit;
    uint8_t builtinPassthrough;
};
extern "C" bool hirari_plugin_worker_parse_args(
    int32_t argc, const char* const* argv,
    HirariPluginWorkerLaunchOptions* output, uint32_t* errorKind);
extern "C" int32_t hirari_plugin_worker_run(
    int32_t controlFd, void* shared, void* backendContext,
    const HirariPluginWorkerBackend* backend,
    const HirariPluginWorkerFaultConfig* faults);
extern "C" void* hirari_clap_input_context_create(
    void* outputCount, void* droppedCount, void* outputEvents);
extern "C" void hirari_clap_input_context_destroy(void* context);
extern "C" bool hirari_clap_process_block(
    const void* plugin, void* context, float* inputAudio, float* outputAudio,
    uint32_t channels, uint32_t frames, const void* midiEvents,
    uint32_t midiCount, void* extendedRing, const void* parameterChanges,
    uint32_t parameterCount);
extern "C" uint16_t hirari_clap_state_block(
    const void* plugin, const void* extension, void* context, uint8_t mode,
    uint8_t* state, const void* stateSize, const void* stateVersion,
    const void* stateChecksum, double sampleRate, uint32_t minFrames,
    uint32_t maxFrames);
extern "C" int32_t hirari_clap_instance_load(
    const char* imagePath, const char* packagePath, double sampleRate,
    uint32_t minFrames, uint32_t maxFrames, void** instance, bool* midiInstrument);
extern "C" const void* hirari_clap_instance_plugin(const void* instance);
extern "C" const void* hirari_clap_instance_state(const void* instance);
extern "C" void hirari_clap_instance_destroy(void* instance);
extern "C" bool hirari_clap_instance_reset(
    void* instance, double sampleRate, uint32_t minFrames, uint32_t maxFrames);
extern "C" char* hirari_plugin_resolve_image_path(const char* path);
extern "C" void hirari_plugin_resolved_image_free(char* path);
extern "C" bool hirari_plugin_worker_apply_limits();
extern "C" void* hirari_plugin_worker_map_shared(
    int32_t fd, const char* name, size_t size, uint32_t* errorKind);
extern "C" void hirari_plugin_worker_unmap_shared(void* shared, size_t size);
extern "C" uint8_t hirari_plugin_worker_publish_state(
    const uint8_t* data, uint32_t size, void* sizeOut, void* versionOut, void* checksumOut);
extern "C" uint8_t hirari_plugin_worker_validate_state(
    const uint8_t* data, const void* sizeIn, const void* versionIn, const void* checksumIn);

bool debugEnabled() noexcept { return std::getenv("HIRARI_PLUGIN_DEBUG") != nullptr; }
void debugLog(const char* message) noexcept {
    if (debugEnabled()) std::fprintf(stderr, "[hirari-plugin-worker] %s\n", message);
}

// AU/VST3/CLAP packages are directories on macOS.  dlopen() cannot load the
// package directory itself; it must receive the Mach-O binary in
// Contents/MacOS.  Keep the package path for CLAP's entry->init callback, but
// resolve the actual image only for the dynamic loader.
std::string resolveMacPluginImage(const char* pluginPath) {
    char* resolved = hirari_plugin_resolve_image_path(pluginPath);
    if (!resolved) return {};
    std::string result(resolved);
    hirari_plugin_resolved_image_free(resolved);
    return result;
}
// The helper is intentionally a separate executable: plugin loading and
// format-specific ABI code belong here, never in the DAW. CLAP, AUv2, and,
// when built with the official Steinberg SDK, VST3 instances use the shared
// mailbox for real audio processing. A VST3 build without that SDK fails
// closed during capability negotiation rather than pretending to process.
#include "plugin_sandbox_worker_main_start.inc"
#include "plugin_sandbox_worker_main_loop.inc"
