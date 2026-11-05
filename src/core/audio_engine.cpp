#include "audio_engine.hpp"
#include "hirari-core-bridge/src/lib.rs.h"
#include <cstdio>
#include <iostream>
#include "status_queue.hpp"

namespace Hirari::Core::BridgeFFI {

AudioEngine::AudioEngine() : AudioEngine(true) {}

AudioEngine::AudioEngine(bool startDevice)
        : m_engine(std::make_shared<::Hirari::Core::Engine::HirariUnifiedEngine>()),
          m_driver(hirari_audio_driver_host_create(&AudioEngine::process_driver_block, this)) {
        // Control-plane clients (CLI, offline render, project inspection and
        // isolated tests) must be able to construct an engine without taking
        // ownership of the user's hardware device. The mode gate is Rust-owned
        // (audio_device_mode.rs) and normal interactive startup is unchanged.
        if (!startDevice || hirari_audio_device_mode_offline()) return;
        // Native integration tests create several independent session engines
        // in one process and the device callback still targets the process-wide
        // compatibility engine, so an isolated run builds the graph but never
        // attaches a real device. A platform with no device at all reports the
        // same failure from its silent host, which the engine logs and ignores.
        if (hirari_native_test_isolation()) return;
        if (hirari_audio_driver_host_start(m_driver)) return;
        char message[192];
        std::snprintf(message, sizeof(message), "Audio device startup failed: status=%s error=%d",
                      hirari_audio_driver_host_status(m_driver),
                      static_cast<int>(hirari_audio_driver_host_last_error_code(m_driver)));
        ::Hirari::Core::pushEngineStatus(::Hirari::Core::StatusSeverity::Warning, message);
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
