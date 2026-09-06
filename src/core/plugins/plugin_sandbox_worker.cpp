#include "plugin_sandbox_protocol.hpp"
#include "clap_abi_minimal.hpp"
#if defined(__APPLE__)
#include "au_sandbox_adapter.hpp"
#endif
#if defined(AURA_ENABLE_VST3_SDK)
#include "vst3_sandbox_adapter.hpp"
#endif

#include <cerrno>
#include <cstdint>
#include <charconv>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <cctype>
#include <poll.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/resource.h>
#include <dirent.h>
#include <string>
#include <chrono>
#include <thread>
#include <algorithm>
#include <cmath>
#if !defined(_WIN32)
#include <dlfcn.h>
#endif

namespace {
bool parseInt(const char* text, int& value) noexcept {
    if (!text || *text == '\0') return false;
    const char* end = text + std::strlen(text);
    auto result = std::from_chars(text, end, value, 10);
    return result.ec == std::errc{} && result.ptr == end;
}

bool parseUint32(const char* text, uint32_t& value) noexcept {
    if (!text || *text == '\0') return false;
    const char* end = text + std::strlen(text);
    auto result = std::from_chars(text, end, value, 10);
    return result.ec == std::errc{} && result.ptr == end;
}

bool parseUint64(const char* text, uint64_t& value) noexcept {
    if (!text || *text == '\0') return false;
    const char* end = text + std::strlen(text);
    auto result = std::from_chars(text, end, value, 10);
    return result.ec == std::errc{} && result.ptr == end;
}

bool parseDouble(const char* text, double& value) noexcept {
    if (!text || *text == '\0') return false;
    char* end = nullptr;
    errno = 0;
    value = std::strtod(text, &end);
    return errno != ERANGE && end != text && end && *end == '\0' && std::isfinite(value);
}

bool hasPluginExtension(const char* path, const char* extension) noexcept {
    if (!path || !extension) return false;
    const char* dot = std::strrchr(path, '.');
    if (!dot) return false;
    for (size_t i = 0; extension[i] != '\0'; ++i) {
        if (dot[i] == '\0' || static_cast<char>(std::tolower(static_cast<unsigned char>(dot[i]))) != extension[i])
            return false;
    }
    return dot[std::strlen(extension)] == '\0';
}
}
bool debugEnabled() noexcept { return std::getenv("AURA_PLUGIN_DEBUG") != nullptr; }
void debugLog(const char* message) noexcept {
    if (debugEnabled()) std::fprintf(stderr, "[aura-plugin-worker] %s\n", message);
}

bool clapDescriptorHasFeature(
    const Aura::Core::Plugins::ClapAbi::Descriptor* descriptor,
    const char* requestedFeature) noexcept {
    if (!descriptor || !requestedFeature || !descriptor->features) return false;
    for (const char* const* feature = descriptor->features; *feature; ++feature) {
        if (std::strcmp(*feature, requestedFeature) == 0) return true;
    }
    return false;
}
struct MidiInputContext {
    const Aura::Core::Plugins::SandboxProtocol::MidiEvent* events = nullptr;
    uint32_t count = 0;
    std::array<Aura::Core::Plugins::MidiExtendedMessageRing::Message,
               Aura::Core::Plugins::MidiExtendedMessageRing::kCapacity> extended{};
    uint32_t extendedCount = 0;
    std::array<Aura::Core::Plugins::ClapAbi::EventParamValue,
               Aura::Core::Plugins::SandboxProtocol::kMaxParameterChanges> parameters{};
    uint32_t parameterCount = 0;
};

struct MidiOutputContext {
    Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock* shared = nullptr;
    uint32_t frames = 0;
};

bool midiOutputTryPush(const Aura::Core::Plugins::ClapAbi::OutputEvents* output,
                       const Aura::Core::Plugins::ClapAbi::EventHeader* header) {
    auto* context = static_cast<MidiOutputContext*>(output ? output->ctx : nullptr);
    if (!context || !context->shared || !header ||
        header->space_id != Aura::Core::Plugins::ClapAbi::kCoreEventSpaceId)
        return false;
    if (header->time >= context->frames) return false;
    uint8_t data[Aura::Core::Plugins::SandboxProtocol::kMaxMidiPayloadBytes]{};
    uint32_t dataSize = 0;
    if (header->type == Aura::Core::Plugins::ClapAbi::kEventMidi) {
        if (header->size < sizeof(Aura::Core::Plugins::ClapAbi::EventMidi)) return false;
        Aura::Core::Plugins::ClapAbi::EventMidi midi{};
        std::memcpy(&midi, header, sizeof(midi));
        std::memcpy(data, midi.data, sizeof(midi.data));
        dataSize = sizeof(midi.data);
    } else if (header->type == Aura::Core::Plugins::ClapAbi::kEventMidiSysex) {
        if (header->size < sizeof(Aura::Core::Plugins::ClapAbi::EventMidiSysex)) return false;
        Aura::Core::Plugins::ClapAbi::EventMidiSysex sysex{};
        std::memcpy(&sysex, header, sizeof(sysex));
        if (sysex.buffer == nullptr || sysex.size == 0 ||
            sysex.size > sizeof(data)) {
            context->shared->outputMidiDropped.fetch_add(1, std::memory_order_relaxed);
            return false;
        }
        std::memcpy(data, sysex.buffer, sysex.size);
        dataSize = sysex.size;
    } else if (header->type == Aura::Core::Plugins::ClapAbi::kEventMidi2) {
        if (header->size < sizeof(Aura::Core::Plugins::ClapAbi::EventMidi2)) return false;
        Aura::Core::Plugins::ClapAbi::EventMidi2 midi2{};
        std::memcpy(&midi2, header, sizeof(midi2));
        std::memcpy(data, midi2.data, sizeof(midi2.data));
        dataSize = sizeof(midi2.data);
    } else {
        return false;
    }
    const uint32_t index = context->shared->outputMidiEvents.load(std::memory_order_relaxed);
    if (index >= Aura::Core::Plugins::SandboxProtocol::kMaxMidiEvents) {
        context->shared->outputMidiDropped.fetch_add(1, std::memory_order_relaxed);
        return false;
    }
    auto& destination = context->shared->outputMidi[index];
    destination.sampleOffset = header->time;
    destination.size = dataSize;
    destination.articulationId = 0;
    std::memcpy(destination.data, data, dataSize);
    context->shared->outputMidiEvents.store(index + 1, std::memory_order_release);
    return true;
}

