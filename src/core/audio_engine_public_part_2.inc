    bool add_region(uint32_t tid, rust::Str path, double startBeat) const {
        return m_engine && m_engine->add_region(tid, std::string_view(path.data(), path.size()), startBeat);
    }
    // The sandbox query surface lives in Rust (plugin_sandbox_maintenance.rs);
    // it reaches the graph through the sandbox host ops, not this bridge.

    rust::String get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId) const {
        return rust::String(m_engine ? m_engine->get_plugin_parameter_name(tid, pluginIndex, parameterId) : "");
    }
    bool save_project(rust::Str path) const { return m_engine && m_engine->saveProject(std::string_view(path.data(), path.size())); }
    bool load_project(rust::Str path) const { return m_engine && m_engine->loadProject(std::string_view(path.data(), path.size())); }
    bool bounce_project(rust::Str path, uint32_t format) const { return m_engine && m_engine->bounce_project(std::string_view(path.data(), path.size()), format); }
    bool bounce_project_async(rust::Str path, uint32_t format) const { return m_engine && m_engine->bounce_project_async(std::string_view(path.data(), path.size()), format); }


    uint64_t open_plugin_native_editor(uint32_t trackId, uint32_t pluginIndex,
                                       uint64_t parent) const {
        if (!m_engine || !m_engine->has_plugin_native_editor(trackId, pluginIndex)) return 0;
        uint64_t session = m_engine->open_plugin_native_editor(
            trackId, pluginIndex, static_cast<uintptr_t>(parent));
        if (session != 0)
            hirari_native_editor_sessions_adopt(m_nativeEditorSessions, trackId, pluginIndex,
                                               session);
        return session;
    }
    bool close_plugin_native_editor(uint32_t trackId, uint32_t pluginIndex) const {
        const uint64_t session =
            hirari_native_editor_sessions_session(m_nativeEditorSessions, trackId, pluginIndex);
        const bool processorClosed = session != 0 && m_engine &&
            m_engine->close_plugin_native_editor(trackId, pluginIndex, session);
        const bool wrapperClosed = session != 0 &&
            hirari_native_editor_sessions_forget(m_nativeEditorSessions, trackId, pluginIndex);
        if (processorClosed && !wrapperClosed)
            hirari_native_editor_sessions_forget(m_nativeEditorSessions, trackId, pluginIndex);
        return wrapperClosed || processorClosed;
    }
    bool plugin_native_editor_embedded(uint32_t trackId, uint32_t pluginIndex) const {
        return hirari_native_editor_sessions_embedded(m_nativeEditorSessions, trackId, pluginIndex);
    }



    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, rust::Str operation, float parameter, bool enabled) const {
        return m_engine && m_engine->append_region_processing(tid, rid, stepId, std::string_view(operation.data(), operation.size()), parameter, enabled);
    }
    // Compatibility API: stop only this wrapper's device.  The underlying
    // HirariUnifiedEngine is a process-wide singleton and must not be shut down
    // when one Rust/FFI handle is dropped.
    void shutdown() const;

    uint64_t queue_region_waveform(uint32_t tid, uint32_t rid) const {
        if (!m_engine || tid == 0 || rid == 0) return 0;
        auto* context = new (std::nothrow) std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine>(m_engine);
        if (!context) return 0;
        return hirari_waveform_snapshot_request(
            context, tid, rid,
            +[](void* opaque, uint32_t trackId, uint32_t regionId,
                float* output, size_t capacity) noexcept -> size_t {
                auto* engine = static_cast<std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine>*>(opaque);
                if (!engine || !*engine || !output) return 0;
                try {
                    const auto waveform = (*engine)->get_region_waveform(trackId, regionId, 128);
                    const size_t count = std::min(capacity, waveform.size());
                    std::copy_n(waveform.begin(), count, output);
                    return count;
                } catch (...) { return 0; }
            },
            +[](void* opaque) noexcept {
                delete static_cast<std::shared_ptr<::Hirari::Core::Engine::HirariUnifiedEngine>*>(opaque);
            });
    }
    // Command Dispatch (admission lives in Rust; the engine keeps the queue)
    bool push_command(CommandType type, uint32_t tid, float val, uint64_t ts,
                      uint64_t expectedProjectGeneration,
                      uint64_t expectedAudioGeneration) const {
        return m_engine && hirari_engine_push_command(
            m_engine.get(), static_cast<uint32_t>(type), tid, val, ts,
            expectedProjectGeneration, expectedAudioGeneration);
    }

    // Core Access for telemetry shims
    const ::Hirari::Core::Engine::HirariUnifiedEngine& get_core() const { return *m_engine; }
