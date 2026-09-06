#include "audio_engine.hpp"
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <string_view>
#include "status_queue.hpp"

namespace Aura::Core::BridgeFFI {

AudioEngine::AudioEngine() : AudioEngine(true) {}

AudioEngine::AudioEngine(bool startDevice)
        : m_engine(std::make_shared<::Aura::Core::Engine::AuraUnifiedEngine>()) {
        m_driver = std::make_unique<AudioDriverHost>();
#if !defined(__APPLE__)
        // The optional JACK host and the offline fallback both use this same
        // callback boundary. The fallback stores the callback and dispatches
        // it from process_audio_block without claiming hardware availability.
        m_driver->set_process_callback(&AudioEngine::process_driver_block, this);
#endif
        if (!startDevice) return;
        // Control-plane clients (CLI, offline render, project inspection and
        // isolated tests) must be able to construct an engine without taking
        // ownership of the user's hardware device. Normal interactive startup
        // remains unchanged when this variable is absent.
        const char* requestedMode = std::getenv("AURA_AUDIO_MODE");
        if (requestedMode &&
            (std::strcmp(requestedMode, "offline") == 0 ||
             std::strcmp(requestedMode, "none") == 0)) {
            return;
        }
#if defined(__APPLE__)
        // Native integration tests may create multiple independent session
        // engines in one process.  The legacy CoreAudio host callback still
        // targets the process-wide compatibility engine, so do not attach a
        // real device in that explicitly isolated mode.  The session graph
        // remains fully usable for offline/plugin sandbox validation.
        const char* isolated = std::getenv("AURA_NATIVE_TEST_ISOLATION");
        if (!(isolated && std::string_view(isolated) == "1")) {
            const bool started = m_driver->start();
            if (!started) {
                char message[192];
                std::snprintf(message, sizeof(message),
                              "Audio device startup failed: status=%s error=%d",
                              m_driver->status(),
                              static_cast<int>(m_driver->last_error_code()));
                ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                    ::Aura::Core::StatusQueue::Severity::Warning, message);
            }
        }
#else
        const bool started = m_driver->start();
        if (!started) {
            ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
            ::Aura::Core::StatusQueue::Severity::Warning,
            "Audio backend unavailable on this platform; using offline backend.");
        } else {
            sync_jack_graph_config();
        }
#endif
    }

bool AudioEngine::start_audio_device() const {
    const char* requestedMode = std::getenv("AURA_AUDIO_MODE");
    if (requestedMode &&
        (std::strcmp(requestedMode, "offline") == 0 ||
         std::strcmp(requestedMode, "none") == 0)) {
        return false;
    }
#if defined(__APPLE__)
    const char* isolated = std::getenv("AURA_NATIVE_TEST_ISOLATION");
    if (isolated && std::strcmp(isolated, "1") == 0) return false;
#endif
    std::lock_guard<std::mutex> lock(m_configMutex);
    const bool started = m_driver && m_driver->start();
    if (!started && m_engine) {
        // A failed device transition is also a transport boundary. Do not
        // leave the graph logically playing while no callback can consume it.
        m_engine->set_playing(false);
    }
#if !defined(__APPLE__)
    if (started) sync_jack_graph_config();
#endif
    return started;
}

    AudioEngine::~AudioEngine() {
        std::lock_guard<std::mutex> lock(m_configMutex);
        if (m_driver) {
            m_driver->stop();
        }
        // AudioEngine owns its session engine. AnalysisHub retains the same
        // shared_ptr while it is alive, so destruction remains ordered and
        // does not tear down another project's graph.
        if (m_engine) m_engine->shutdown();
    }

    void AudioEngine::shutdown() const {
        // Preserve the existing FFI entry point while making its lifetime
        // semantics session-scoped: a handle stops only its own device and
        // graph.
        std::lock_guard<std::mutex> lock(m_configMutex);
        if (m_driver) {
            m_driver->stop();
        }
        if (m_engine) m_engine->shutdown();
    }

    void AudioEngine::shutdown_process() noexcept {
        // This is intentionally explicit and is not called by AudioEngine's
        // destructor.  The host must invoke it once, at process shutdown,
        // after all bridge handles and dependent services have stopped.
        ::Aura::Core::Engine::AuraUnifiedEngine::getInstance().shutdown();
    }

    rust::Vec<float> AudioEngine::get_engine_status_v() const {
        rust::Vec<float> v;
        if (!m_engine) return v;
        v.push_back(m_engine->is_playing() ? 1.0f : 0.0f);
        v.push_back(static_cast<float>(m_engine->get_playhead()));
        return v;
    }

    rust::Vec<float> AudioEngine::get_runtime_health_v() const {
        rust::Vec<float> result;
        if (!m_engine) return result;
        const auto index = m_engine->get_active_telemetry_idx();
        const auto& telemetry = m_engine->get_telemetry(index);
        float peakL = 0.0f;
        float peakR = 0.0f;
        const uint32_t count = std::min<uint32_t>(telemetry.count, 256u);
        for (uint32_t i = 0; i < count; ++i) {
            if (std::isfinite(telemetry.peaksL[i])) peakL = std::max(peakL, std::abs(telemetry.peaksL[i]));
            if (std::isfinite(telemetry.peaksR[i])) peakR = std::max(peakR, std::abs(telemetry.peaksR[i]));
        }
        result.reserve(10);
        result.push_back(std::isfinite(telemetry.dspLoad) ? telemetry.dspLoad : 0.0f);
        result.push_back(peakL);
        result.push_back(peakR);
        result.push_back(std::isfinite(telemetry.correlation) ? telemetry.correlation : 0.0f);
        result.push_back(static_cast<float>(telemetry.activeVoices));
        result.push_back(static_cast<float>(count));
        result.push_back(static_cast<float>(telemetry.version & 0x00ffffffu));
        result.push_back(m_driver && m_driver->is_running() ? 1.0f : 0.0f);
        result.push_back(m_engine->is_playing() ? 1.0f : 0.0f);
        result.push_back(static_cast<float>(m_engine->get_playhead()));
        return result;
    }

    rust::Vec<uint8_t> AudioEngine::get_video_frame() const {
        return m_engine ? m_engine->get_video_frame() : rust::Vec<uint8_t>();
    }


    float AudioEngine::get_cpu_total_v() const {
        return m_engine ? m_engine->get_dsp_load() : 0.0f;
    }

    rust::Vec<float> AudioEngine::poll_audio_input() const {
        rust::Vec<float> result;
        if (!m_driver) return result;
        constexpr uint32_t kMaxChannels = 2;
        constexpr uint32_t kMaxFrames = 4096;
        float left[kMaxFrames]{};
        float right[kMaxFrames]{};
        float* channels[kMaxChannels] = {left, right};
        ::Aura::Core::Driver::MacAudioInputBlockQueue::BlockInfo info{};
        uint64_t dropped = 0;
        if (!m_driver->poll_input_block(channels, kMaxChannels, kMaxFrames, info, dropped))
            return result;
        result.reserve(static_cast<size_t>(info.channelCount) * info.frameCount);
        for (uint32_t frame = 0; frame < info.frameCount; ++frame)
            for (uint32_t channel = 0; channel < info.channelCount; ++channel)
                result.push_back(channels[channel][frame]);
        return result;
    }

    rust::String AudioEngine::get_project_layout_json() const {
        return m_engine ? m_engine->get_project_layout_json() : rust::String();
    }

    rust::String AudioEngine::get_routing_snapshot_json() const {
        return m_engine ? m_engine->get_routing_snapshot_json() : rust::String();
    }

} // namespace Aura::Core::BridgeFFI