uint32_t midiInputSize(const Aura::Core::Plugins::ClapAbi::InputEvents* input) {
    const auto* context = static_cast<const MidiInputContext*>(input ? input->ctx : nullptr);
    return context ? context->count + context->extendedCount + context->parameterCount : 0;
}

const Aura::Core::Plugins::ClapAbi::EventHeader* midiInputGet(
    const Aura::Core::Plugins::ClapAbi::InputEvents* input, uint32_t index) {
    const auto* context = static_cast<const MidiInputContext*>(input ? input->ctx : nullptr);
    if (!context || index >= context->count + context->extendedCount + context->parameterCount) return nullptr;
    if (index >= context->count + context->extendedCount) {
        const auto& parameter = context->parameters[
            index - context->count - context->extendedCount];
        return &parameter.header;
    }
    if (index >= context->count) {
        const auto& extended = context->extended[index - context->count];
        if (extended.size == 0 || extended.size > UINT32_MAX ||
            extended.sampleOffset > UINT32_MAX) return nullptr;
        static thread_local Aura::Core::Plugins::ClapAbi::EventMidiSysex convertedSysex{};
        convertedSysex.header.size = sizeof(convertedSysex);
        convertedSysex.header.time = static_cast<uint32_t>(extended.sampleOffset);
        convertedSysex.header.space_id = Aura::Core::Plugins::ClapAbi::kCoreEventSpaceId;
        convertedSysex.header.type = Aura::Core::Plugins::ClapAbi::kEventMidiSysex;
        convertedSysex.header.flags = 0;
        convertedSysex.port_index = 0;
        convertedSysex.buffer = extended.data.data();
        convertedSysex.size = extended.size;
        return &convertedSysex.header;
    }
    const auto* event = &context->events[index];
    if (event->size < 3 || event->sampleOffset > UINT32_MAX) return nullptr;
    // A MIDI 2.0 UMP is carried losslessly in the fixed 16-byte payload.
    // Expose it as CLAP_EVENT_MIDI2 instead of rejecting it as a non-legacy
    // two/three-byte MIDI message.
    if (event->size == sizeof(Aura::Core::Plugins::ClapAbi::EventMidi2::data)) {
        static thread_local Aura::Core::Plugins::ClapAbi::EventMidi2 convertedMidi2{};
        convertedMidi2.header.size = sizeof(convertedMidi2);
        convertedMidi2.header.time = static_cast<uint32_t>(event->sampleOffset);
        convertedMidi2.header.space_id = Aura::Core::Plugins::ClapAbi::kCoreEventSpaceId;
        convertedMidi2.header.type = Aura::Core::Plugins::ClapAbi::kEventMidi2;
        convertedMidi2.header.flags = 0;
        convertedMidi2.port_index = 0;
        convertedMidi2.reserved = 0;
        std::memcpy(convertedMidi2.data, event->data, sizeof(convertedMidi2.data));
        return &convertedMidi2.header;
    }
    static thread_local Aura::Core::Plugins::ClapAbi::EventMidi converted{};
    converted.header.size = sizeof(converted);
    converted.header.time = static_cast<uint32_t>(event->sampleOffset);
    converted.header.space_id = Aura::Core::Plugins::ClapAbi::kCoreEventSpaceId;
    converted.header.type = Aura::Core::Plugins::ClapAbi::kEventMidi;
    converted.header.flags = 0;
    converted.port_index = 0;
    std::memcpy(converted.data, event->data, sizeof(converted.data));
    return &converted.header;
}

struct StateStreamContext {
    uint8_t* data = nullptr;
    uint32_t size = 0;
    uint32_t cursor = 0;
    uint32_t capacity = 0;
    bool writing = false;
};

int64_t writeState(const Aura::Core::Plugins::ClapAbi::OStream* stream,
                   const void* data, uint64_t size) {
    auto* context = static_cast<StateStreamContext*>(stream ? stream->ctx : nullptr);
    if (!context || !context->writing || (!data && size) ||
        context->cursor > context->capacity || size > context->capacity - context->cursor)
        return -1;
    if (size) std::memcpy(context->data + context->cursor, data, static_cast<size_t>(size));
    context->cursor += static_cast<uint32_t>(size);
    context->size = context->cursor;
    return static_cast<int64_t>(size);
}

