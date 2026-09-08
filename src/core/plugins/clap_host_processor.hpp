#pragma once

#include <stdint.h>
#include <array>
#include <algorithm>
#include <cstring>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <cmath>
#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif
#include "../../dsp/iprocessor.hpp"
#include "../audio_buffer.hpp"
#include "clap_abi_minimal.hpp"

namespace Aura::Core::Plugins {

class CLAPHostProcessor : public ::Aura::DSP::IProcessor {
public:
    enum class LoadState : uint8_t {
        Unloaded,
        LibraryResolved,
        Operational,
        Failed,
        Unsupported,
        NotInstantiated,
    };
    using ProcessFunction = void (CLAPHostProcessor::*)(
        Core::AudioBuffer&,
        Core::MidiBuffer&,
        const ::Aura::DSP::ProcessContext&) noexcept;
    static constexpr const char* kNoProcessFunctionDiagnostic =
        "CLAP process function is not connected";

    CLAPHostProcessor()
        : m_initialized(false)
        , m_handle(nullptr)
        , m_clapEntry(nullptr)
        , m_state(LoadState::Unloaded) {
    }

    ~CLAPHostProcessor() {
        unloadLibrary();
    }

    void prepareToPlay(double sr, uint32_t sz) noexcept override {
        m_sampleRate = sr;
        m_maxBlockSize = sz;
        if (!m_plugin || !m_initialized.load(std::memory_order_acquire)) return;
        if (!std::isfinite(sr) || sr <= 0.0 || sz == 0) {
            m_processFailed.store(true, std::memory_order_release);
            return;
        }
        // CLAP activation owns the audio configuration. Re-activate an
        // already loaded instance when the host changes sample rate or block
        // size instead of silently continuing with stale plugin buffers.
        if (m_processing && m_plugin->stop_processing) m_plugin->stop_processing(m_plugin);
        m_processing = false;
        if (m_plugin->deactivate) m_plugin->deactivate(m_plugin);
        if (!m_plugin->activate || !m_plugin->activate(m_plugin, sr, 1, sz) ||
            !m_plugin->start_processing || !m_plugin->start_processing(m_plugin)) {
            m_error = "CLAP reactivation failed";
            m_state.store(LoadState::Failed, std::memory_order_release);
            m_processFailed.store(true, std::memory_order_release);
            return;
        }
        m_processing = true;
        m_processFailed.store(false, std::memory_order_release);
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& /*context*/) noexcept override {
        const auto* plugin = m_plugin;
        const uint32_t channels = buffer.getNumChannels();
        const uint32_t frames = buffer.getNumSamples();
        if (plugin == nullptr || !m_initialized.load(std::memory_order_acquire) ||
            channels == 0 || channels > kMaxChannels || frames == 0 ||
            frames > m_maxBlockSize || !plugin->process) {
            m_processFailed.store(true, std::memory_order_release);
            buffer.clear();
            return;
        }

        for (uint32_t channel = 0; channel < channels; ++channel) {
            m_channelPointers[channel] = buffer.getWritePointer(channel);
            if (m_channelPointers[channel] == nullptr) {
                m_processFailed.store(true, std::memory_order_release);
                buffer.clear();
                return;
            }
        }
        m_inputEventCount = 0;
        for (size_t i = 0; i < midi.size() && m_inputEventCount < m_inputEvents.size(); ++i) {
            const auto& event = midi.getEvents()[i];
            if (event.size == 0 || event.sampleOffset >= frames) continue;
            auto& converted = m_inputEvents[m_inputEventCount++];
            if (event.size > sizeof(converted.sysex)) {
                --m_inputEventCount;
                continue;
            }
            // Core MidiEvent stores the 128-bit UMP payload, not the CLAP
            // event envelope (header/port metadata). Compare against the
            // payload width so MIDI 2.0 input is not misclassified and
            // rejected as an invalid legacy message.
            const bool midi2 = event.size == sizeof(ClapAbi::EventMidi2::data);
            const bool sysex = !midi2 && event.size >= 2 && event.data[0] == 0xf0;
            if (midi2) {
                converted.midi2.header = {sizeof(ClapAbi::EventMidi2),
                                          static_cast<uint32_t>(event.sampleOffset),
                                          ClapAbi::kCoreEventSpaceId,
                                          ClapAbi::kEventMidi2, 0};
                converted.midi2.port_index = 0;
                converted.midi2.reserved = 0;
                std::memcpy(converted.midi2.data, event.data, sizeof(converted.midi2.data));
            } else if (sysex) {
                converted.sysexEvent.header = {sizeof(ClapAbi::EventMidiSysex),
                                               static_cast<uint32_t>(event.sampleOffset),
                                               ClapAbi::kCoreEventSpaceId,
                                               ClapAbi::kEventMidiSysex, 0};
                converted.sysexEvent.port_index = 0;
                std::memcpy(converted.sysex, event.data, event.size);
                converted.sysexEvent.buffer = converted.sysex;
                converted.sysexEvent.size = event.size;
            } else {
                if (event.size != 2 && event.size != 3) {
                    --m_inputEventCount;
                    continue;
                }
                converted.midi.header = {sizeof(ClapAbi::EventMidi),
                                         static_cast<uint32_t>(event.sampleOffset),
                                         ClapAbi::kCoreEventSpaceId, ClapAbi::kEventMidi, 0};
                converted.midi.port_index = 0;
                std::memset(converted.midi.data, 0, sizeof(converted.midi.data));
                std::memcpy(converted.midi.data, event.data, event.size);
            }
        }
        for (uint32_t parameterId = 0; parameterId < kMaxParameters &&
             m_inputEventCount < m_inputEvents.size(); ++parameterId) {
            if (!m_parameterValid[parameterId].load(std::memory_order_acquire)) continue;
            auto& converted = m_inputEvents[m_inputEventCount++];
            converted.param.header = {sizeof(ClapAbi::EventParamValue), 0,
                                      ClapAbi::kCoreEventSpaceId,
                                      ClapAbi::kEventParamValue, 0};
            converted.param.param_id = parameterId;
            converted.param.cookie = nullptr;
            converted.param.value = m_parameterValues[parameterId].load(std::memory_order_acquire);
            converted.param.note_id = -1;
            converted.param.port_index = -1;
            converted.param.channel = -1;
            converted.param.key = -1;
        }
        // CLAP consumers are allowed to assume monotonically nondecreasing
        // event times. Parameter snapshots are emitted at time zero, so sort
        // the bounded array with insertion sort. `std::stable_sort` is not
        // suitable on the audio thread because standard library
        // implementations may allocate a temporary buffer.
        for (uint32_t i = 1; i < m_inputEventCount; ++i) {
            InputEvent current = m_inputEvents[i];
            uint32_t j = i;
            while (j > 0 && m_inputEvents[j - 1].midi.header.time >
                             current.midi.header.time) {
                m_inputEvents[j] = m_inputEvents[j - 1];
                --j;
            }
            m_inputEvents[j] = current;
        }
        m_inputEventsContext.events = m_inputEvents.data();
        m_inputEventsContext.count = m_inputEventCount;
        const ClapAbi::AudioBuffer audio{m_channelPointers.data(), nullptr, channels, 0, 0};
        const ClapAbi::InputEvents inputEvents{&m_inputEventsContext, &inputEventCount, &inputEventAt};
        const ClapAbi::OutputEvents outputEvents{this, &captureOutputEvent};
        m_activeMidi = &midi;
        m_activeFrames = frames;
        const ClapAbi::Process processData{
            0, frames, nullptr, &audio, 1,
            const_cast<ClapAbi::AudioBuffer*>(&audio), 1,
            &inputEvents, const_cast<ClapAbi::OutputEvents*>(&outputEvents)};
        if (plugin->process(plugin, &processData) == ClapAbi::Error) {
            m_processFailed.store(true, std::memory_order_release);
        }
        // A CLAP plugin may emit output events in a different order from the
        // input events (the ABI does not make producer order a timestamp
        // guarantee). Keep Aura's sample-accurate MIDI contract monotonic at
        // the boundary while preserving producer order for equal timestamps.
        midi.sort();
        // A third-party plugin must not be allowed to poison the rest of the
        // graph with NaN/Inf.  Sanitize at the format boundary and retain a
        // lock-free diagnostic counter for the UI/telemetry plane.
        for (uint32_t channel = 0; channel < channels; ++channel) {
            auto* output = m_channelPointers[channel];
            for (uint32_t frame = 0; frame < frames; ++frame) {
                if (!std::isfinite(output[frame])) {
                    output[frame] = 0.0f;
                    m_nonFiniteSamples.fetch_add(1, std::memory_order_relaxed);
                }
            }
        }
        m_activeMidi = nullptr;
        m_activeFrames = 0;
    }

