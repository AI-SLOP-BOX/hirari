#pragma once

#include <string>
#include <memory>
#include <vector>
#include <atomic>
#include <cstdint>
#include <filesystem>
#include <cstdlib>
#include <set>
#include <system_error>
#include <algorithm>
#include <cmath>
#if !defined(_WIN32)
#include <dlfcn.h>
#endif
#include "../../dsp/iprocessor.hpp"
#include "../audio_buffer.hpp"
#include "../midi_buffer.hpp"
#include "../rust_ffi.hpp"
#include "plugin_admission.hpp"
#include "process_sandbox_processor.hpp"
#include "../../dsp/effects/spectral_restoration_processor.hpp"
#include "../../dsp/effects/cabinet_simulator.hpp"
#include "../../dsp/effects/stereo_imager.hpp"
#include "../../dsp/effects/stereo_expander.hpp"
#include "../../dsp/effects/atmos_reverb.hpp"
#include "../../dsp/effects/deesser.hpp"
#include "../../dsp/effects/transient_shaper.hpp"
#include "../../dsp/effects/chromaglow.hpp"

namespace Hirari::Core::Plugins {

/**
 * @brief PluginFormat: The professional standard for external DSP extensions.
 */
enum class PluginFormat { VST3, AU, CLAP, Internal };

struct PluginDescription {
    std::string name;
    std::string manufacturer;
    PluginFormat format;
    std::string binaryPath;
};

/** A small, deterministic built-in processor used by the internal factory. */
class BuiltinGainProcessor final : public ::Hirari::DSP::IProcessor {
public:
    BuiltinGainProcessor() : m_state(hirari_builtin_gain_create()) {}
    ~BuiltinGainProcessor() override { hirari_builtin_gain_destroy(m_state); }
    BuiltinGainProcessor(const BuiltinGainProcessor&) = delete;
    BuiltinGainProcessor& operator=(const BuiltinGainProcessor&) = delete;

    void prepareToPlay(double sampleRate, uint32_t blockSize) noexcept override {
        hirari_builtin_gain_prepare(m_state, sampleRate, blockSize);
    }

    void process(::Hirari::Core::AudioBuffer& buffer, ::Hirari::Core::MidiBuffer&,
                 const ::Hirari::DSP::ProcessContext&) noexcept override {
        (void)hirari_builtin_gain_process_state(
            m_state,
            buffer.getArrayOfWritePointers(), buffer.getNumChannels(),
            buffer.getNumSamples());
    }

    void reset() noexcept override {}
    std::string getName() const override { return "Built-in Gain"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    bool getParameterDescriptor(uint32_t id, ParameterDescriptor& out) const noexcept override {
        if (id != 0) return false;
        out = ParameterDescriptor{0.0f, 4.0f, false};
        return true;
    }
    void setParameter(uint32_t id, float value) noexcept override {
        hirari_builtin_gain_set_parameter(m_state, id, value);
    }
    float getParameter(uint32_t id) const noexcept override {
        return hirari_builtin_gain_get_parameter(m_state, id);
    }

private:
    void* m_state = nullptr;
};

class PluginFactory {
public:
    static const char* formatName(PluginFormat format) noexcept {
        switch (format) {
        case PluginFormat::VST3: return "vst3";
        case PluginFormat::AU: return "au";
        case PluginFormat::CLAP: return "clap";
        case PluginFormat::Internal: return "builtin";
        }
        return "auto";
    }