int64_t readState(const Aura::Core::Plugins::ClapAbi::IStream* stream,
                  void* data, uint64_t size) {
    auto* context = static_cast<StateStreamContext*>(stream ? stream->ctx : nullptr);
    if (!context || context->writing || (!data && size) ||
        context->cursor > context->size || size > context->size - context->cursor)
        return -1;
    if (size) std::memcpy(data, context->data + context->cursor, static_cast<size_t>(size));
    context->cursor += static_cast<uint32_t>(size);
    return static_cast<int64_t>(size);
}

void noopRestart(const Aura::Core::Plugins::ClapAbi::Host*) {}
void noopProcess(const Aura::Core::Plugins::ClapAbi::Host*) {}
void noopCallback(const Aura::Core::Plugins::ClapAbi::Host*) {}
const void* noExtension(const Aura::Core::Plugins::ClapAbi::Host*, const char*) { return nullptr; }

bool applyWorkerLimits() noexcept {
    struct rlimit coreLimit{0, 0};
    (void)::setrlimit(RLIMIT_CORE, &coreLimit);

    struct rlimit currentFiles{};
    if (::getrlimit(RLIMIT_NOFILE, &currentFiles) == 0 && currentFiles.rlim_cur > 64) {
        struct rlimit fileLimit{64, currentFiles.rlim_max};
        (void)::setrlimit(RLIMIT_NOFILE, &fileLimit);
    }

#if defined(RLIMIT_AS)
    // Keep a malformed plugin from exhausting the DAW host machine. The
    // limit is intentionally generous for normal audio plugins.
    struct rlimit currentMemory{};
    if (::getrlimit(RLIMIT_AS, &currentMemory) == 0 && currentMemory.rlim_cur > static_cast<rlim_t>(2ull * 1024ull * 1024ull * 1024ull)) {
        struct rlimit memoryLimit{static_cast<rlim_t>(2ull * 1024ull * 1024ull * 1024ull), currentMemory.rlim_max};
        (void)::setrlimit(RLIMIT_AS, &memoryLimit);
    }
#endif
    return true;
}

