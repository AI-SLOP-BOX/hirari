#pragma once
#include <string>
#include <vector>
#include <memory>
#include <atomic>
#include <array>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <mutex>
#include "../dsp/iprocessor.hpp"
#include "../dsp/effects/pro_limiter.hpp"
#include "../dsp/effects/sub_bass_generator.hpp"
#include "../dsp/effects/compressor.hpp"
#include "../dsp/effects/tube_saturation.hpp"
#include "../scae/AuraAISuite.hpp"
#include "plugins/process_sandbox_processor.hpp"
#include "plugins/vst3_host_processor.hpp"

namespace Aura::Core::Plugin {

class NativeNoiseGate final : public DSP::IProcessor {
public:
    void prepareToPlay(double sr, uint32_t) noexcept override { m_sampleRate = sr > 0.0 ? sr : 44100.0; }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        if (channels == 0) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            const float target = level >= m_threshold ? 1.0f : 0.0f;
            m_gain += (target - m_gain) * (target > m_gain ? m_attack : m_release);
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] *= m_gain;
        }
    }
    void reset() noexcept override { m_gain = 0.0f; }
    std::string getName() const override { return "Aura Gate"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_threshold = std::clamp(value, 0.00001f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_threshold : 0.0f; }
private:
    double m_sampleRate = 44100.0;
    float m_threshold = 0.01f, m_gain = 0.0f;
    float m_attack = 0.05f, m_release = 0.005f;
};

class NativeTransientShaper final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            const float transient = std::max(0.0f, level - m_envelope);
            m_envelope += (level - m_envelope) * 0.08f;
            const float boost = 1.0f + std::min(2.0f, transient * m_amount * 8.0f);
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] = std::clamp(buffer.getReadPointer(c)[i] * boost, -1.0f, 1.0f);
        }
    }
    void reset() noexcept override { m_envelope = 0.0f; }
    std::string getName() const override { return "Aura Transient Shaper"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_amount = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
private:
    float m_envelope = 0.0f, m_amount = 0.5f;
};

class NativeDeEsser final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            m_detector += (level - m_detector) * 0.12f;
            const float excess = std::max(0.0f, m_detector - m_threshold);
            const float gain = std::clamp(1.0f - excess * m_amount * 2.5f, 0.15f, 1.0f);
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] *= gain;
        }
    }
    void reset() noexcept override { m_detector = 0.0f; }
    std::string getName() const override { return "Aura De-Esser"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_amount = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
private:
    float m_detector = 0.0f, m_amount = 0.5f, m_threshold = 0.45f;
};

class NativeDelay final : public DSP::IProcessor {
public:
    void prepareToPlay(double sr, uint32_t) noexcept override { m_sampleRate = sr > 0.0 ? sr : 44100.0; reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext& context) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        const uint32_t delay = std::clamp<uint32_t>(static_cast<uint32_t>(m_sampleRate * 60.0 / (std::max(1.0, context.bpm) * 2.0)), 1, kSize - 1);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const uint32_t read = (m_write + kSize - delay) % kSize;
            for (uint32_t c = 0; c < channels; ++c) {
                const float input = buffer.getReadPointer(c)[i];
                const float echo = m_buffer[c][read];
                m_buffer[c][m_write] = input + echo * m_feedback;
                buffer.getWritePointer(c)[i] = input * (1.0f - m_mix) + echo * m_mix;
            }
            ++m_write;
            if (m_write == kSize) m_write = 0;
        }
    }
    void reset() noexcept override { m_write = 0; for (auto& channel : m_buffer) channel.fill(0.0f); }
    std::string getName() const override { return "Aura Delay"; }
    // Eight delay cycles leave the 0.35 feedback path below -60 dB even at
    // the longest supported tempo-derived delay.
    uint32_t getTailSamples() const noexcept override { return kSize * 8u; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_mix = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_mix : 0.0f; }
