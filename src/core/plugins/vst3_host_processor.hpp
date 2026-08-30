#pragma once

#include <stdint.h>
#include <algorithm>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <cstring>
#include <cmath>
#ifdef _WIN32
#include <windows.h>
#else
#include <dlfcn.h>
#endif
#include "../../dsp/iprocessor.hpp"
#include "../audio_buffer.hpp"
#if defined(AURA_ENABLE_VST3_SDK)
#include "vst3_sandbox_adapter.hpp"
#endif

namespace Aura::Core::Plugins {

class VST3HostProcessor : public ::Aura::DSP::IProcessor {
public:
    enum class LoadState : uint8_t {
        Unloaded,
        LibraryResolved,
        Operational,
        Failed,
        Unsupported,
        NotInstantiated,
    };
    using ProcessFunction = void (VST3HostProcessor::*)(
        Core::AudioBuffer&,
        Core::MidiBuffer&,
        const ::Aura::DSP::ProcessContext&) noexcept;
    static constexpr const char* kNoProcessFunctionDiagnostic =
        "VST3 process function is not connected";

    VST3HostProcessor()
        : m_initialized(false)
        , m_handle(nullptr)
        , m_vst3FactoryProc(nullptr)
        , m_state(LoadState::Unloaded) {
    }

    ~VST3HostProcessor() {
        unloadLibrary();
    }

