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

namespace Aura::DSP::Analysis {

enum class StemSeparatorBackend : uint8_t {
    Heuristic = 0,
    OnnxRuntime = 1,
    LibTorch = 2,
    ExternalProcess = 3,
};

inline const char* stemSeparatorBackendName(StemSeparatorBackend backend) noexcept {
    switch (backend) {
        case StemSeparatorBackend::Heuristic: return "Heuristic";
        case StemSeparatorBackend::OnnxRuntime: return "ONNX Runtime";
        case StemSeparatorBackend::LibTorch: return "LibTorch";
        case StemSeparatorBackend::ExternalProcess: return "External Process";
    }
    return "Unknown";
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
    const auto fail = [error](const char* message) {
        if (error) *error = message;
        return false;
    };
    if (spec.config.backend == StemSeparatorBackend::Heuristic) {
        // The built-in provider is valid without a model. This validator is
        // also used by the registry status path, so heuristic must not be
        // rejected as though it were an external neural backend.
        return true;
    }
    if (spec.config.modelPath.empty()) return fail("model path is empty");
    if (spec.config.modelSampleRate < 8000 || spec.config.modelSampleRate > 384000)
        return fail("model sample rate is outside the supported range");
    if (spec.config.maxChannels == 0 || spec.config.maxChannels > 32)
        return fail("model channel count is outside the supported range");

    std::error_code ec;
    const std::filesystem::path path(spec.config.modelPath);
    if (!std::filesystem::is_regular_file(path, ec) || ec)
        return fail("model path is not a regular file");
    const auto size = std::filesystem::file_size(path, ec);
    if (ec || size == 0) return fail("model file is empty or unreadable");
    if (size > (4ull * 1024ull * 1024ull * 1024ull))
        return fail("model file exceeds the 4 GiB safety limit");

    const auto extension = path.extension().string();
    if (spec.config.backend == StemSeparatorBackend::OnnxRuntime && extension != ".onnx")
        return fail("ONNX Runtime backend requires an .onnx model");
    if (spec.config.backend == StemSeparatorBackend::LibTorch &&
        extension != ".pt" && extension != ".pth")
        return fail("LibTorch backend requires a .pt or .pth model");
    return true;
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

} // namespace Aura::DSP::Analysis
