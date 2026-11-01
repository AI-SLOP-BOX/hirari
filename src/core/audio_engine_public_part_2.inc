    bool add_region(uint32_t tid, rust::Str path, double startBeat) const {
        return m_engine && m_engine->add_region(tid, std::string_view(path.data(), path.size()), startBeat);
    }
    bool process_sandboxed_plugin_block(uint32_t tid, uint32_t sandbox_index,
                                        rust::Slice<float> left, rust::Slice<float> right) const {
        if (!m_engine || left.empty() || left.size() != right.size()) return false;
        return m_engine->process_sandboxed_plugin_block(
            tid, sandbox_index, left.data(), right.data(), static_cast<uint32_t>(left.size()));
    }
    
    rust::Vec<uint8_t> process_sandboxed_plugin_midi_block(
        uint32_t tid, uint32_t sandbox_index, uint32_t frames,
        rust::Slice<const uint8_t> midi_data) const {
        rust::Vec<uint8_t> result;
        if (!m_engine || midi_data.empty()) return result;
        const auto output = m_engine->process_sandboxed_plugin_midi_block(
            tid, sandbox_index, frames, midi_data.data(), static_cast<uint32_t>(midi_data.size()));
        for (const auto byte : output) result.push_back(byte);
        return result;
    }
    rust::String get_last_sandbox_failure_text(uint32_t trackId) const {
        return rust::String(m_engine ? m_engine->get_last_sandbox_failure_text(trackId)
                                     : "engine-unavailable");
    }
    rust::Vec<rust::String> get_sandbox_plugin_paths() const {
        rust::Vec<rust::String> result;
        if (!m_engine) return result;
        for (const auto& path : m_engine->get_sandbox_plugin_paths())
            result.push_back(rust::String(path));
        return result;
    }
    rust::Vec<uint8_t> get_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex) const {
        rust::Vec<uint8_t> result;
        if (!m_engine) return result;
        for (const auto byte : m_engine->get_sandbox_plugin_state(trackId, sandboxIndex))
            result.push_back(byte);
        return result;
    }
    bool set_sandbox_plugin_state(uint32_t trackId, uint32_t sandboxIndex,
                                  rust::Slice<const uint8_t> state) const {
        if (!m_engine || state.size() > Hirari::Core::Plugins::SandboxProtocol::kMaxStateBytes)
            return false;
        std::vector<uint8_t> bytes(state.begin(), state.end());
        return m_engine && m_engine->set_sandbox_plugin_state(trackId, sandboxIndex, bytes);
    }








    rust::String get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
        return rust::String(m_engine ? m_engine->get_sandbox_plugin_state_error_text(trackId, sandboxIndex)
                                     : "state-unavailable");
    }
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


    rust::Vec<uint8_t> get_video_frame() const;

    bool load_video(rust::Str path) const {
        return m_engine && m_engine->load_video(std::string_view(path.data(), path.size()));
    }
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
    uint64_t request_project_layout_snapshot() const {
        if (!m_engine) return 0;
        auto* context = new (std::nothrow) ProjectLayoutSnapshotContext{m_engine};
        if (!context) return 0;
        return hirari_layout_snapshot_request(
            m_layoutSnapshotOwnerId, context, &computeProjectLayoutSnapshot, &readProjectLayoutSnapshot,
            &destroyProjectLayoutResult, &destroyProjectLayoutContext,
            &cancelProjectLayoutSnapshot, &isProjectLayoutSnapshotCancelled);
    }
    rust::String poll_project_layout_snapshot(uint64_t requestId) const {
        if (requestId == 0) return rust::String("ERROR");
        std::string result;
        const uint8_t status = hirari_layout_snapshot_poll(
            requestId, m_layoutSnapshotOwnerId, &result,
            +[](void* opaque, const uint8_t* bytes, size_t size) noexcept {
                auto* output = static_cast<std::string*>(opaque);
                if (!output || (!bytes && size != 0)) return;
                try { output->assign(reinterpret_cast<const char*>(bytes), size); }
                catch (...) { output->clear(); }
            });
        if (status == 0) return rust::String();
        return rust::String(status == 1 ? result : "ERROR");
    }
    bool cancel_project_layout_snapshot(uint64_t requestId) const {
        return requestId != 0 && hirari_layout_snapshot_cancel(requestId, m_layoutSnapshotOwnerId);
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
