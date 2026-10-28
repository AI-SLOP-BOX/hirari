#pragma once

#include <cmath>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <filesystem>
#include <string>
#include <utility>

#include "stem_splitter.hpp"

namespace Hirari::DSP::Analysis {

enum class StemSeparatorBackend : uint8_t {
    Heuristic = 0,
    OnnxRuntime = 1,
    LibTorch = 2,
    ExternalProcess = 3,
};

inline const char* stemSeparatorBackendName(StemSeparatorBackend backend) noexcept {
    return hirari_stem_separator_backend_name(static_cast<uint8_t>(backend));
}

struct StemSeparatorConfig {
    StemSeparatorBackend backend = StemSeparatorBackend::Heuristic;
    std::string modelPath;
    uint32_t modelSampleRate = 44100;
    uint32_t maxChannels = 2;
};

struct StemSeparatorModelSpec {
    StemSeparatorConfig config;
    std::string displayName;
};

struct StemSeparatorStatus {
    StemSeparatorBackend backend = StemSeparatorBackend::Heuristic;
    std::string backendName;
    std::string displayName;
    std::string modelFileName;
    bool available = false;
};

inline bool validateStemSeparatorModel(const StemSeparatorModelSpec& spec,
                                       std::string* error = nullptr) {
    const auto code = hirari_stem_separator_validate_model(
        static_cast<uint8_t>(spec.config.backend),
        reinterpret_cast<const uint8_t*>(spec.config.modelPath.data()),
        spec.config.modelPath.size(), spec.config.modelSampleRate,
        spec.config.maxChannels);
    if (code == 0) return true;
    if (error) *error = hirari_stem_separator_validation_message(code);
    return false;
}

/**
 * Backend boundary for OSS stem-separation models.
 *
 * The engine deliberately does not vendor a neural-runtime ABI. An ONNX,
 * Demucs, Open-Unmix, or LibTorch adapter can be linked separately and
 * registered here without coupling the real-time graph to its allocator or
 * thread pool. Separation is an offline/background operation, never an audio
 * callback operation.
 */
class IStemSeparatorProvider {
public:
    virtual ~IStemSeparatorProvider() = default;
    virtual StemSeparatorBackend backend() const noexcept = 0;
    virtual bool available() const noexcept = 0;
    virtual const std::string& modelPath() const noexcept = 0;
    virtual bool split(const Core::AudioBuffer& input, double sampleRate,
                       StemSplitter::Stems& output) = 0;
};

class HeuristicStemSeparatorProvider final : public IStemSeparatorProvider {
public:
    StemSeparatorBackend backend() const noexcept override {
        return StemSeparatorBackend::Heuristic;
    }
    bool available() const noexcept override { return true; }
    const std::string& modelPath() const noexcept override { return m_empty; }
    bool split(const Core::AudioBuffer& input, double sampleRate,
               StemSplitter::Stems& output) override {
        output = m_splitter.split(input, sampleRate);
        return output.drums.getNumSamples() != 0;
    }

private:
    StemSplitter m_splitter;
    std::string m_empty;
};

/**
 * Adapter shell for an independently linked OSS runtime.
 * The callback owns the runtime and may be backed by ONNX Runtime, Demucs,
 * Open-Unmix, or another compatible implementation. Keeping the callback
 * injected makes the core buildable when the optional runtime is absent.
 */
class CallbackStemSeparatorProvider final : public IStemSeparatorProvider {
public:
    using SplitFunction = std::function<bool(const Core::AudioBuffer&, double,
                                             const StemSeparatorConfig&, StemSplitter::Stems&)>;

    CallbackStemSeparatorProvider(StemSeparatorConfig config, SplitFunction split)
        : m_config(std::move(config)), m_split(std::move(split)) {}

    StemSeparatorBackend backend() const noexcept override { return m_config.backend; }
    bool available() const noexcept override {
        return m_split && m_config.backend != StemSeparatorBackend::Heuristic &&
               !m_config.modelPath.empty();
    }
    const std::string& modelPath() const noexcept override { return m_config.modelPath; }
    bool split(const Core::AudioBuffer& input, double sampleRate,
               StemSplitter::Stems& output) override {
        if (!available() || !std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
        return m_split(input, sampleRate, m_config, output);
    }

private:
    StemSeparatorConfig m_config;
    SplitFunction m_split;
};

/** Control-thread registry. The active provider is atomically replaced under a mutex. */
class StemSeparatorRegistry {
public:
    static StemSeparatorRegistry& instance() {
        static StemSeparatorRegistry registry;
        return registry;
    }

    void setProvider(std::shared_ptr<IStemSeparatorProvider> provider) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_provider = std::move(provider);
    }

    bool installOSSProvider(const StemSeparatorModelSpec& spec,
                            CallbackStemSeparatorProvider::SplitFunction split,
                            std::string* error = nullptr) {
        if (!validateStemSeparatorModel(spec, error)) return false;
        if (!split) {
            if (error) *error = "OSS provider callback is empty";
            return false;
        }
        setProvider(std::make_shared<CallbackStemSeparatorProvider>(spec.config,
                                                                      std::move(split)));
        return true;
    }

    void resetToHeuristic() {
        setProvider(std::make_shared<HeuristicStemSeparatorProvider>());
    }

    StemSeparatorStatus status() const {
        const auto active = provider();
        StemSeparatorStatus result;
        if (!active) {
            result.backendName = "Unavailable";
            return result;
        }
        result.backend = active->backend();
        result.backendName = stemSeparatorBackendName(result.backend);
        result.available = active->available();
        const std::filesystem::path path(active->modelPath());
        result.modelFileName = path.filename().string();
        result.displayName = result.modelFileName.empty() ? result.backendName : result.modelFileName;
        return result;
    }

    std::shared_ptr<IStemSeparatorProvider> provider() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_provider;
    }

    bool split(const Core::AudioBuffer& input, double sampleRate,
               StemSplitter::Stems& output) const {
        auto active = provider();
        return active && active->available() && active->split(input, sampleRate, output);
    }

private:
    StemSeparatorRegistry()
        : m_provider(std::make_shared<HeuristicStemSeparatorProvider>()) {}

    mutable std::mutex m_mutex;
    std::shared_ptr<IStemSeparatorProvider> m_provider;
};

} // namespace Hirari::DSP::Analysis