    void reset() noexcept override {
        if (m_plugin && m_processing && m_plugin->reset) m_plugin->reset(m_plugin);
    }

    void setParameter(uint32_t id, float value) noexcept override {
        if (id >= kMaxParameters || !std::isfinite(value)) return;
        m_parameterValues[id].store(static_cast<double>(value), std::memory_order_release);
        m_parameterValid[id].store(true, std::memory_order_release);
    }

    std::vector<uint8_t> getState() const override {
        if (!m_plugin || !m_plugin->get_extension) return {};
        const auto* extension = static_cast<const ClapAbi::StateExtension*>(
            m_plugin->get_extension(m_plugin, ClapAbi::kStateExtensionId));
        if (!extension || !extension->save) return {};
        std::vector<uint8_t> state;
        state.reserve(4096);
        StateWriteContext context{&state};
        const ClapAbi::OStream stream{&context, &writeState};
        if (!extension->save(m_plugin, &stream) || state.size() > kMaxStateBytes) return {};
        return state;
    }

    bool setState(const std::vector<uint8_t>& state) override {
        if (state.size() > kMaxStateBytes || !m_plugin || !m_plugin->get_extension) return false;
        const auto* extension = static_cast<const ClapAbi::StateExtension*>(
            m_plugin->get_extension(m_plugin, ClapAbi::kStateExtensionId));
        if (!extension || !extension->load) return false;
        StateReadContext context{state.data(), state.size(), 0};
        const ClapAbi::IStream stream{&context, &readState};
        return extension->load(m_plugin, &stream) && context.offset == context.size;
    }

    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        return setState(state);
    }

