#include "audio_engine.hpp"
#include "hirari-core-bridge/src/lib.rs.h"

namespace Hirari::Core::BridgeFFI {

AudioEngine::AudioEngine() : AudioEngine(true) {}

AudioEngine::AudioEngine(bool startDevice)
        : m_engine(std::make_shared<::Hirari::Core::Engine::HirariUnifiedEngine>()),
          m_driver(hirari_audio_driver_host_create(&AudioEngine::process_driver_block, this)) {
    // Whether this session may claim the device, and what a failed start is
    // reported as, are the Rust driver host's decisions.
    hirari_audio_driver_host_start_at_creation(m_driver, startDevice);
}

    AudioEngine::~AudioEngine() {
        // Rust joins the layout snapshot worker for this graph here, so the
        // background save can never read an engine that is being released.
        hirari_layout_snapshot_owner_release_host(m_engine.get());
        {
            std::lock_guard<std::mutex> lock(m_configMutex);
            hirari_audio_driver_host_stop(m_driver);
            // AudioEngine owns its session engine, so teardown of the shared graph
            // cannot tear down another project's graph.
            if (m_engine) m_engine->shutdown();
        }
        hirari_audio_driver_host_destroy(m_driver);
        m_driver = nullptr;
    }

    void AudioEngine::shutdown() const {
        // Preserve the existing FFI entry point while making its lifetime
        // semantics session-scoped: a handle stops only its own device and
        // graph.
        std::lock_guard<std::mutex> lock(m_configMutex);
        if (m_driver) hirari_audio_driver_host_stop(m_driver);
        if (m_engine) m_engine->shutdown();
    }

} // namespace Hirari::Core::BridgeFFI