    void prepareToPlay(double sr, uint32_t sz) noexcept override {
        m_sampleRate = sr;
        m_maxBlockSize = sz;
#if defined(AURA_ENABLE_VST3_SDK)
        // VST3's ProcessSetup is immutable for the active processing session.
        // Re-prepare an already loaded instance when the device changes so a
        // later block cannot be sent with the old sample rate or max frame
        // size.  prepareToPlay is a control-plane call; the audio callback
        // only observes the resulting atomic initialized/state flags.
        const bool configurationChanged = sr != m_runtimeSampleRate || sz != m_runtimeMaxBlockSize;
        if (m_initialized.load(std::memory_order_acquire) && !m_loadedPath.empty() &&
            configurationChanged) {
            try {
                if (!m_runtime.load(m_loadedPath.c_str(), m_sampleRate, m_maxBlockSize)) {
                    m_error = m_runtime.error();
                    m_initialized.store(false, std::memory_order_release);
                    m_processFunction.store(nullptr, std::memory_order_release);
                    m_state.store(LoadState::Failed, std::memory_order_release);
                    m_processFailed.store(true, std::memory_order_release);
                } else {
                    m_runtimeSampleRate = m_sampleRate;
                    m_runtimeMaxBlockSize = m_maxBlockSize;
                }
            } catch (...) {
                m_error = "VST3 reconfiguration failed";
                m_initialized.store(false, std::memory_order_release);
                m_processFunction.store(nullptr, std::memory_order_release);
                m_state.store(LoadState::Failed, std::memory_order_release);
                m_processFailed.store(true, std::memory_order_release);
            }
        }
#endif
    }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer& midi, const ::Aura::DSP::ProcessContext& /*context*/) noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        const uint32_t channels = buffer.getNumChannels();
        const uint32_t frames = buffer.getNumSamples();
        if (!m_initialized.load(std::memory_order_acquire) || channels == 0 ||
            channels > SandboxProtocol::kMaxChannels || frames == 0 ||
            frames > m_maxBlockSize || !m_runtime.ready()) {
            m_processFailed.store(true, std::memory_order_release);
            buffer.clear();
            return;
        }
        for (uint32_t channel = 0; channel < channels; ++channel) {
            const float* input = buffer.getReadPointer(channel);
            if (!input) { m_processFailed.store(true, std::memory_order_release); buffer.clear(); return; }
            std::copy(input, input + frames, m_shared.input[channel]);
        }
        // Keep the direct VST3 host path semantically identical to the
        // isolated worker path.  Previously this path copied audio but left
        // the shared MIDI mailbox stale, so instruments appeared operational
        // while never receiving notes or SysEx.
        uint32_t midiCount = 0;
        for (size_t index = 0; index < midi.size() && midiCount < SandboxProtocol::kMaxMidiEvents; ++index) {
            const auto& source = midi.getEvents()[index];
            if (source.size > SandboxProtocol::kMaxMidiPayloadBytes) {
                m_shared.inputMidiTruncations.fetch_add(1, std::memory_order_relaxed);
                continue;
            }
            auto& destination = m_shared.midi[midiCount++];
            destination.sampleOffset = source.sampleOffset;
            destination.size = source.size;
            destination.articulationId = source.articulationId;
            if (source.size > 0)
                std::memcpy(destination.data, source.data, source.size);
        }
        if (midi.size() > SandboxProtocol::kMaxMidiEvents)
            m_shared.inputMidiTruncations.fetch_add(
                static_cast<uint32_t>(midi.size() - SandboxProtocol::kMaxMidiEvents),
                std::memory_order_relaxed);
        m_shared.midiEvents.store(midiCount, std::memory_order_release);
        m_shared.outputMidiEvents.store(0, std::memory_order_relaxed);
        if (!m_runtime.process(m_shared, channels, frames)) {
            m_processFailed.store(true, std::memory_order_release);
            buffer.clear();
            return;
        }
        // Keep malformed third-party output from contaminating downstream
        // DSP.  This runs at the native-plugin boundary and is intentionally
        // allocation-free; the count is consumed by diagnostics off-thread.
        for (uint32_t channel = 0; channel < channels; ++channel) {
            for (uint32_t frame = 0; frame < frames; ++frame) {
                if (!std::isfinite(m_shared.output[channel][frame])) {
                    m_shared.output[channel][frame] = 0.0f;
                    m_nonFiniteSamples.fetch_add(1, std::memory_order_relaxed);
                }
            }
        }
        const uint32_t outputMidiCount = std::min<uint32_t>(
            m_shared.outputMidiEvents.load(std::memory_order_acquire),
            SandboxProtocol::kMaxMidiEvents);
        for (uint32_t index = 0; index < outputMidiCount; ++index) {
            const auto& event = m_shared.outputMidi[index];
            if (event.size == 0 || event.size > SandboxProtocol::kMaxMidiPayloadBytes ||
                event.sampleOffset >= frames)
                continue;
            midi.addEvent(event.sampleOffset, event.data, event.size, event.articulationId);
        }
        midi.sort();
        for (uint32_t channel = 0; channel < channels; ++channel) {
            float* output = buffer.getWritePointer(channel);
            if (!output) { m_processFailed.store(true, std::memory_order_release); buffer.clear(); return; }
            std::copy(m_shared.output[channel], m_shared.output[channel] + frames, output);
        }
#else
        (void)buffer;
        (void)midi;
        if (loadState() != LoadState::Operational || !hasProcessFunction() ||
            !m_initialized.load(std::memory_order_acquire)) m_processFailed.store(true, std::memory_order_release);
#endif
    }

    void reset() noexcept override {}

    std::vector<uint8_t> getState() const override {
#if defined(AURA_ENABLE_VST3_SDK)
        std::vector<uint8_t> state;
        auto* runtime = const_cast<SandboxVST3::Runtime*>(&m_runtime);
        if (!runtime->saveState(state)) return {};
        return state;
#else
        return {};
#endif
    }

    bool setState(const std::vector<uint8_t>& state) override {
#if defined(AURA_ENABLE_VST3_SDK)
        return m_runtime.loadState(state.data(), state.size());
#else
        (void)state;
        return false;
#endif
    }

    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        return setState(state);
    }

    bool loadVst3(const std::string& path) {
        unloadLibrary();
        m_processFailed.store(false, std::memory_order_release);
        if (path.empty()) {
            m_error = "VST3 path is empty";
            m_state = LoadState::Failed;
            return false;
        }

#if defined(AURA_ENABLE_VST3_SDK)
        if (!m_runtime.load(path.c_str(), m_sampleRate, m_maxBlockSize)) {
            m_error = m_runtime.error();
            m_state = LoadState::Failed;
            return false;
        }
        m_loadedPath = path;
        m_runtimeSampleRate = m_sampleRate;
        m_runtimeMaxBlockSize = m_maxBlockSize;
        m_initialized = true;
        m_processFunction.store(&VST3HostProcessor::process, std::memory_order_release);
        m_state = LoadState::Operational;
        m_error.clear();
        return true;
#endif

#ifdef _WIN32
        m_handle = reinterpret_cast<void*>(LoadLibraryA(path.c_str()));
#else
        m_handle = dlopen(path.c_str(), RTLD_NOW | RTLD_LOCAL);
#endif
        if (!m_handle) {
            m_error = "failed to load VST3 module";
            m_state = LoadState::Failed;
            return false;
        }

        typedef void* (*GetFactoryProc)();
#ifdef _WIN32
        m_vst3FactoryProc = reinterpret_cast<GetFactoryProc>(GetProcAddress(reinterpret_cast<HMODULE>(m_handle), "GetPluginFactory"));
#else
        m_vst3FactoryProc = reinterpret_cast<GetFactoryProc>(dlsym(m_handle, "GetPluginFactory"));
#endif
        if (!m_vst3FactoryProc) {
            m_error = "VST3 factory symbol was not found";
            unloadLibrary();
            m_state = LoadState::Failed;
            return false;
        }

        // Resolving the module is not the same as creating a plugin
        // instance.  Keep processing bypassed until an instance exists.
        m_initialized = false;
        m_state = LoadState::NotInstantiated;
        m_error = "VST3 module resolved; plugin instance is not instantiated";
        // A shared library and its factory symbol are not an audio processor.
        // Returning true here made callers insert a permanent bypass while
        // reporting the plugin as loaded. Keep the diagnostic state so the
        // caller can distinguish a missing file from an incomplete host.
        return false;
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
        return hasProcessFunction() ? "VST3 process function is connected" : kNoProcessFunctionDiagnostic;
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
    void unloadLibrary() noexcept {
#if defined(AURA_ENABLE_VST3_SDK)
        m_runtime.unload();
#endif
        m_initialized = false;
        m_processFunction.store(nullptr, std::memory_order_release);
        m_vst3FactoryProc = nullptr;
        m_loadedPath.clear();
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
    [[maybe_unused]] double m_runtimeSampleRate = 0.0;
    [[maybe_unused]] uint32_t m_runtimeMaxBlockSize = 0;

    void* m_handle = nullptr;
    void* (*m_vst3FactoryProc)() = nullptr;
    std::atomic<LoadState> m_state{LoadState::Unloaded};
    std::string m_error;
    std::string m_loadedPath;
    // RT-safe failure telemetry; diagnostic text is never written by process().
    std::atomic<bool> m_processFailed{false};
    std::atomic<uint64_t> m_nonFiniteSamples{0};
#if defined(AURA_ENABLE_VST3_SDK)
    SandboxVST3::Runtime m_runtime;
    SandboxProtocol::SharedAudioBlock m_shared;
#endif
};

} // namespace Aura::Core::Plugins