    bool loadClap(const std::string& path, uint32_t index) {
        unloadLibrary();
        m_processFailed.store(false, std::memory_order_release);
        if (path.empty()) {
            m_error = "CLAP path is empty";
            m_state = LoadState::Failed;
            return false;
        }

#ifdef _WIN32
        m_handle = reinterpret_cast<void*>(LoadLibraryA(path.c_str()));
#else
        m_handle = dlopen(path.c_str(), RTLD_NOW | RTLD_LOCAL);
#endif
        if (!m_handle) {
            m_error = "failed to load CLAP module";
            m_state = LoadState::Failed;
            return false;
        }

        // Resolve CLAP entrypoint "clap_entry"
#ifdef _WIN32
        m_clapEntry = reinterpret_cast<void*>(GetProcAddress(reinterpret_cast<HMODULE>(m_handle), "clap_entry"));
#else
        m_clapEntry = dlsym(m_handle, "clap_entry");
#endif
        if (!m_clapEntry) {
            m_error = "CLAP entrypoint was not found";
            unloadLibrary();
            m_state = LoadState::Failed;
            return false;
        }

        m_entry = static_cast<const ClapAbi::Entry*>(m_clapEntry);
        if (m_entry->clap_version.major != 1 || !m_entry->init ||
            !m_entry->deinit || !m_entry->get_factory || !m_entry->init(path.c_str())) {
            m_error = "invalid CLAP entrypoint";
            unloadLibrary();
            m_state = LoadState::Failed;
            return false;
        }
        m_entryInitialized = true;
        const auto* factory = static_cast<const ClapAbi::Factory*>(
            m_entry->get_factory(ClapAbi::kPluginFactoryId));
        if (!factory || !factory->get_plugin_count || !factory->get_plugin_descriptor ||
            !factory->create_plugin || index >= factory->get_plugin_count(factory)) {
            m_error = "CLAP plugin factory or index is invalid";
            unloadLibrary();
            m_state = LoadState::Failed;
            return false;
        }
        const auto* descriptor = factory->get_plugin_descriptor(factory, index);
        if (!descriptor || !descriptor->id) {
            m_error = "CLAP descriptor is invalid";
            unloadLibrary();
            m_state = LoadState::Failed;
            return false;
        }
        m_plugin = factory->create_plugin(factory, &m_host, descriptor->id);
        if (!m_plugin || !m_plugin->init || !m_plugin->destroy ||
            !m_plugin->activate || !m_plugin->deactivate ||
            !m_plugin->start_processing || !m_plugin->stop_processing ||
            !m_plugin->process || !m_plugin->init(m_plugin) ||
            !m_plugin->activate(m_plugin, m_sampleRate, 1, m_maxBlockSize) ||
            !m_plugin->start_processing(m_plugin)) {
            m_error = "CLAP instance activation failed";
            unloadLibrary();
            m_state = LoadState::Failed;
            return false;
        }
        m_processing = true;
        m_initialized = true;
        m_processFunction.store(&CLAPHostProcessor::process, std::memory_order_release);
        m_state = LoadState::Operational;
        m_error.clear();
        return true;
    }