private:
    static constexpr uint32_t kSize = 65536;
    double m_sampleRate = 44100.0;
    std::array<std::array<float, kSize>, 2> m_buffer{};
    uint32_t m_write = 0;
    float m_feedback = 0.35f, m_mix = 0.35f;
};

class NativeReverb final : public DSP::IProcessor {
public:
    void prepareToPlay(double sr, uint32_t) noexcept override {
        m_sampleRate = std::isfinite(sr) && sr >= 8'000.0 && sr <= 384'000.0
            ? sr : 44'100.0;
        reset();
    }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float inL = buffer.getReadPointer(0)[i];
            const float inR = channels > 1 ? buffer.getReadPointer(1)[i] : inL;
            const float wetL = m_lines[0][m_index] + m_lines[2][m_index];
            const float wetR = m_lines[1][m_index] + m_lines[3][m_index];
            m_lines[0][m_index] = inL + wetR * m_feedback;
            m_lines[1][m_index] = inR + wetL * m_feedback;
            m_lines[2][m_index] = inL * 0.7f + wetL * m_feedback * 0.6f;
            m_lines[3][m_index] = inR * 0.7f + wetR * m_feedback * 0.6f;
            buffer.getWritePointer(0)[i] = inL * (1.0f - m_mix) + wetL * m_mix * 0.5f;
            if (channels > 1) buffer.getWritePointer(1)[i] = inR * (1.0f - m_mix) + wetR * m_mix * 0.5f;
            m_index = (m_index + 1) % kSize;
        }
    }
    void reset() noexcept override { m_index = 0; for (auto& line : m_lines) line.fill(0.0f); }
    std::string getName() const override { return "Aura Reverb"; }
    // The four-line tank feeds back at 0.72; reserving 24 maximum-delay
    // traversals reaches the practical -60 dB decay point with margin.
    uint32_t getTailSamples() const noexcept override { return kSize * 24u; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_mix = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_mix : 0.0f; }
private:
    static constexpr uint32_t kSize = 8192;
    std::array<std::array<float, kSize>, 4> m_lines{};
    uint32_t m_index = 0;
    double m_sampleRate = 44'100.0;
    float m_feedback = 0.72f, m_mix = 0.3f;
};

class NativeDynamicEq final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override { reset(); }
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        const uint32_t channels = std::min<uint32_t>(buffer.getNumChannels(), 2);
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            float level = 0.0f;
            for (uint32_t c = 0; c < channels; ++c) level = std::max(level, std::abs(buffer.getReadPointer(c)[i]));
            m_env += (level - m_env) * 0.1f;
            const float reduction = m_env > m_threshold ? std::clamp((m_env - m_threshold) * m_amount, 0.0f, 0.75f) : 0.0f;
            for (uint32_t c = 0; c < channels; ++c) buffer.getWritePointer(c)[i] *= 1.0f - reduction;
        }
    }
    void reset() noexcept override { m_env = 0.0f; }
    std::string getName() const override { return "Aura Dynamic EQ"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_amount = std::clamp(value, 0.0f, 1.0f); }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_amount : 0.0f; }
private:
    float m_env = 0.0f, m_amount = 0.5f, m_threshold = 0.35f;
};

class NativeMidSide final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float l = buffer.getReadPointer(0)[i], r = buffer.getReadPointer(1)[i];
            const float mid = (l + r) * 0.70710678f * m_mid;
            const float side = (l - r) * 0.70710678f * m_side;
            buffer.getWritePointer(0)[i] = (mid + side) * 0.70710678f;
            buffer.getWritePointer(1)[i] = (mid - side) * 0.70710678f;
        }
    }
    void reset() noexcept override {}
    std::string getName() const override { return "Aura Mid/Side"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_side = std::clamp(value, 0.0f, 1.0f) * 2.0f; }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_side * 0.5f : 0.0f; }
private:
    float m_mid = 1.0f, m_side = 1.0f;
};

