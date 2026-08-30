#include "src/core/plugins/clap_abi_minimal.hpp"

#include <cstdlib>
#include <cstring>
#include <chrono>
#include <thread>
#include <new>

using namespace Aura::Core::Plugins::ClapAbi;

namespace {
struct FixtureState {
    float gain = 0.5f;
    double sampleRate = 0.0;
    uint32_t minFrames = 0;
    uint32_t maxFrames = 0;
};

FixtureState& stateFor(const Plugin* plugin) {
    return *static_cast<FixtureState*>(plugin->plugin_data);
}

bool init(const Plugin*) { return true; }
void destroy(const Plugin* plugin) {
    if (!plugin) return;
    delete static_cast<FixtureState*>(plugin->plugin_data);
    delete plugin;
}
bool activate(const Plugin* plugin, double sampleRate, uint32_t minFrames, uint32_t maxFrames) {
    auto& state = stateFor(plugin);
    state.sampleRate = sampleRate;
    state.minFrames = minFrames;
    state.maxFrames = maxFrames;
    return sampleRate > 0.0 && minFrames > 0 && maxFrames >= minFrames;
}
void deactivate(const Plugin*) {}
bool startProcessing(const Plugin*) { return true; }
void stopProcessing(const Plugin*) {}
void reset(const Plugin*) {}

ProcessStatus process(const Plugin* plugin, const Process* request) {
    auto& state = stateFor(plugin);
    if (const char* crash = std::getenv("AURA_CLAP_FIXTURE_CRASH");
        crash && std::strcmp(crash, "1") == 0) {
        std::_Exit(86);
    }
    if (const char* delay = std::getenv("AURA_CLAP_FIXTURE_DELAY_MS");
        delay && *delay != '\0') {
        const long milliseconds = std::strtol(delay, nullptr, 10);
        if (milliseconds > 0 && milliseconds <= 1000) {
            std::this_thread::sleep_for(std::chrono::milliseconds(milliseconds));
        }
    }
    if (!request || !request->audio_inputs || !request->audio_outputs ||
        request->audio_inputs_count == 0 || request->audio_outputs_count == 0 ||
        request->frames_count == 0) {
        return Error;
    }

    // A received extended MIDI payload changes the gain for this block. This
    // makes the sandbox SysEx path observable without requiring a second
    // fixture-specific output event ABI.
    const auto* inputEvents = static_cast<const InputEvents*>(request->in_events);
    if (inputEvents && inputEvents->size && inputEvents->get) {
        for (uint32_t index = 0; index < inputEvents->size(inputEvents); ++index) {
            const auto* header = inputEvents->get(inputEvents, index);
            if (header && header->type == kEventMidiSysex &&
                header->size >= sizeof(EventMidiSysex)) {
                const auto* sysex = reinterpret_cast<const EventMidiSysex*>(header);
                if (sysex->buffer && sysex->size > 256) {
                    state.gain = 0.25f;
                }
            }
        }
    }
    const auto& input = request->audio_inputs[0];
    auto& output = request->audio_outputs[0];
    if (!input.data32 || !output.data32 || input.channel_count == 0 || output.channel_count == 0) {
        return Error;
    }
    const uint32_t channels = input.channel_count < output.channel_count
        ? input.channel_count : output.channel_count;
    for (uint32_t channel = 0; channel < channels; ++channel) {
        if (!input.data32[channel] || !output.data32[channel]) return Error;
        for (uint32_t frame = 0; frame < request->frames_count; ++frame)
            output.data32[channel][frame] = input.data32[channel][frame] * state.gain;
    }

    // Echo MIDI events so the sandbox integration test proves that the event
    // path reaches the plugin and returns through shared memory.
    const auto* outputEvents = static_cast<const OutputEvents*>(request->out_events);
    if (inputEvents && outputEvents && inputEvents->size && inputEvents->get && outputEvents->try_push) {
        for (uint32_t index = 0; index < inputEvents->size(inputEvents); ++index) {
            const auto* header = inputEvents->get(inputEvents, index);
            if (header && header->type == kEventMidi && header->size >= sizeof(EventMidi)) {
                const auto* inputMidi = reinterpret_cast<const EventMidi*>(header);
                EventMidi echoed{};
                echoed.header.size = sizeof(EventMidi);
                echoed.header.time = header->time;
                echoed.header.space_id = kCoreEventSpaceId;
                echoed.header.type = kEventMidi;
                echoed.port_index = inputMidi->port_index;
                std::memcpy(echoed.data, inputMidi->data, sizeof(echoed.data));
                outputEvents->try_push(outputEvents, &echoed.header);
            }
        }
    }
    return Continue;
}

bool saveState(const Plugin* plugin, const OStream* stream) {
    const auto& state = stateFor(plugin);
    return stream && stream->write && stream->write(stream, &state.gain, sizeof(state.gain)) == sizeof(state.gain);
}

bool loadState(const Plugin* plugin, const IStream* stream) {
    auto& state = stateFor(plugin);
    if (const char* delay = std::getenv("AURA_CLAP_FIXTURE_STATE_DELAY_MS");
        delay && *delay != '\0') {
        const long milliseconds = std::strtol(delay, nullptr, 10);
        if (milliseconds > 0 && milliseconds <= 10000)
            std::this_thread::sleep_for(std::chrono::milliseconds(milliseconds));
    }
    float gain = 0.0f;
    if (!stream || !stream->read || stream->read(stream, &gain, sizeof(gain)) != sizeof(gain)) return false;
    if (!(gain >= 0.0f && gain <= 2.0f)) return false;
    state.gain = gain;
    return true;
}

const StateExtension stateExtension{&saveState, &loadState};
const Descriptor descriptor{{1, 0, 0}, "com.aura.test.minimal-gain", "Aura Test Gain",
                           "Aura Tests", "", "", "", "1.0", nullptr};

const Plugin* createPlugin(const Factory*, const Host*, const char*) {
    auto* state = new (std::nothrow) FixtureState{};
    auto* plugin = state ? new (std::nothrow) Plugin{
        &descriptor, state, &init, &destroy, &activate, &deactivate,
        &startProcessing, &stopProcessing, &reset, &process,
        [](const Plugin*, const char* id) -> const void* {
            return id && std::strcmp(id, kStateExtensionId) == 0 ? &stateExtension : nullptr;
        }, nullptr} : nullptr;
    if (!plugin) delete state;
    return plugin;
}

uint32_t pluginCount(const Factory*) { return 1; }
const Descriptor* pluginDescriptor(const Factory*, uint32_t index) {
    return index == 0 ? &descriptor : nullptr;
}
const Factory factory{&pluginCount, &pluginDescriptor, &createPlugin};
bool entryInit(const char*) { return true; }
void entryDeinit() {}
const void* entryFactory(const char* id) {
    return id && std::strcmp(id, kPluginFactoryId) == 0 ? &factory : nullptr;
}
}

extern "C" const Entry clap_entry{{1, 0, 0}, &entryInit, &entryDeinit, &entryFactory};