    static std::shared_ptr<::Hirari::DSP::IProcessor> create(const PluginDescription& description,
                                                            std::string* error = nullptr) {
        if (error) error->clear();
        if (description.format != PluginFormat::Internal) {
            if (description.binaryPath.empty()) {
                if (error) *error = "external plugin binary path is empty";
                return {};
            }
            try {
                // All external formats share the lifecycle-isolated processor
                // boundary.  The worker selects CLAP, AU, or VST3 from the
                // package, so callers do not need a format-specific factory.
                return std::make_shared<ProcessSandboxProcessor>(
                    description.binaryPath, 44100.0, 512, formatName(description.format));
            } catch (...) {
                if (error) *error = "failed to create external plugin sandbox";
                return {};
            }
        }
        if (description.name.empty()) {
            if (error) *error = "internal plugin name is empty";
            return {};
        }
        if (description.name == "Gain" || description.name == "Built-in Gain") {
            return std::make_shared<BuiltinGainProcessor>();
        }
        if (description.name == "Spectral Restoration" ||
            description.name == "SpectraLayers Restoration") {
            return std::make_shared<::Hirari::DSP::Effects::SpectralRestorationProcessor>();
        }
        if (description.name == "Cabinet Simulator") {
            return std::make_shared<::Hirari::DSP::Effects::CabinetSimulator>();
        }
        if (description.name == "Stereo Imager") {
            return std::make_shared<::Hirari::DSP::Effects::StereoImager>();
        }
        if (description.name == "Stereo Expander") {
            return std::make_shared<::Hirari::DSP::Effects::StereoExpander>();
        }
        if (description.name == "Atmos Immersive Reverb") {
            return std::make_shared<::Hirari::DSP::Effects::AtmosReverb>();
        }
        if (description.name == "DeEsser" || description.name == "De-Esser") {
            return std::make_shared<::Hirari::DSP::Effects::DeEsser>();
        }
        if (description.name == "Transient Shaper") {
            return std::make_shared<::Hirari::DSP::Effects::TransientShaper>();
        }
        if (description.name == "ChromaGlow") {
            return std::make_shared<::Hirari::DSP::Effects::ChromaGlow>();
        }
        if (error) *error = "unknown internal plugin: " + description.name;
        return {};
    }
};

/**
 * @brief HirariPluginHost: The Pro-Grade bridge for external .vst3/.component files.
 * Uses POSIX dynamic library symbols loading to load VST3 factory entrypoints.
 */
class ExternalPluginProcessor : public ::Hirari::DSP::IProcessor {
public:
    enum class LoadState { Unloaded, Operational, Failed, Unsupported };
    using ProcessFunction = void (ExternalPluginProcessor::*)(
        ::Hirari::Core::AudioBuffer&,
        ::Hirari::Core::MidiBuffer&,
        const ::Hirari::DSP::ProcessContext&) noexcept;
    static constexpr const char* kNoProcessFunctionDiagnostic =
        "external plugin process function is not connected";

    ExternalPluginProcessor(const PluginDescription& desc)
        : m_desc(desc)
        , m_handle(nullptr)
        , m_vst3FactoryProc(nullptr)
        , m_state(LoadState::Unloaded) {
    }

    virtual ~ExternalPluginProcessor() {
        if (m_sandbox) m_sandbox->stop();
#if !defined(_WIN32)
        if (m_handle) dlclose(m_handle);
#endif
    }

    /**
     * @brief Dynamic loading of the external binary on the current OS.
     * Resolves VST3 factory entrypoint Symbol 'GetPluginFactory'.
     */
    bool load() {
        if (m_sandbox) {
            m_sandbox->stop();
            m_sandbox.reset();
        }
#if !defined(_WIN32)
        if (m_handle) {
            dlclose(m_handle);
            m_handle = nullptr;
        }
#endif
        m_error.clear();
        m_processFailed.store(false, std::memory_order_release);
        m_processFunction.store(nullptr, std::memory_order_release);
        m_state = LoadState::Unloaded;
        if (m_desc.binaryPath.empty()) {
            m_state = LoadState::Failed;
            m_error = "plugin binary path is empty";
            return false;
        }
        if (m_desc.format != PluginFormat::Internal) {
            try {
                m_sandbox = std::make_unique<ProcessSandboxProcessor>(m_desc.binaryPath,
                                                                       m_sampleRate,
                                                                       m_maxBlockSize);
            } catch (...) {
                m_sandbox.reset();
                m_state = LoadState::Failed;
                m_error = "failed to create external plugin sandbox";
                return false;
            }
            m_processFunction.store(&ExternalPluginProcessor::processSandbox,
                                    std::memory_order_release);
            // Loading the host object and starting the isolated worker are
            // separate lifecycle phases. prepareToPlay() performs the start
            // after the final audio configuration is known.
            m_state = LoadState::Unloaded;
            return true;
        }
        // Internal entries are real built-in processors.  The default
        // processor is intentionally a transparent pass, but it is fully
        // lifecycle-safe and can be replaced by a registered DSP factory
        // without changing the host contract.
        m_processFunction.store(&ExternalPluginProcessor::processInternal,
                                std::memory_order_release);
        m_state = LoadState::Operational;
        return true;
#if 0
        m_handle = dlopen(m_desc.binaryPath.c_str(), RTLD_NOW | RTLD_LOCAL);
        if (!m_handle) return false;

        if (m_desc.format == PluginFormat::VST3) {
            typedef void* (*GetFactoryProc)();
            m_vst3FactoryProc = reinterpret_cast<GetFactoryProc>(dlsym(m_handle, "GetPluginFactory"));
            if (!m_vst3FactoryProc) {
                dlclose(m_handle);
                m_handle = nullptr;
                return false;
            }
        }

        return true;
#endif
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        m_sampleRate = sr;
        m_maxBlockSize = bs;
        if (m_sandbox) {
            try {
                m_sandbox->prepareToPlay(sr, bs);
                if (!m_sandbox->isAlive() && !m_sandbox->start()) {
                    m_state = LoadState::Failed;
                } else {
                    m_state = LoadState::Operational;
                }
            } catch (...) {
                m_state = LoadState::Failed;
            }
        }
    }