// AU/VST3/CLAP packages are directories on macOS.  dlopen() cannot load the
// package directory itself; it must receive the Mach-O binary in
// Contents/MacOS.  Keep the package path for CLAP's entry->init callback, but
// resolve the actual image only for the dynamic loader.
std::string resolveMacPluginImage(const char* pluginPath) {
    if (!pluginPath || *pluginPath == '\0') return {};
    struct stat pathStat{};
    if (::stat(pluginPath, &pathStat) != 0 || !S_ISDIR(pathStat.st_mode)) {
        return pluginPath;
    }

    std::string contents = std::string(pluginPath) + "/Contents/MacOS";
    DIR* directory = ::opendir(contents.c_str());
    if (!directory) return {};

    std::string selected;
    while (const dirent* entry = ::readdir(directory)) {
        if (std::strcmp(entry->d_name, ".") == 0 || std::strcmp(entry->d_name, "..") == 0)
            continue;
        const std::string candidate = contents + "/" + entry->d_name;
        struct stat candidateStat{};
        if (::stat(candidate.c_str(), &candidateStat) == 0 && S_ISREG(candidateStat.st_mode) &&
            ::access(candidate.c_str(), X_OK) == 0) {
            selected = candidate;
            break;
        }
    }
    ::closedir(directory);
    return selected;
}
// The helper is intentionally a separate executable: plugin loading and
// format-specific ABI code belong here, never in the DAW. CLAP, AUv2, and,
// when built with the official Steinberg SDK, VST3 instances use the shared
// mailbox for real audio processing. A VST3 build without that SDK fails
// closed during capability negotiation rather than pretending to process.
int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "--capabilities") == 0) {
        const char* capabilities =
#if defined(AURA_ENABLE_VST3_SDK)
            "{\"clap\":true,\"au\":true,\"vst3\":true}\n";
#elif defined(__APPLE__)
            "{\"clap\":true,\"au\":true,\"vst3\":false}\n";
#else
            "{\"clap\":true,\"au\":false,\"vst3\":false}\n";
#endif
        (void)::write(STDOUT_FILENO, capabilities, std::strlen(capabilities));
        return 0;
    }
    int controlFd = -1;
    int statusFd = -1;
    int sharedFd = -1;
    const char* pluginPath = nullptr;
    const char* sharedName = nullptr;
    double sampleRate = 44100.0;
    uint32_t minFrames = 1;
    uint32_t maxFrames = Aura::Core::Plugins::SandboxProtocol::kMaxFrames;
    uint32_t channels = Aura::Core::Plugins::SandboxProtocol::kMaxChannels;
    const char* requestedFormat = "auto";
    for (int i = 1; i < argc; ++i) {
        const char* option = argv[i];
        if (i + 1 >= argc) {
            debugLog("missing command-line option value");
            return 2;
        }
        const char* value = argv[++i];
        if (std::strcmp(option, "--control-fd") == 0) {
            if (!parseInt(value, controlFd)) return 2;
        } else if (std::strcmp(option, "--status-fd") == 0) {
            if (!parseInt(value, statusFd)) return 2;
        } else if (std::strcmp(option, "--shared-fd") == 0) {
            if (!parseInt(value, sharedFd)) return 2;
        } else if (std::strcmp(option, "--shared-name") == 0) {
            sharedName = value;
        } else if (std::strcmp(option, "--plugin") == 0) {
            pluginPath = value;
        } else if (std::strcmp(option, "--format") == 0) {
            requestedFormat = value;
        } else if (std::strcmp(option, "--sample-rate") == 0) {
            if (!parseDouble(value, sampleRate)) return 2;
        } else if (std::strcmp(option, "--min-frames") == 0) {
            if (!parseUint32(value, minFrames)) return 2;
        } else if (std::strcmp(option, "--max-frames") == 0) {
            if (!parseUint32(value, maxFrames)) return 2;
        } else if (std::strcmp(option, "--channels") == 0) {
            if (!parseUint32(value, channels)) return 2;
        } else {
            debugLog("unknown command-line option");
            return 2;
        }
    }
    if (controlFd < 0 || statusFd < 0 || (sharedFd < 0 && (!sharedName || *sharedName == '\0')) ||
        pluginPath == nullptr || *pluginPath == '\0') {
        debugLog("required worker argument is invalid");
        return 2;
    }
    if (!requestedFormat ||
        (std::strcmp(requestedFormat, "auto") != 0 &&
         std::strcmp(requestedFormat, "builtin") != 0 &&
         std::strcmp(requestedFormat, "clap") != 0 &&
         std::strcmp(requestedFormat, "vst3") != 0 &&
         std::strcmp(requestedFormat, "au") != 0)) {
        debugLog("requested plugin format is invalid");
        return 2;
    }
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || sampleRate > 384000.0 ||
        minFrames == 0 || minFrames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames ||
        maxFrames < minFrames || maxFrames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames ||
        channels == 0 || channels > Aura::Core::Plugins::SandboxProtocol::kMaxChannels) {
        debugLog("requested audio configuration is invalid");
        return 2;
    }

    if (!applyWorkerLimits()) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorSecurity;
        (void)::write(statusFd, &error, sizeof(error));
        return 11;
    }

    struct stat sharedStat{};
    bool sharedValid = ::fstat(sharedFd, &sharedStat) == 0 &&
        static_cast<size_t>(sharedStat.st_size) >= sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock);
    if (!sharedValid && sharedName) {
        const int namedFd = ::shm_open(sharedName, O_RDWR, 0);
        if (namedFd >= 0) {
            sharedFd = namedFd;
            sharedValid = ::fstat(sharedFd, &sharedStat) == 0 &&
                static_cast<size_t>(sharedStat.st_size) >= sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock);
        }
    }
    if (!sharedValid ||
        static_cast<size_t>(sharedStat.st_size) < sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock)) {
        debugLog("shared audio block descriptor and name are unavailable");
        return 4;
    }
    auto* shared = static_cast<Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock*>(
        ::mmap(nullptr, sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock),
               PROT_READ | PROT_WRITE, MAP_SHARED, sharedFd, 0));
    if (shared == MAP_FAILED) {
        debugLog("shared audio block mapping failed");
        return 5;
    }
    if (shared->protocolVersion.load(std::memory_order_acquire) !=
        Aura::Core::Plugins::SandboxProtocol::kAudioBlockProtocolVersion) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorAbi;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(Aura::Core::Plugins::SandboxProtocol::SharedAudioBlock));
        return 12;
    }
    shared->activeSampleRate.store(static_cast<uint32_t>(sampleRate), std::memory_order_release);
    shared->activeChannels.store(channels, std::memory_order_release);

    void* pluginHandle = nullptr;
    const Aura::Core::Plugins::ClapAbi::Entry* clapEntry = nullptr;
    const Aura::Core::Plugins::ClapAbi::Plugin* clapPlugin = nullptr;
    bool clapStarted = false;
#if defined(__APPLE__)
    Aura::Core::Plugins::SandboxAU::Runtime auRuntime;
    bool auStarted = false;
#endif
#if defined(AURA_ENABLE_VST3_SDK)
    Aura::Core::Plugins::SandboxVST3::Runtime vst3Runtime;
    bool vst3Started = false;
#endif
    const Aura::Core::Plugins::ClapAbi::StateExtension* clapState = nullptr;
    const bool testFaultsEnabled = std::getenv("AURA_PLUGIN_TEST_FAULTS") != nullptr;
    uint64_t processedBlocks = 0;
    uint64_t crashAfterBlocks = 0;
    uint32_t delayMilliseconds = 0;
    if (testFaultsEnabled) {
        if (const char* value = std::getenv("AURA_PLUGIN_WORKER_CRASH_AFTER_BLOCKS")) {
            uint64_t parsed = 0;
            if (parseUint64(value, parsed)) crashAfterBlocks = parsed;
        }
        if (const char* value = std::getenv("AURA_PLUGIN_WORKER_DELAY_MS")) {
            uint32_t parsed = 0;
            if (parseUint32(value, parsed)) delayMilliseconds = parsed;
        }
    }