class NativeStereoWidth final : public DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&, const DSP::ProcessContext&) noexcept override {
        if (buffer.getNumChannels() < 2) return;
        for (uint32_t i = 0; i < buffer.getNumSamples(); ++i) {
            const float l = buffer.getReadPointer(0)[i], r = buffer.getReadPointer(1)[i];
            const float mid = (l + r) * 0.5f, side = (l - r) * 0.5f * m_width;
            buffer.getWritePointer(0)[i] = mid + side;
            buffer.getWritePointer(1)[i] = mid - side;
        }
    }
    void reset() noexcept override {}
    std::string getName() const override { return "Aura Stereo Width"; }
    uint32_t getNumParameters() const noexcept override { return 1; }
    void setParameter(uint32_t id, float value) noexcept override { if (id == 0) m_width = std::clamp(value, 0.0f, 1.0f) * 2.0f; }
    float getParameter(uint32_t id) const noexcept override { return id == 0 ? m_width * 0.5f : 0.0f; }
private:
    float m_width = 1.0f;
};

/**
 * @class ExternalPluginHost
 * @brief Unified Bridge for Internal and External Processors.
 */
class ExternalPluginHost : public DSP::IProcessor {
public:
    enum class Format { Internal, VST3, AU, CLAP };
    enum class LoadState { Unloaded, Operational, Failed, Unsupported };

    ExternalPluginHost(const std::string& path, Format format) : m_path(path), m_format(format) {
        for (auto& v : m_paramValues) v.store(0.0f);
        for (auto& mask : m_dirtyMask) mask.store(0, std::memory_order_relaxed);

        // Instantiate Internal Plugin if path matches
        if (m_format == Format::Internal) {
            if (m_path == "Aura/Limiter") m_internal = std::make_unique<DSP::Effects::ProLimiter>();
            else if (m_path == "Aura/SubBass") m_internal = std::make_unique<DSP::Effects::SubBassGenerator>();
            else if (m_path == "Aura/Compressor") m_internal = std::make_unique<DSP::Effects::DynamicCompressor>();
            else if (m_path == "Aura/Gate") m_internal = std::make_unique<NativeNoiseGate>();
            else if (m_path == "Aura/Saturation") m_internal = std::make_unique<DSP::Effects::TubeSaturation>();
            else if (m_path == "Aura/Transient") m_internal = std::make_unique<NativeTransientShaper>();
            else if (m_path == "Aura/DeEsser") m_internal = std::make_unique<NativeDeEsser>();
            else if (m_path == "Aura/Delay") m_internal = std::make_unique<NativeDelay>();
            else if (m_path == "Aura/Reverb") m_internal = std::make_unique<NativeReverb>();
            else if (m_path == "Aura/DynamicEQ") m_internal = std::make_unique<NativeDynamicEq>();
            else if (m_path == "Aura/MidSide") m_internal = std::make_unique<NativeMidSide>();
            else if (m_path == "Aura/Width") m_internal = std::make_unique<NativeStereoWidth>();
            // NeuralCloner is not part of the available processor interface;
            // leave it failed rather than manufacturing an external bridge.
            m_state = m_internal ? LoadState::Operational : LoadState::Failed;
            if (!m_internal) m_error = "unknown internal plugin: " + m_path;
        } else {
#if defined(AURA_ENABLE_VST3_SDK)
            if (m_format == Format::VST3) {
                m_vst3 = std::make_unique<Plugins::VST3HostProcessor>();
                if (!m_vst3->loadVst3(m_path)) {
                    m_state = LoadState::Failed;
                    m_error = m_vst3->lastError();
                } else {
                    m_state = LoadState::Operational;
                }
            } else
#endif
            {
            // All external formats use the same process boundary.  The
            // worker selects the ABI adapter from the bundle/path, while the
            // host keeps one lifecycle and failure contract for VST3, AU,
            // and CLAP instead of silently treating them as pass-through.
            m_external = std::make_unique<Plugins::ProcessSandboxProcessor>(m_path);
            m_state = m_external ? LoadState::Unloaded : LoadState::Failed;
            if (!m_external) m_error = "failed to allocate external plugin sandbox";
            }
        }
        // The host-side atomics are the only control/audio exchange for
        // parameters.  Seed them once before the processor becomes visible;
        // subsequent updates are applied by the audio thread at a block
        // boundary, avoiding a concurrent direct call into DSP state.
        if (m_internal) {
            for (uint32_t id = 0; id < m_paramValues.size(); ++id) {
                m_paramValues[id].store(m_internal->getParameter(id),
                                        std::memory_order_relaxed);
            }
        }
        reset();
    }

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        if (m_internal) m_internal->prepareToPlay(sr, bs);
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) m_vst3->prepareToPlay(sr, bs);
#endif
        if (m_external) {
            m_external->prepareToPlay(sr, bs);
            if (!m_external->isAlive() && !m_external->start()) {
                m_state = LoadState::Failed;
            } else {
                m_state = LoadState::Operational;
            }
        }
    }

    uint32_t getLatencySamples() const noexcept override {
        if (m_internal) return m_internal->getLatencySamples();
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->getLatencySamples();
#endif
        return m_external ? m_external->getLatencySamples() : 0u;
    }

    uint32_t getTailSamples() const noexcept override {
        if (m_internal) return m_internal->getTailSamples();
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->getTailSamples();
#endif
        return m_external ? m_external->getTailSamples() : 0u;
    }

    void process(AudioBuffer& b, MidiBuffer& midi, const DSP::ProcessContext& context) noexcept override {
        if (isBypassed()) return;

        // --- INTERNAL PLUG-IN EXECUTION ---
        if (m_internal) {
            // INDUSTRIAL PARAMETER DISPATCH (Bitmask Optimized)
            for (int m = 0; m < 2; ++m) {
                uint64_t dirty = m_dirtyMask[m].exchange(0, std::memory_order_acq_rel);
                if (dirty != 0) {
                    for (int i = 0; i < 64; ++i) {
                        if (dirty & (1ULL << i)) {
                            uint32_t paramId = m * 64 + i;
                            m_internal->setParameter(paramId, m_paramValues[paramId].load(std::memory_order_relaxed));
                        }
                    }
                }
            }
            m_internal->process(b, midi, context);
            return;
        }

#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) {
            m_vst3->process(b, midi, context);
            if (m_vst3->processFailed())
                m_processFailed.store(true, std::memory_order_release);
            return;
        }