    LoadState loadState() const noexcept {
        const LoadState state = m_state.load(std::memory_order_acquire);
        return state == LoadState::Operational && !hasProcessFunction()
            ? LoadState::NotInstantiated
            : state;
    }
    bool hasProcessFunction() const noexcept {
        return m_processFunction.load(std::memory_order_acquire) != nullptr;
    }
    bool isOperational() const noexcept {
        return loadState() == LoadState::Operational && hasProcessFunction() &&
               m_initialized.load(std::memory_order_acquire);
    }
    const char* processDiagnostic() const noexcept {
        return hasProcessFunction() ? "CLAP process function is connected" : kNoProcessFunctionDiagnostic;
    }
    bool hasError() const noexcept {
        const auto state = loadState();
        return state == LoadState::Failed || state == LoadState::Unsupported ||
               state == LoadState::NotInstantiated || processFailed();
    }
    const std::string& errorMessage() const noexcept { return m_error; }
    bool processFailed() const noexcept {
        return m_processFailed.load(std::memory_order_acquire);
    }
    uint64_t nonFiniteSampleCount() const noexcept override {
        return m_nonFiniteSamples.load(std::memory_order_acquire);
    }
    std::string lastError() const {
        if (processFailed() && m_error.empty()) {
            return kNoProcessFunctionDiagnostic;
        }
        return m_error;
    }

private:
    static constexpr uint32_t kMaxChannels = 16;
    static constexpr uint32_t kMaxParameters = 128;
    static constexpr size_t kMaxStateBytes = 4u * 1024u * 1024u;
    struct StateWriteContext { std::vector<uint8_t>* bytes; };
    struct StateReadContext { const uint8_t* data; size_t size; size_t offset; };
    static int64_t writeState(const ClapAbi::OStream* stream, const void* data,
                              uint64_t size) {
        if (!stream || !stream->ctx || (!data && size != 0) || size > kMaxStateBytes)
            return -1;
        auto* context = static_cast<StateWriteContext*>(stream->ctx);
        if (!context->bytes || size > kMaxStateBytes - context->bytes->size()) return -1;
        const auto* bytes = static_cast<const uint8_t*>(data);
        try {
            context->bytes->insert(context->bytes->end(), bytes, bytes + size);
        } catch (...) {
            return -1;
        }
        return static_cast<int64_t>(size);
    }
    static int64_t readState(const ClapAbi::IStream* stream, void* data,
                             uint64_t size) noexcept {
        if (!stream || !stream->ctx || (!data && size != 0)) return -1;
        auto* context = static_cast<StateReadContext*>(stream->ctx);
        if ((!context->data && size != 0) || context->offset > context->size ||
            size > context->size - context->offset) return -1;
        std::memcpy(data, context->data + context->offset, static_cast<size_t>(size));
        context->offset += static_cast<size_t>(size);
        return static_cast<int64_t>(size);
    }
    struct InputEvent {
        union {
            ClapAbi::EventMidi midi;
            ClapAbi::EventMidiSysex sysexEvent;
            ClapAbi::EventMidi2 midi2;
            ClapAbi::EventParamValue param;
        };
        uint8_t sysex[sizeof(Core::MidiEvent::data)]{};
        InputEvent() noexcept { std::memset(&midi, 0, sizeof(midi)); }
    };
    struct InputEventContext {
        const InputEvent* events = nullptr;
        uint32_t count = 0;
    };
    static uint32_t inputEventCount(const ClapAbi::InputEvents* events) noexcept {
        const auto* context = static_cast<const InputEventContext*>(events->ctx);
        return context ? context->count : 0;
    }
    static const ClapAbi::EventHeader* inputEventAt(const ClapAbi::InputEvents* events,
                                                     uint32_t index) noexcept {
        const auto* context = static_cast<const InputEventContext*>(events->ctx);
        return context && index < context->count ?
            &context->events[index].midi.header : nullptr;
    }
    static bool captureOutputEvent(const ClapAbi::OutputEvents* events,
                                   const ClapAbi::EventHeader* header) noexcept {
        if (events == nullptr || header == nullptr || events->ctx == nullptr) return false;
        auto* host = static_cast<CLAPHostProcessor*>(events->ctx);
        if (host->m_activeMidi == nullptr || header->time >= host->m_activeFrames) return false;
        if (header->space_id != ClapAbi::kCoreEventSpaceId) return false;
        if (header->type == ClapAbi::kEventMidi) {
            if (header->size < sizeof(ClapAbi::EventMidi)) return false;
            // CLAP events originate in an external module and are not
            // required to be aligned for EventMidi. Copy the fixed-size
            // payload instead of dereferencing a potentially misaligned
            // struct pointer (which is UB under strict-aliasing/ASan).
            ClapAbi::EventMidi event{};
            std::memcpy(&event, header, sizeof(event));
            host->m_activeMidi->addEvent(header->time, event.data, sizeof(event.data));
            return true;
        }
        if (header->type == ClapAbi::kEventMidiSysex) {
            if (header->size < sizeof(ClapAbi::EventMidiSysex)) return false;
            ClapAbi::EventMidiSysex event{};
            std::memcpy(&event, header, sizeof(event));
            if (event.buffer == nullptr || event.size > sizeof(Core::MidiEvent::data)) return false;
            host->m_activeMidi->addEvent(header->time, event.buffer, event.size);
            return true;
        }
        if (header->type == ClapAbi::kEventMidi2) {
            if (header->size < sizeof(ClapAbi::EventMidi2) ||
                sizeof(ClapAbi::EventMidi2::data) > sizeof(Core::MidiEvent::data)) return false;
            ClapAbi::EventMidi2 event{};
            std::memcpy(&event, header, sizeof(event));
            host->m_activeMidi->addEvent(header->time,
                                         reinterpret_cast<const uint8_t*>(event.data),
                                         sizeof(event.data));
            return true;
        }
        return false;
    }
    static void requestRestart(const ClapAbi::Host*) noexcept {}
    static void requestProcess(const ClapAbi::Host*) noexcept {}
    static void requestCallback(const ClapAbi::Host*) noexcept {}
    static const void* noExtension(const ClapAbi::Host*, const char*) noexcept { return nullptr; }