    void process(::Hirari::Core::AudioBuffer& buffer,
                 ::Hirari::Core::MidiBuffer& midi,
                 const ::Hirari::DSP::ProcessContext& context) noexcept override {
        // No processing is performed unless the host is operational.
        if (loadState() != LoadState::Operational || !hasProcessFunction()) {
            m_processFailed.store(true, std::memory_order_release);
            // A failed plugin must not leak the previous processor's block
            // into the graph.  The caller owns this buffer, so fail closed
            // with deterministic silence at the common host boundary.
            buffer.clear();
            return;
        }
        const auto fn = m_processFunction.load(std::memory_order_acquire);
        (this->*fn)(buffer, midi, context);
        // Keep the generic host boundary defensive as well.  The dedicated
        // CLAP/AU/VST3 adapters sanitize their own output, but this wrapper
        // is also used by legacy and extension-host paths.  A malformed
        // plugin must never let NaN/Inf escape into the graph.
        for (uint32_t channel = 0; channel < buffer.getNumChannels(); ++channel) {
            float* samples = buffer.getWritePointer(channel);
            if (!samples) continue;
            for (uint32_t sample = 0; sample < buffer.getNumSamples(); ++sample) {
                if (!std::isfinite(samples[sample])) {
                    samples[sample] = 0.0f;
                    m_nonFiniteSamples.fetch_add(1, std::memory_order_relaxed);
                }
            }
        }
    }

    void reset() noexcept override {
        // Keep reset realtime-safe. ProcessSandboxProcessor clears its
        // recovery boundary without stopping the child worker; a later
        // control-plane reconfigure/restart performs plugin-specific reset.
        if (m_sandbox) m_sandbox->reset();
        m_processFailed.store(false, std::memory_order_release);
    }
    uint32_t getLatencySamples() const noexcept override {
        return m_sandbox ? m_sandbox->getLatencySamples() : m_latency;
    }
    // The generic host must expose the same control-plane state contract as
    // the format-specific adapters. Without these overrides, an external
    // plugin could process audio successfully while project save/restore
    // silently fell back to IProcessor's unsupported default.
    std::vector<uint8_t> getState() const override {
        return m_sandbox ? m_sandbox->getState() : std::vector<uint8_t>{};
    }
    bool setState(const std::vector<uint8_t>& state) override {
        return restoreStateChecked(state);
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        if (!m_sandbox) return false;
        const bool restored = m_sandbox->restoreStateChecked(state);
        if (!restored) m_processFailed.store(true, std::memory_order_release);
        return restored;
    }
    LoadState loadState() const noexcept {
        const LoadState state = m_state.load(std::memory_order_acquire);
        return state == LoadState::Operational && !hasProcessFunction()
            ? LoadState::Unsupported
            : state;
    }
    bool hasProcessFunction() const noexcept {
        return m_processFunction.load(std::memory_order_acquire) != nullptr;
    }
    bool isOperational() const noexcept {
        return loadState() == LoadState::Operational && hasProcessFunction();
    }
    const char* processDiagnostic() const noexcept {
        return hasProcessFunction() ? "external plugin process function is connected" : kNoProcessFunctionDiagnostic;
    }
    bool hasError() const noexcept { return loadState() == LoadState::Failed || loadState() == LoadState::Unsupported; }
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
    void processInternal(::Hirari::Core::AudioBuffer&, ::Hirari::Core::MidiBuffer&,
                         const ::Hirari::DSP::ProcessContext&) noexcept {}