#endif

        if (m_external) {
            m_external->process(b, midi, context);
            if (m_external->processFailed()) {
                // process() is the realtime boundary: publish only an atomic
                // edge here. State text and lifecycle state are collected by
                // pollExternalHealth() on the control thread.
                m_processFailed.store(true, std::memory_order_release);
            }
            return;
        }
        m_processFailed.store(true, std::memory_order_release);
    }

    void reset() noexcept override {
        if (m_internal) m_internal->reset();
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) m_vst3->reset();
#endif
        // External reset is intentionally a lifecycle no-op: stopping the
        // worker here would make every graph reset an implicit plugin crash.
    }
    std::vector<uint8_t> getState() const override {
        if (m_internal) {
            auto state = m_internal->getState();
            // A few legacy built-ins have no state serializer. Preserve a
            // deterministic control-plane snapshot rather than publishing an
            // empty blob; parameter values are restored independently.
            if (state.empty() && m_internal->getNumParameters() > 0) {
                state.resize(sizeof(float));
                const float value = m_paramValues[0].load(std::memory_order_relaxed);
                std::memcpy(state.data(), &value, sizeof(float));
            }
            return state;
        }
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->getState();
#endif
        return m_external ? m_external->getState() : std::vector<uint8_t>{};
    }
    bool setState(const std::vector<uint8_t>& state) override {
        if (m_internal) {
            const bool restored = m_internal->setState(state);
            if (restored) {
                for (uint32_t id = 0; id < m_internal->getNumParameters() && id < 128; ++id)
                    m_paramValues[id].store(m_internal->getParameter(id), std::memory_order_relaxed);
            }
            return restored;
        }
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->setState(state);
#endif
        return m_external && m_external->setState(state);
    }
    bool restoreStateChecked(const std::vector<uint8_t>& state) override {
        if (m_internal) {
            // Legacy built-in processors expose validated restoration through
            // setState rather than the newer restoreStateChecked hook.
            if (m_internal->restoreStateChecked(state)) {
                for (uint32_t id = 0; id < m_internal->getNumParameters() && id < 128; ++id)
                    m_paramValues[id].store(m_internal->getParameter(id), std::memory_order_relaxed);
                return true;
            }
            return setState(state);
        }
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->restoreStateChecked(state);
#endif
        return m_external && m_external->restoreStateChecked(state);
    }
    
    void setParameter(uint32_t id, float value) noexcept override {
        if (id < 128 && std::isfinite(value)) {
            m_paramValues[id].store(value, std::memory_order_relaxed);
            // Built-in processors are control-thread-owned and expose their
            // state synchronously. Keep their serializer in step with the
            // mirror so preset/project saves immediately capture edits made
            // between audio blocks.
            if (m_internal) m_internal->setParameter(id, value);
            // Keep the control-thread state authoritative for project saves and
            // UI reads. The audio thread is the sole caller that mutates the
            // internal processor, and applies the value at a block boundary.
            // A parameter may legitimately be edited before the isolated
            // worker has started.  ProcessSandboxProcessor retains the value
            // and replays it at the next lifecycle boundary; only a live
            // worker with a genuinely full mailbox is a queue failure.
            if (m_external) {
                const bool queued = m_external->enqueueParameterChange(id, value);
                if (!queued && m_external->isAlive()) {
                    m_parameterQueueFailed.store(true, std::memory_order_release);
                }
            }
            uint32_t maskIdx = id / 64;
            uint32_t bitIdx = id % 64;
            m_dirtyMask[maskIdx].fetch_or(1ULL << bitIdx, std::memory_order_release);
        }
    }

    float getParameter(uint32_t id) const noexcept override {
        // The control-side atomic is authoritative between audio blocks. The
        // internal DSP instance is updated at the next block boundary, so
        // reading it here makes UI/CLI callers observe a stale value.
        if (m_internal) return (id < 128) ? m_paramValues[id].load(std::memory_order_relaxed) : 0.0f;
        return (id < 128) ? m_paramValues[id].load(std::memory_order_relaxed) : 0.0f;
    }

    bool isOperational() const noexcept { return m_state == LoadState::Operational; }
    LoadState loadState() const noexcept { return m_state; }
    bool hasError() const noexcept { return m_state == LoadState::Failed || m_state == LoadState::Unsupported; }
    const std::string& errorMessage() const noexcept { return m_error; }
    bool processFailed() const noexcept {
        return m_processFailed.load(std::memory_order_acquire) ||
               m_parameterQueueFailed.load(std::memory_order_acquire);
    }
    std::string lastError() const {
        if (processFailed() && m_error.empty()) {
            return "external plugin process is unavailable";
        }
        return m_error;
    }
    const std::string& path() const { return m_path; }

    bool hasNativeEditor() const noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        if (m_vst3) return m_vst3->hasNativeEditor();
