#pragma once

#include <atomic>
#include <cmath>
#include <cstdint>
#include <mutex>
#include <string>

#if defined(AURA_ENABLE_JACK)
#include <jack/jack.h>
#endif

namespace Aura::Core::External {

/** Optional JACK audio bridge. JACK is opt-in and never reports a fake device. */
class JackBridgeDeep {
public:
    using ProcessCallback = void (*)(const float* const* inputs,
                                      float* const* outputs,
                                      uint32_t frames,
                                      void* context) noexcept;

    static JackBridgeDeep& getInstance() {
        static JackBridgeDeep instance;
        return instance;
    }

    ~JackBridgeDeep() { shutdown(); }

    // Compatibility wrapper for older callers that used a void method.
    void initialize(const std::string& clientName) { (void)tryInitialize(clientName); }

    bool tryInitialize(const std::string& clientName) {
        std::lock_guard<std::mutex> lock(m_mutex);
        shutdownLocked();
        if (clientName.empty() || clientName.size() > 128 || clientName.find('\0') != std::string::npos) {
            m_error = "JACK client name is invalid";
            return false;
        }
        m_clientName = clientName;
#if defined(AURA_ENABLE_JACK)
        jack_status_t status = JackFailure;
        m_client = jack_client_open(m_clientName.c_str(), JackNullOption, &status);
        if (!m_client) {
            m_error = "JACK client could not be opened";
            return false;
        }
        m_sampleRate.store(jack_get_sample_rate(m_client), std::memory_order_release);
        m_bufferSize.store(jack_get_buffer_size(m_client), std::memory_order_release);
        m_inputPorts[0] = jack_port_register(m_client, "input_l", JACK_DEFAULT_AUDIO_TYPE, JackPortIsInput, 0);
        m_inputPorts[1] = jack_port_register(m_client, "input_r", JACK_DEFAULT_AUDIO_TYPE, JackPortIsInput, 0);
        m_outputPorts[0] = jack_port_register(m_client, "output_l", JACK_DEFAULT_AUDIO_TYPE, JackPortIsOutput, 0);
        m_outputPorts[1] = jack_port_register(m_client, "output_r", JACK_DEFAULT_AUDIO_TYPE, JackPortIsOutput, 0);
        if (!m_inputPorts[0] || !m_inputPorts[1] || !m_outputPorts[0] || !m_outputPorts[1] ||
            jack_set_process_callback(m_client, &JackBridgeDeep::process, this) != 0 ||
            jack_activate(m_client) != 0) {
            m_error = "JACK ports or process callback could not be activated";
            shutdownLocked();
            return false;
        }
        m_running.store(true, std::memory_order_release);
        m_error.clear();
        return true;
#else
        m_error = "JACK support is not compiled; enable AURA_ENABLE_JACK with libjack";
        return false;
#endif
    }

    void shutdown() noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        shutdownLocked();
    }

    bool isRunning() const noexcept { return m_running.load(std::memory_order_acquire); }
    const std::string& lastError() const noexcept { return m_error; }
    double sampleRate() const noexcept { return m_sampleRate.load(std::memory_order_acquire); }
    uint32_t bufferSize() const noexcept { return m_bufferSize.load(std::memory_order_acquire); }

    void setProcessCallback(ProcessCallback callback, void* context = nullptr) noexcept {
        m_context.store(context, std::memory_order_release);
        m_callback.store(callback, std::memory_order_release);
    }

    /** JACK realtime callback: no allocation, locks, or logging. */
    static int process(uint32_t nframes, void* arg) noexcept {
        auto* self = static_cast<JackBridgeDeep*>(arg);
        if (!self || !self->m_running.load(std::memory_order_acquire) || nframes == 0 ||
            nframes > self->m_bufferSize.load(std::memory_order_acquire)) return 0;
#if defined(AURA_ENABLE_JACK)
        const float* inputs[2] = {
            static_cast<const float*>(jack_port_get_buffer(self->m_inputPorts[0], nframes)),
            static_cast<const float*>(jack_port_get_buffer(self->m_inputPorts[1], nframes))};
        float* outputs[2] = {
            static_cast<float*>(jack_port_get_buffer(self->m_outputPorts[0], nframes)),
            static_cast<float*>(jack_port_get_buffer(self->m_outputPorts[1], nframes))};
        if (!inputs[0] || !inputs[1] || !outputs[0] || !outputs[1]) return 0;
        const auto callback = self->m_callback.load(std::memory_order_acquire);
        if (callback) callback(inputs, outputs, nframes, self->m_context.load(std::memory_order_acquire));
        else {
            for (uint32_t frame = 0; frame < nframes; ++frame) {
                outputs[0][frame] = 0.0f;
                outputs[1][frame] = 0.0f;
            }
        }
#else
        (void)nframes;
#endif
        return 0;
    }

    void setSampleRate(double rate) noexcept {
        if (std::isfinite(rate) && rate >= 8000.0 && rate <= 384000.0)
            m_sampleRate.store(rate, std::memory_order_release);
    }

private:
    JackBridgeDeep() = default;

    void shutdownLocked() noexcept {
        m_running.store(false, std::memory_order_release);
        m_callback.store(nullptr, std::memory_order_release);
        m_context.store(nullptr, std::memory_order_release);
#if defined(AURA_ENABLE_JACK)
        if (m_client) {
            jack_deactivate(m_client);
            jack_client_close(m_client);
            m_client = nullptr;
        }
        m_inputPorts[0] = m_inputPorts[1] = nullptr;
        m_outputPorts[0] = m_outputPorts[1] = nullptr;
#endif
    }

    mutable std::mutex m_mutex;
    std::string m_clientName;
    std::string m_error;
    std::atomic<double> m_sampleRate{48000.0};
    std::atomic<uint32_t> m_bufferSize{0};
    std::atomic<bool> m_running{false};
    std::atomic<ProcessCallback> m_callback{nullptr};
    std::atomic<void*> m_context{nullptr};
#if defined(AURA_ENABLE_JACK)
    jack_client_t* m_client = nullptr;
    jack_port_t* m_inputPorts[2] = {};
    jack_port_t* m_outputPorts[2] = {};
#endif
};

} // namespace Aura::Core::External
