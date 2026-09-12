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
#include "plugin_sandbox_worker_main_start.inc"
#include "plugin_sandbox_worker_main_loop.inc"