#endif
        return false;
    }
    uint64_t openNativeEditor(uintptr_t parent) noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        return m_vst3 ? m_vst3->openNativeEditor(parent) : 0;
#else
        (void)parent;
        return 0;
#endif
    }
    bool closeNativeEditor(uint64_t session) noexcept override {
#if defined(AURA_ENABLE_VST3_SDK)
        return m_vst3 && m_vst3->closeNativeEditor(session);
#else
        (void)session;
        return false;
#endif
    }

    bool startExternal() {
        if (!m_external) return false;
        const bool started = m_external->start();
        m_state = started ? LoadState::Operational : LoadState::Failed;
        if (!started) {
            m_error = m_external->failureText();
        } else {
            m_processFailed.store(false, std::memory_order_release);
            m_parameterQueueFailed.store(false, std::memory_order_release);
            m_error.clear();
        }
        return started;
    }
    void stopExternal() noexcept {
        if (m_external) m_external->stop();
        if (m_state == LoadState::Operational) m_state = LoadState::Unloaded;
    }
    bool pollExternalHealth() {
        if (!m_external) return false;
        const bool healthy = m_external->pollHealth();
        if (!healthy) {
            m_state = LoadState::Failed;
            m_error = m_external->failureText();
        }
        return healthy;
    }

private:
    std::string m_path;
    Format m_format;
    std::unique_ptr<DSP::IProcessor> m_internal;
    std::unique_ptr<Plugins::ProcessSandboxProcessor> m_external;
