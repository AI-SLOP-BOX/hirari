#pragma once

#include <atomic>
#include <memory>
#include <utility>
#include "../../dsp/iprocessor.hpp"
#include "../../core/audio_buffer.hpp"
#include "../../core/log_buffer.hpp"

namespace Aura::Core::Plugins {

/**
 * @class PluginSandboxHost
 * @brief Exception-safe bypass wrapper for an in-process plugin.
 *
 * This class is deliberately not a process sandbox. Recovering SIGSEGV or
 * SIGILL with siglongjmp is undefined for C++ objects and can leave the audio
 * graph, allocator, and locks corrupted. Real crash isolation must be provided
 * by a child process/IPC host; this wrapper only contains C++ exceptions and
 * bypasses after a failure.
 */
class PluginSandboxHost : public DSP::IProcessor {
public:
    explicit PluginSandboxHost(std::shared_ptr<DSP::IProcessor> inner)
        : m_inner(std::move(inner)) {}

    void prepareToPlay(double sr, uint32_t bs) noexcept override {
        if (m_hasCrashed.load(std::memory_order_acquire) || !m_inner) return;
        try {
            m_inner->prepareToPlay(sr, bs);
        } catch (...) {
            markFailed("PLUGIN_EXCEPTION_CAUGHT");
        }
    }

    void process(Core::AudioBuffer& b, Core::MidiBuffer& m,
                 const DSP::ProcessContext& context) noexcept override {
        if (m_hasCrashed.load(std::memory_order_acquire) || !m_inner) {
            b.clear();
            return;
        }

        try {
            m_inner->process(b, m, context);
        } catch (...) {
            markFailed("PLUGIN_EXCEPTION_CAUGHT");
            b.clear();
        }
    }

    void reset() noexcept override {
        if (m_hasCrashed.load(std::memory_order_acquire) || !m_inner) return;
        try {
            m_inner->reset();
        } catch (...) {
            markFailed("PLUGIN_EXCEPTION_CAUGHT");
        }
    }

    bool hasCrashed() const noexcept {
        return m_hasCrashed.load(std::memory_order_acquire);
    }

private:
    void markFailed(const char* message) noexcept {
        m_hasCrashed.store(true, std::memory_order_release);
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, message);
    }

    std::shared_ptr<DSP::IProcessor> m_inner;
    std::atomic<bool> m_hasCrashed{false};
};

} // namespace Aura::Core::Plugins