#if !defined(_WIN32)
    const bool builtinPassthrough = std::strncmp(pluginPath, "builtin://", 10) == 0;
    const bool pathIsClap = hasPluginExtension(pluginPath, ".clap");
    const bool pathIsVst3 = hasPluginExtension(pluginPath, ".vst3");
    const bool pathIsAudioUnit = hasPluginExtension(pluginPath, ".component");
    const bool isClap = std::strcmp(requestedFormat, "clap") == 0 ? pathIsClap :
                        (std::strcmp(requestedFormat, "auto") == 0 && pathIsClap);
    const bool isVst3 = std::strcmp(requestedFormat, "vst3") == 0 ? pathIsVst3 :
                        (std::strcmp(requestedFormat, "auto") == 0 && pathIsVst3);
    const bool isAudioUnit = std::strcmp(requestedFormat, "au") == 0 ? pathIsAudioUnit :
                             (std::strcmp(requestedFormat, "auto") == 0 && pathIsAudioUnit);
    const bool requestedBuiltin = std::strcmp(requestedFormat, "builtin") == 0;
    if ((requestedFormat[0] == 'c' && !pathIsClap) ||
        (requestedFormat[0] == 'v' && !pathIsVst3) ||
        (requestedFormat[0] == 'a' && !pathIsAudioUnit) ||
        (requestedBuiltin && !builtinPassthrough)) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 12;
    }
    if (isVst3) {
#if defined(AURA_ENABLE_VST3_SDK)
        if (!vst3Runtime.load(pluginPath, sampleRate, maxFrames)) {
            debugLog(vst3Runtime.error());
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            ::munmap(shared, sizeof(*shared));
            return 10;
        }
        vst3Started = true;
#else
        // VST3 requires the official ABI interfaces. Do not acknowledge
        // readiness until an SDK adapter is present.
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 12;
#endif
    }
    if (isAudioUnit) {
#if defined(__APPLE__)
        if (!auRuntime.load(pluginPath, sampleRate, maxFrames, channels)) {
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            ::munmap(shared, sizeof(*shared));
            return 10;
        }
        auStarted = true;
#else
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorUnsupported;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 12;
#endif
    }
    const bool vst3SdkLoaded = isVst3 &&
#if defined(AURA_ENABLE_VST3_SDK)
        true;
#else
        false;
#endif
    const std::string pluginImage = builtinPassthrough || isAudioUnit || vst3SdkLoaded
        ? std::string{} : resolveMacPluginImage(pluginPath);
    debugLog(pluginImage.empty() ? "plugin image empty" : pluginImage.c_str());
    debugLog("resolving plugin image");
    pluginHandle = builtinPassthrough || isAudioUnit || vst3SdkLoaded ? reinterpret_cast<void*>(1) :
        (pluginImage.empty() ? nullptr : ::dlopen(pluginImage.c_str(), RTLD_NOW | RTLD_LOCAL));
    if (pluginHandle == nullptr) {
        debugLog("dlopen failed");
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorLoad;
        (void)::write(statusFd, &error, sizeof(error));
        ::munmap(shared, sizeof(*shared));
        return 6;
    }
    debugLog("plugin image ready");
    const char* symbol = isClap ? "clap_entry" : (isVst3 ? "GetPluginFactory" : nullptr);
    if (!builtinPassthrough && !isAudioUnit && !vst3SdkLoaded &&
        (symbol == nullptr || ::dlsym(pluginHandle, symbol) == nullptr)) {
        const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorAbi;
        (void)::write(statusFd, &error, sizeof(error));
        if (!builtinPassthrough && !isAudioUnit) ::dlclose(pluginHandle);
        ::munmap(shared, sizeof(*shared));
        return 7;
    }
    if (isClap && !builtinPassthrough) {
        debugLog("initializing CLAP entry");
        // CLAP exports `clap_entry` as a const entry object, not a function.
        // Calling the symbol as a function is undefined behaviour.
        clapEntry = reinterpret_cast<const Aura::Core::Plugins::ClapAbi::Entry*>(
            ::dlsym(pluginHandle, "clap_entry"));
        if (!clapEntry || !clapEntry->init || !clapEntry->deinit || !clapEntry->get_factory ||
            clapEntry->clap_version.major != 1 || !clapEntry->init(pluginPath)) {
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorAbi;
            (void)::write(statusFd, &error, sizeof(error));
            ::dlclose(pluginHandle);
            ::munmap(shared, sizeof(*shared));
            return 8;
        }
        const auto* factory = static_cast<const Aura::Core::Plugins::ClapAbi::Factory*>(
            clapEntry->get_factory(Aura::Core::Plugins::ClapAbi::kPluginFactoryId));
        debugLog("factory acquired");
        if (!factory || !factory->get_plugin_count || !factory->get_plugin_descriptor ||
            !factory->create_plugin || factory->get_plugin_count(factory) == 0) {
            clapEntry->deinit();
            ::dlclose(pluginHandle);
            ::munmap(shared, sizeof(*shared));
            return 9;
        }
        uint32_t descriptorIndex = 0;
        const char* requestedFeature = std::getenv("AURA_PLUGIN_KIND");
        if (requestedFeature && *requestedFeature) {
            const uint32_t count = factory->get_plugin_count(factory);
            bool found = false;
            for (uint32_t candidate = 0; candidate < count; ++candidate) {
                const auto* candidateDescriptor =
                    factory->get_plugin_descriptor(factory, candidate);
                if (clapDescriptorHasFeature(candidateDescriptor, requestedFeature)) {
                    descriptorIndex = candidate;
                    found = true;
                    break;
                }
            }
            if (!found) {
                debugLog("requested CLAP feature was not found");
                clapEntry->deinit();
                ::dlclose(pluginHandle);
                ::munmap(shared, sizeof(*shared));
                const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
                (void)::write(statusFd, &error, sizeof(error));
                return 10;
            }
        }
        const auto* descriptor = factory->get_plugin_descriptor(factory, descriptorIndex);
        Aura::Core::Plugins::ClapAbi::Host host{
            {1, 0, 0}, "aura-sandbox", "Aura Studio", "Aura", "", "1",
            &noopRestart, &noopProcess, &noopCallback, &noExtension};
        clapPlugin = descriptor ? factory->create_plugin(factory, &host, descriptor->id) : nullptr;
        debugLog("plugin instance created");
        const bool validPlugin = clapPlugin && clapPlugin->init && clapPlugin->destroy &&
            clapPlugin->activate && clapPlugin->deactivate && clapPlugin->start_processing &&
            clapPlugin->stop_processing && clapPlugin->process;
        if (!validPlugin) debugLog("plugin vtable incomplete");
        const bool initialized = validPlugin && clapPlugin->init(clapPlugin);
        debugLog(initialized ? "plugin init ok" : "plugin init failed");
        const bool activated = initialized && clapPlugin->activate(
            clapPlugin, sampleRate, minFrames, maxFrames);
        debugLog(activated ? "plugin activate ok" : "plugin activate failed");
        const bool processing = activated && clapPlugin->start_processing(clapPlugin);
        debugLog(processing ? "plugin start_processing ok" : "plugin start_processing failed");
        if (!validPlugin || !initialized || !activated || !processing) {
            if (clapPlugin && clapPlugin->destroy) clapPlugin->destroy(clapPlugin);
            clapEntry->deinit();
            ::dlclose(pluginHandle);
            ::munmap(shared, sizeof(*shared));
            const uint8_t error = Aura::Core::Plugins::SandboxProtocol::kErrorInstance;
            (void)::write(statusFd, &error, sizeof(error));
            return 10;
        }
        debugLog("plugin activated and processing started");
        clapStarted = true;
        clapState = static_cast<const Aura::Core::Plugins::ClapAbi::StateExtension*>(
            clapPlugin->get_extension ? clapPlugin->get_extension(clapPlugin, Aura::Core::Plugins::ClapAbi::kStateExtensionId) : nullptr);
    }