    void processSandbox(::Hirari::Core::AudioBuffer& buffer,
                        ::Hirari::Core::MidiBuffer& midi,
                        const ::Hirari::DSP::ProcessContext&) noexcept {
        if (!m_sandbox) {
            m_processFailed.store(true, std::memory_order_release);
            buffer.clear();
            return;
        }
        (void)m_sandbox->processBlock(buffer, midi);
        if (m_sandbox->processFailed())
            m_processFailed.store(true, std::memory_order_release);
    }

    PluginDescription m_desc;
    std::unique_ptr<ProcessSandboxProcessor> m_sandbox;
    void* m_handle;
    [[maybe_unused]] void* (*m_vst3FactoryProc)();
    double m_sampleRate = 44100.0;
    uint32_t m_maxBlockSize = 512;
    uint32_t m_latency = 0;
    std::atomic<LoadState> m_state;
    std::atomic<ProcessFunction> m_processFunction{nullptr};
    std::string m_error;
    std::atomic<bool> m_processFailed{false};
    std::atomic<uint64_t> m_nonFiniteSamples{0};
};

/**
 * @brief PluginScanner: Logic Pro-style background indexing of external plugins.
 */
class PluginScanner {
public:
    static std::vector<PluginDescription> scanFolders(const std::vector<std::string>& folders) {
        std::vector<PluginDescription> result;
        std::set<std::string> seen;
        for (const auto& folder : folders) {
            if (folder.empty()) continue;
            std::error_code ec;
            const std::filesystem::path root(folder);
            if (!std::filesystem::is_directory(root, ec) || ec) continue;
            std::filesystem::recursive_directory_iterator it(
                root, std::filesystem::directory_options::skip_permission_denied, ec);
            const std::filesystem::recursive_directory_iterator end;
            for (; it != end && !ec; it.increment(ec)) {
                if (ec) continue;
                const auto path = it->path();
                if (!PluginAdmission::isSafeCandidate(path)) continue;
                std::string ext = path.extension().string();
                std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char character) {
                    return static_cast<char>(std::tolower(character));
                });
                PluginFormat format;
                if (ext == ".vst3") format = PluginFormat::VST3;
                else if (ext == ".clap") format = PluginFormat::CLAP;
#if defined(__APPLE__)
                else if (ext == ".component") format = PluginFormat::AU;
#endif
                else continue;
                const bool bundle = it->is_directory(ec);
                const bool file = it->is_regular_file(ec);
                if (!bundle && !file) continue;
                const std::string binary = path.string();
                if (!seen.insert(binary).second) continue;
                result.push_back({path.stem().string(), "Unknown", format, binary});
                if (bundle) it.disable_recursion_pending();
            }
        }
        std::sort(result.begin(), result.end(),
                  [](const auto& lhs, const auto& rhs) { return lhs.binaryPath < rhs.binaryPath; });
        return result;
    }

    static std::vector<PluginDescription> scanFolders() {
        std::vector<std::string> folders;
        // Keep the native scanner aligned with the Rust catalog.  The plural
        // name is the public path-list API; accept the old singular spelling
        // as a compatibility fallback for existing launch scripts.
        const char* raw = std::getenv("HIRARI_PLUGIN_PATHS");
        if (!raw || *raw == '\0') raw = std::getenv("HIRARI_PLUGIN_PATH");
        if (!raw || *raw == '\0') return {};
        std::string paths(raw);
        size_t begin = 0;
        const char separator =
#if defined(_WIN32)
            ';';
#else
            ':';
#endif
        while (begin <= paths.size()) {
            const size_t end = paths.find(separator, begin);
            const size_t length = end == std::string::npos ? paths.size() - begin : end - begin;
            if (length > 0) folders.emplace_back(paths.substr(begin, length));
            if (end == std::string::npos) break;
            begin = end + 1;
        }
        return scanFolders(folders);
    }
};

} // namespace Hirari::Core::Plugins