    void unloadLibrary() noexcept {
        if (m_plugin != nullptr) {
            if (m_processing && m_plugin->stop_processing) m_plugin->stop_processing(m_plugin);
            if (m_plugin->deactivate) m_plugin->deactivate(m_plugin);
            if (m_plugin->destroy) m_plugin->destroy(m_plugin);
        }
        if (m_entryInitialized && m_entry != nullptr && m_entry->deinit) {
            m_entry->deinit();
        }
        m_plugin = nullptr;
        m_processing = false;
        m_initialized = false;
        m_processFunction.store(nullptr, std::memory_order_release);
        m_clapEntry = nullptr;
        m_entry = nullptr;
        m_entryInitialized = false;
        m_factory = nullptr;
        if (m_handle) {
#ifdef _WIN32
            FreeLibrary(reinterpret_cast<HMODULE>(m_handle));
#else
            dlclose(m_handle);
#endif
            m_handle = nullptr;
        }
        m_state = LoadState::Unloaded;
    }

    std::atomic<bool> m_initialized{false};
    std::atomic<ProcessFunction> m_processFunction{nullptr};
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 1024;

    void* m_handle = nullptr;
    void* m_clapEntry = nullptr;
    const ClapAbi::Entry* m_entry = nullptr;
    const ClapAbi::Factory* m_factory = nullptr;
    const ClapAbi::Plugin* m_plugin = nullptr;
    bool m_entryInitialized = false;
    bool m_processing = false;
    ClapAbi::Host m_host{
        {1, 0, 0}, "tinja-direct", "Tinja", "Tinja", "", "1",
        &requestRestart, &requestProcess, &requestCallback, &noExtension};
    std::array<float*, kMaxChannels> m_channelPointers{};
    std::array<InputEvent, Core::MidiBuffer::kMaxEventsPerBlock> m_inputEvents{};
    InputEventContext m_inputEventsContext{};
    uint32_t m_inputEventCount = 0;
    std::array<std::atomic<double>, kMaxParameters> m_parameterValues{};
    std::array<std::atomic<bool>, kMaxParameters> m_parameterValid{};
    // Valid only while the synchronous CLAP process callback is executing.
    Core::MidiBuffer* m_activeMidi = nullptr;
    uint32_t m_activeFrames = 0;
    std::atomic<LoadState> m_state{LoadState::Unloaded};
    std::atomic<uint64_t> m_nonFiniteSamples{0};
    std::string m_error;
    // RT-safe failure telemetry; diagnostic text is never written by process().
    std::atomic<bool> m_processFailed{false};
};

} // namespace Aura::Core::Plugins