#else
    (void)pluginPath;
#endif

    const uint8_t ready = Aura::Core::Plugins::SandboxProtocol::kReady;
    debugLog("sending ready");
    if (::write(statusFd, &ready, sizeof(ready)) != 1) {
#if !defined(_WIN32)
        if (clapStarted) { clapPlugin->stop_processing(clapPlugin); clapPlugin->deactivate(clapPlugin); clapPlugin->destroy(clapPlugin); clapEntry->deinit(); }
        if (!builtinPassthrough && !isAudioUnit) ::dlclose(pluginHandle);
#endif
        ::munmap(shared, sizeof(*shared));
        return 3;
    }

    for (;;) {
        struct pollfd descriptor{controlFd, POLLIN | POLLHUP | POLLERR, 0};
        const int result = ::poll(&descriptor, 1, 1);
        if (result < 0 && errno == EINTR) continue;
        if (result > 0 && (descriptor.revents & (POLLHUP | POLLERR))) break;
        if (result > 0 && (descriptor.revents & POLLIN)) {
            uint8_t command = 0;
            const ssize_t bytes = ::read(controlFd, &command, sizeof(command));
            if (bytes != 1 || command == Aura::Core::Plugins::SandboxProtocol::kShutdown) break;
            if (command == Aura::Core::Plugins::SandboxProtocol::kReset) {
                bool resetOk = true;
                if (clapStarted && clapPlugin) {
                    clapPlugin->stop_processing(clapPlugin);
                    clapPlugin->deactivate(clapPlugin);
                    resetOk = clapPlugin->activate(clapPlugin, sampleRate, minFrames, maxFrames);
                    resetOk = resetOk && clapPlugin->start_processing(clapPlugin);
                }
#if defined(AURA_ENABLE_VST3_SDK)
                if (vst3Started) resetOk = vst3Runtime.reset() && resetOk;
#endif
#if defined(__APPLE__)
                if (auStarted) resetOk = auRuntime.reset() && resetOk;
#endif
                if (!resetOk) shared->processErrors.fetch_add(1, std::memory_order_release);
            }
        }
        shared->heartbeat.fetch_add(1, std::memory_order_relaxed);
        const uint64_t stateRequest = shared->stateRequestSequence.load(std::memory_order_acquire);
        const uint64_t stateCompleted = shared->stateCompletedSequence.load(std::memory_order_relaxed);
        if (stateRequest != stateCompleted) {
            const uint8_t mode = shared->stateMode.load(std::memory_order_relaxed);
            bool stateOk = false;
            uint8_t stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
            bool clapSuspendedForState = false;
            if (clapState && clapPlugin) {
                // CLAP state is a main-thread/control-plane operation.  A
                // number of production plugins assume the instance is
                // inactive while loading state and can deadlock when load is
                // called concurrently with process().  Keep save active
                // (some instruments only expose their current state while
                // active), but suspend the instance for load and restore the
                // lifecycle before acknowledging the request.
                if (mode == 2) {
                    clapPlugin->stop_processing(clapPlugin);
                    clapPlugin->deactivate(clapPlugin);
                    clapSuspendedForState = true;
                }
                StateStreamContext context;
                context.data = shared->state;
                context.capacity = Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes;
                if (mode == 1 && clapState->save) {
                    context.writing = true;
                    const Aura::Core::Plugins::ClapAbi::OStream stream{&context, &writeState};
                    stateOk = clapState->save(clapPlugin, &stream);
                    if (stateOk) {
                        if (context.size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                            stateOk = false;
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                        } else {
                            shared->stateSize.store(context.size, std::memory_order_release);
                            shared->stateVersion.store(
                                Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                                std::memory_order_release);
                            shared->stateChecksum.store(
                                Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, context.size),
                                std::memory_order_release);
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorNone;
                        }
                    }
                } else if (mode == 2 && clapState->load) {
                    context.writing = false;
                    context.size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (context.size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(version)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorVersion;
                    } else if (shared->stateChecksum.load(std::memory_order_acquire) !=
                               Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, context.size)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorChecksum;
                    } else {
                        const Aura::Core::Plugins::ClapAbi::IStream stream{&context, &readState};
                        stateOk = clapState->load(clapPlugin, &stream);
                        stateError = stateOk ? Aura::Core::Plugins::SandboxProtocol::kStateErrorNone
                                             : Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#if defined(AURA_ENABLE_VST3_SDK)
            else if (vst3Started) {
                if (mode == 1) {
                    std::vector<uint8_t> state;
                    stateOk = vst3Runtime.saveState(state);
                    if (stateOk) {
                        if (state.size() > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                            stateOk = false;
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                        } else {
                            std::memcpy(shared->state, state.data(), state.size());
                            shared->stateSize.store(static_cast<uint32_t>(state.size()), std::memory_order_release);
                            shared->stateVersion.store(
                                Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                                std::memory_order_release);
                            shared->stateChecksum.store(
                                Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, state.size()),
                                std::memory_order_release);
                            stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorNone;
                        }
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(version)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorVersion;
                    } else if (shared->stateChecksum.load(std::memory_order_acquire) !=
                               Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, size)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorChecksum;
                    } else {
                        stateOk = vst3Runtime.loadState(shared->state, size);
                        stateError = stateOk ? Aura::Core::Plugins::SandboxProtocol::kStateErrorNone
                                             : Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#endif
#if defined(__APPLE__)
            else if (auStarted) {
                if (mode == 1) {
                    uint32_t size = 0;
                    stateOk = auRuntime.saveState(
                        shared->state, Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes, size);
                    if (stateOk) {
                        shared->stateSize.store(size, std::memory_order_release);
                        shared->stateVersion.store(
                            Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion,
                            std::memory_order_release);
                        shared->stateChecksum.store(
                            Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, size),
                            std::memory_order_release);
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorNone;
                    }
                } else if (mode == 2) {
                    const uint32_t size = shared->stateSize.load(std::memory_order_acquire);
                    const uint32_t version = shared->stateVersion.load(std::memory_order_acquire);
                    if (size > Aura::Core::Plugins::SandboxProtocol::kMaxStateBytes) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorOversize;
                    } else if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(version)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorVersion;
                    } else if (shared->stateChecksum.load(std::memory_order_acquire) !=
                               Aura::Core::Plugins::SandboxProtocol::stateChecksum(shared->state, size)) {
                        stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorChecksum;
                    } else {
                        stateOk = auRuntime.loadState(shared->state, size);
                        stateError = stateOk ? Aura::Core::Plugins::SandboxProtocol::kStateErrorNone
                                             : Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                    }
                }
            }
#endif
            if (clapSuspendedForState) {
                const bool reactivated = clapPlugin->activate(
                    clapPlugin, sampleRate, minFrames, maxFrames);
                const bool restartedProcessing = reactivated &&
                    clapPlugin->start_processing(clapPlugin);
                if (!reactivated || !restartedProcessing) {
                    stateOk = false;
                    stateError = Aura::Core::Plugins::SandboxProtocol::kStateErrorPlugin;
                }
            }
            if (!stateOk && mode == 1) {
                shared->stateSize.store(0, std::memory_order_release);
                shared->stateVersion.store(0, std::memory_order_release);
                shared->stateChecksum.store(0, std::memory_order_release);
            }
            shared->stateError.store(stateError, std::memory_order_release);
            shared->stateResult.store(stateOk ? 1 : 0, std::memory_order_release);
            shared->stateCompletedSequence.store(stateRequest, std::memory_order_release);
        }
        const uint64_t request = shared->requestSequence.load(std::memory_order_acquire);
        const uint64_t completed = shared->completedSequence.load(std::memory_order_relaxed);
        if (request != 0 && request != completed) {
            ++processedBlocks;
            if (crashAfterBlocks != 0 && processedBlocks >= crashAfterBlocks) {
                // Test-only fault injection. The explicit opt-in prevents a
                // project or plugin environment variable from crashing a
                // production worker accidentally.
                ::_exit(90);
            }
            if (delayMilliseconds != 0)
                std::this_thread::sleep_for(std::chrono::milliseconds(delayMilliseconds));
            const uint32_t channels = shared->channels.load(std::memory_order_relaxed);
            const uint32_t frames = shared->frames.load(std::memory_order_relaxed);
            if (channels == 0 || channels > Aura::Core::Plugins::SandboxProtocol::kMaxChannels ||
                frames == 0 || frames > Aura::Core::Plugins::SandboxProtocol::kMaxFrames) break;
            for (uint32_t channel = 0; channel < channels; ++channel) {
                std::memcpy(shared->output[channel], shared->input[channel],
                            static_cast<size_t>(frames) * sizeof(float));
            }
            if (clapStarted) {
                float* inputChannels[Aura::Core::Plugins::SandboxProtocol::kMaxChannels]{};
                float* outputChannels[Aura::Core::Plugins::SandboxProtocol::kMaxChannels]{};
                for (uint32_t channel = 0; channel < channels; ++channel) {
                    inputChannels[channel] = shared->input[channel];
                    outputChannels[channel] = shared->output[channel];
                }
                Aura::Core::Plugins::ClapAbi::AudioBuffer input{
                    inputChannels, nullptr, channels, 0, 0};
                Aura::Core::Plugins::ClapAbi::AudioBuffer output{
                    outputChannels, nullptr, channels, 0, 0};
                const uint32_t midiEvents = std::min<uint32_t>(
                    shared->midiEvents.load(std::memory_order_acquire),
                    Aura::Core::Plugins::SandboxProtocol::kMaxMidiEvents);
                MidiInputContext midiContext{shared->midi, midiEvents};
                while (midiContext.extendedCount < midiContext.extended.size() &&
                       shared->extendedMidi.pop(midiContext.extended[midiContext.extendedCount])) {
                    ++midiContext.extendedCount;
                }
                midiContext.parameterCount = std::min<uint32_t>(
                    shared->parameterChanges.load(std::memory_order_acquire),
                    Aura::Core::Plugins::SandboxProtocol::kMaxParameterChanges);
                for (uint32_t parameterIndex = 0;
                     parameterIndex < midiContext.parameterCount; ++parameterIndex) {
                    const auto& source = shared->parameterChange[parameterIndex];
                    auto& destination = midiContext.parameters[parameterIndex];
                    destination.header.size = sizeof(destination);
                    destination.header.time = std::min<uint32_t>(source.sampleOffset, frames);
                    destination.header.space_id = Aura::Core::Plugins::ClapAbi::kCoreEventSpaceId;
                    destination.header.type = Aura::Core::Plugins::ClapAbi::kEventParamValue;
                    destination.header.flags = 0;
                    destination.param_id = source.parameterId;
                    destination.cookie = nullptr;
                    destination.value = source.value;
                    destination.note_id = -1;
                    destination.port_index = 0;
                    destination.channel = -1;
                    destination.key = -1;
                }
                const Aura::Core::Plugins::ClapAbi::InputEvents inputEvents{
                    &midiContext, &midiInputSize, &midiInputGet};
                MidiOutputContext midiOutputContext{shared, frames};
                const Aura::Core::Plugins::ClapAbi::OutputEvents outputEvents{
                    &midiOutputContext, &midiOutputTryPush};
                Aura::Core::Plugins::ClapAbi::Process process{
                    0, frames, nullptr, &input, 1, &output, 1, &inputEvents,
                    const_cast<Aura::Core::Plugins::ClapAbi::OutputEvents*>(&outputEvents)};
                const auto processStatus = clapPlugin->process(clapPlugin, &process);
                if (processStatus == Aura::Core::Plugins::ClapAbi::Error) {
                    std::memset(shared->output, 0, sizeof(shared->output));
                    shared->processErrors.fetch_add(1, std::memory_order_release);
                }
#if defined(__APPLE__)
            } else if (auStarted && !auRuntime.process(*shared, channels, frames)) {
                std::memset(shared->output, 0, sizeof(shared->output));
                shared->processErrors.fetch_add(1, std::memory_order_release);
#endif
#if defined(AURA_ENABLE_VST3_SDK)
            } else if (vst3Started) {
                const bool processed = vst3Runtime.process(*shared, channels, frames);
                if (!processed) {
                    std::memset(shared->output, 0, sizeof(shared->output));
                    shared->processErrors.fetch_add(1, std::memory_order_release);
                }
#endif
            }
            // Parameter changes belong to this exact mailbox block. Clear
            // them only after the selected backend consumed the block, so a
            // slow worker cannot reapply an old change on the next request.
            shared->parameterChanges.store(0, std::memory_order_release);
            shared->completedSequence.store(request, std::memory_order_release);
        }
    }
#if !defined(_WIN32)
    if (clapStarted) { clapPlugin->stop_processing(clapPlugin); clapPlugin->deactivate(clapPlugin); clapPlugin->destroy(clapPlugin); clapEntry->deinit(); }
    if (pluginHandle != reinterpret_cast<void*>(1)) ::dlclose(pluginHandle);
#endif
    ::munmap(shared, sizeof(*shared));
    return 0;
}
