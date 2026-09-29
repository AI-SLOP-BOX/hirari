#include "audio_engine.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include <array>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <string_view>
#include <vector>
#include "status_queue.hpp"

namespace Hirari::Core::BridgeFFI {

AudioEngine::AudioEngine() : AudioEngine(true) {}

AudioEngine::AudioEngine(bool startDevice)
        : m_engine(std::make_shared<::Hirari::Core::Engine::HirariUnifiedEngine>()) {
        m_driver = std::make_unique<AudioDriverHost>();
        // The optional JACK host and the offline fallback both use this same
        // callback boundary. The fallback stores the callback and dispatches
        // it from process_audio_block without claiming hardware availability.
        m_driver->set_process_callback(&AudioEngine::process_driver_block, this);
        if (!startDevice) return;
        // Control-plane clients (CLI, offline render, project inspection and
        // isolated tests) must be able to construct an engine without taking
        // ownership of the user's hardware device. Normal interactive startup
        // remains unchanged when this variable is absent.
        const char* requestedMode = std::getenv("HIRARI_AUDIO_MODE");
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
        const char* isolated = std::getenv("HIRARI_NATIVE_TEST_ISOLATION");
        if (!(isolated && std::string_view(isolated) == "1")) {
            const bool started = m_driver->start();
            if (!started) {
                char message[192];
                std::snprintf(message, sizeof(message),
                              "Audio device startup failed: status=%s error=%d",
                              m_driver->status(),
                              static_cast<int>(m_driver->last_error_code()));
                ::Hirari::Core::StatusQueue::getInstance().pushFromAudio(
                    ::Hirari::Core::StatusQueue::Severity::Warning, message);
            }
        }
#else
        const bool started = m_driver->start();
        if (!started) {
            ::Hirari::Core::StatusQueue::getInstance().pushFromAudio(
            ::Hirari::Core::StatusQueue::Severity::Warning,
            "Audio backend unavailable on this platform; using offline backend.");
        } else {
            sync_jack_graph_config();
        }
#endif
    }

bool AudioEngine::start_audio_device() const {
    const char* requestedMode = std::getenv("HIRARI_AUDIO_MODE");
    if (requestedMode &&
        (std::strcmp(requestedMode, "offline") == 0 ||
         std::strcmp(requestedMode, "none") == 0)) {
        return false;
    }
#if defined(__APPLE__)
    const char* isolated = std::getenv("HIRARI_NATIVE_TEST_ISOLATION");
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
        ::Hirari::Core::Engine::HirariUnifiedEngine::getInstance().shutdown();
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
        ::Hirari::Core::Engine::HirariUnifiedEngine::TelemetryData telemetry{};
        if (!m_engine->copy_telemetry(index, telemetry)) return result;
        return ::Hirari::Core::Bridge::runtime_health_vector(
            rust::Slice<const float>(telemetry.peaksL, 256),
            rust::Slice<const float>(telemetry.peaksR, 256), telemetry.count,
            telemetry.dspLoad, telemetry.correlation, telemetry.activeVoices,
            telemetry.version, m_driver && m_driver->is_running(),
            m_engine->is_playing(), static_cast<float>(m_engine->get_playhead()));
    }

    rust::Vec<uint8_t> AudioEngine::get_video_frame() const {
        return m_engine ? m_engine->get_video_frame() : rust::Vec<uint8_t>();
    }


    float AudioEngine::get_cpu_total_v() const {
        return m_engine ? m_engine->get_dsp_load() : 0.0f;
    }

    void AudioEngine::discard_pending_audio_input() const {
        std::lock_guard<std::mutex> pollLock(m_inputPollMutex);
        if (m_driver) m_driver->discard_pending_input_blocks();
    }

    rust::Vec<float> AudioEngine::poll_audio_input(
        uint32_t& channelCount, uint64_t& droppedInputBlocks) const {
        rust::Vec<float> result;
        channelCount = 0;
        droppedInputBlocks = 0;
        if (!m_driver) return result;
        std::lock_guard<std::mutex> pollLock(m_inputPollMutex);
        constexpr uint32_t kMaxChannels = ::Hirari::Core::Driver::MacAudioInputBlockQueue::kMaxChannels;
        constexpr uint32_t kMaxFrames = ::Hirari::Core::Driver::MacAudioInputBlockQueue::kMaxFrames;
        const uint32_t channelCapacity = std::min(m_driver->input_channel_count(), kMaxChannels);
        if (channelCapacity == 0) return result;
        const size_t requiredSamples = static_cast<size_t>(channelCapacity) * kMaxFrames;
        if (m_inputPollScratch.size() < requiredSamples)
            m_inputPollScratch.resize(requiredSamples);
        std::array<float*, kMaxChannels> channels{};
        for (uint32_t channel = 0; channel < channelCapacity; ++channel)
            channels[channel] = m_inputPollScratch.data() + static_cast<size_t>(channel) * kMaxFrames;
        ::Hirari::Core::Driver::MacAudioInputBlockQueue::BlockInfo info{};
        uint64_t dropped = 0;
        if (!m_driver->poll_input_block(channels.data(), channelCapacity, kMaxFrames, info, dropped)) {
            droppedInputBlocks = dropped;
            return result;
        }
        droppedInputBlocks = dropped;
        channelCount = info.channelCount;
        return ::Hirari::Core::Bridge::interleave_planar_audio_input(
            rust::Slice<const float>(m_inputPollScratch.data(), m_inputPollScratch.size()),
            info.channelCount, info.frameCount, kMaxFrames);
    }

    rust::String AudioEngine::get_project_layout_json() const {
        return m_engine ? m_engine->get_project_layout_json() : rust::String();
    }

    rust::String AudioEngine::get_routing_snapshot_json() const {
        return m_engine ? m_engine->get_routing_snapshot_json() : rust::String();
    }

} // namespace Hirari::Core::BridgeFFI