#if defined(AURA_ENABLE_VST3_SDK)
    std::unique_ptr<Plugins::VST3HostProcessor> m_vst3;
#endif
    LoadState m_state = LoadState::Unloaded;
    std::string m_error;

    // Thread-safe parameter state (Industrial Bitmask)
    std::array<std::atomic<float>, 128> m_paramValues;
    std::atomic<uint64_t> m_dirtyMask[2];
    // Set from the RT callback without allocating or mutating diagnostic text.
    std::atomic<bool> m_processFailed{false};
    std::atomic<bool> m_parameterQueueFailed{false};
};

/**
 * @class PluginScanner
 * @brief Professional OS-level scanner with Process Sandboxing.
 */
class PluginScanner {
public:
    enum class ScanState : uint8_t {
        Idle,
        Running,
        Rejected,
        Completed,
    };

    PluginScanner() : m_isScanning(false), m_state(ScanState::Idle) {}

    void scanInSandbox(const std::string& path) {
        if (m_isScanning.exchange(true, std::memory_order_acq_rel)) return;

        m_state.store(ScanState::Running, std::memory_order_release);
        {
            std::lock_guard<std::mutex> lock(m_metadataMutex);
            m_lastPath = path;
            m_lastError.clear();
        }

        const auto reject = [this](const char* reason) {
            {
                std::lock_guard<std::mutex> lock(m_metadataMutex);
                m_lastError = reason;
            }
            m_state.store(ScanState::Rejected, std::memory_order_release);
            m_isScanning.store(false, std::memory_order_release);
        };

        if (path.empty()) {
            reject("plugin scan rejected: path is empty");
            return;
        }

        std::error_code ec;
        const std::filesystem::path candidate(path);
        if (!std::filesystem::exists(candidate, ec)) {
            reject(ec ? "plugin scan rejected: unable to inspect path"
                      : "plugin scan rejected: path does not exist");
            return;
        }
        if (ec) {
            reject("plugin scan rejected: unable to inspect path");
            return;
        }
        if (std::filesystem::is_directory(candidate, ec)) {
            reject("plugin scan rejected: path is a directory");
            return;
        }
        if (ec || !std::filesystem::is_regular_file(candidate, ec)) {
            reject(ec ? "plugin scan rejected: unable to inspect file"
                      : "plugin scan rejected: path is not a regular file");
            return;
        }

        // This API validates and records a scan request only.  It deliberately
        // does not pretend to launch a sandbox process or load a plug-in SDK.
        m_state.store(ScanState::Completed, std::memory_order_release);
        m_isScanning.store(false, std::memory_order_release);
    }

    bool isScanning() const noexcept {
        return m_isScanning.load(std::memory_order_acquire);
    }

    ScanState state() const noexcept {
        return m_state.load(std::memory_order_acquire);
    }

    std::string lastError() const {
        std::lock_guard<std::mutex> lock(m_metadataMutex);
        return m_lastError;
    }

    std::string lastPath() const {
        std::lock_guard<std::mutex> lock(m_metadataMutex);
        return m_lastPath;
    }

private:
    std::atomic<bool> m_isScanning;
    std::atomic<ScanState> m_state;
    mutable std::mutex m_metadataMutex;
    std::string m_lastError;
    std::string m_lastPath;
};

} // namespace Aura::Core::Plugin
