#pragma once

#include <cstdint>

// Minimal CLAP 1.x ABI declarations used only inside the sandbox worker.
// This is intentionally not exposed to the DAW or audio thread.
namespace Aura::Core::Plugins::ClapAbi {

struct Version { uint32_t major, minor, revision; };
struct Host;
struct Plugin;
struct OStream;
struct IStream;

enum ProcessStatus : int32_t { Error = 0, Continue = 1, ContinueIfNotQuiet = 2, Tail = 3, Sleep = 4 };

struct AudioBuffer {
    float** data32;
    double** data64;
    uint32_t channel_count;
    uint32_t latency;
    uint64_t constant_mask;
};

struct Process {
    int64_t steady_time;
    uint32_t frames_count;
    const void* transport;
    const AudioBuffer* audio_inputs;
    uint32_t audio_inputs_count;
    AudioBuffer* audio_outputs;
    uint32_t audio_outputs_count;
    const void* in_events;
    void* out_events;
};

struct EventHeader {
    uint32_t size;
    uint32_t time;
    uint16_t space_id;
    uint16_t type;
    uint32_t flags;
};

struct EventMidi {
    EventHeader header;
    uint16_t port_index;
    uint8_t data[3];
};

struct EventMidiSysex {
    EventHeader header;
    uint16_t port_index;
    const uint8_t* buffer;
    uint32_t size;
};

// CLAP_EVENT_MIDI2 carries a 128-bit UMP payload. Keeping the payload as
// four words preserves every bit of high-resolution channel/per-note data.
struct EventMidi2 {
    EventHeader header;
    uint16_t port_index;
    uint16_t reserved;
    uint32_t data[4];
};

// CLAP parameter-value event (CLAP_EVENT_PARAM_VALUE). Keep the layout
// explicit because this header is the worker ABI boundary.
struct EventParamValue {
    EventHeader header;
    uint32_t param_id;
    void* cookie;
    double value;
    int32_t note_id;
    int16_t port_index;
    int16_t channel;
    int16_t key;
};

struct InputEvents {
    const void* ctx;
    uint32_t (*size)(const InputEvents*);
    const EventHeader* (*get)(const InputEvents*, uint32_t index);
};

struct OutputEvents {
    void* ctx;
    bool (*try_push)(const OutputEvents*, const EventHeader* event);
};

inline constexpr uint16_t kCoreEventSpaceId = 0;
inline constexpr uint16_t kEventMidi = 2;
inline constexpr uint16_t kEventMidiSysex = 6;
inline constexpr uint16_t kEventParamValue = 5;
inline constexpr uint16_t kEventMidi2 = 10;

struct Host {
    Version clap_version;
    const char* host_data;
    const char* name;
    const char* vendor;
    const char* url;
    const char* version;
    void (*request_restart)(const Host*);
    void (*request_process)(const Host*);
    void (*request_callback)(const Host*);
    const void* (*get_extension)(const Host*, const char*);
};

struct Descriptor {
    Version clap_version;
    const char* id;
    const char* name;
    const char* vendor;
    const char* url;
    const char* manual_url;
    const char* support_url;
    const char* version;
    const char* const* features;
};

struct Plugin {
    const Descriptor* desc;
    void* plugin_data;
    bool (*init)(const Plugin*);
    void (*destroy)(const Plugin*);
    bool (*activate)(const Plugin*, double, uint32_t, uint32_t);
    void (*deactivate)(const Plugin*);
    bool (*start_processing)(const Plugin*);
    void (*stop_processing)(const Plugin*);
    void (*reset)(const Plugin*);
    ProcessStatus (*process)(const Plugin*, const Process*);
    const void* (*get_extension)(const Plugin*, const char*);
    void (*on_main_thread)(const Plugin*);
};

struct OStream {
    void* ctx;
    int64_t (*write)(const OStream*, const void*, uint64_t);
};

struct IStream {
    void* ctx;
    int64_t (*read)(const IStream*, void*, uint64_t);
};

struct StateExtension {
    bool (*save)(const Plugin*, const OStream*);
    bool (*load)(const Plugin*, const IStream*);
};

inline constexpr const char* kStateExtensionId = "clap.state";

struct Factory {
    uint32_t (*get_plugin_count)(const Factory*);
    const Descriptor* (*get_plugin_descriptor)(const Factory*, uint32_t);
    const Plugin* (*create_plugin)(const Factory*, const Host*, const char*);
};

struct Entry {
    Version clap_version;
    bool (*init)(const char*);
    void (*deinit)();
    const void* (*get_factory)(const char*);
};

inline constexpr const char* kPluginFactoryId = "clap.plugin-factory";
} // namespace Aura::Core::Plugins::ClapAbi
