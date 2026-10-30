    bool add_vca_group(uint32_t groupId, float gain) const { return m_engine && m_engine->add_vca_group(groupId, gain); }
    bool assign_track_to_vca(uint32_t trackId, uint32_t groupId) const { return m_engine && m_engine->assign_track_to_vca(trackId, groupId); }
    bool set_vca_group_gain(uint32_t groupId, float gain) const { return m_engine && m_engine->set_vca_group_gain(groupId, gain); }
    rust::String get_vca_snapshot_json() const { return m_engine ? m_engine->get_vca_snapshot_json() : rust::String("[]"); }
    void new_project() const { if (m_engine) m_engine->new_project(); }
    bool set_low_latency_mode(bool active) const { return m_engine && m_engine->set_low_latency_mode(active); }
    bool low_latency_mode() const { return m_engine && m_engine->low_latency_mode(); }
    bool region_table_mirror_matches(uint32_t tid) const {
        return m_engine && m_engine->region_table_mirror_matches(tid);
    }
    bool add_region(uint32_t tid, rust::Str path, double startBeat) const {
        return m_engine && m_engine->add_region(tid, std::string_view(path.data(), path.size()), startBeat);
    }
    uint64_t queue_region_import(uint32_t tid, rust::Str path, uint64_t startSample) const {
        return m_engine ? m_engine->queue_region_import(
            tid, std::string_view(path.data(), path.size()), startSample) : 0;
    }
    
    bool move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) const { return m_engine && m_engine->move_plugin(tid, fromIndex, toIndex); }
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
    uint32_t maintain_sandboxed_plugins(bool autoRestart) const {
        return m_engine ? m_engine->maintain_sandboxed_plugins(autoRestart) : 0;
    }
    uint32_t take_watchdog_trips() const {
        return m_engine ? m_engine->take_watchdog_trips() : 0;
    }
    uint64_t non_finite_plugin_samples() const {
        return m_engine ? m_engine->non_finite_plugin_samples() : 0;
    }
    bool retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) const {
        return m_engine && m_engine->retry_sandboxed_plugin(trackId, sandboxIndex);
    }
    bool restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) const {
        return m_engine && m_engine->restart_sandboxed_plugin(trackId, sandboxIndex);
    }
    rust::Vec<uint32_t> get_sandbox_statuses() const {
        if (!m_engine) return {};
        // Rust enumerates the tracks through the maintenance host and owns the
        // packed status protocol.
        return ::Hirari::Core::Bridge::sandbox_statuses_packed(
            reinterpret_cast<size_t>(m_engine.get()));
    }
    uint32_t get_last_sandbox_failure(uint32_t trackId) const {
        return m_engine ? m_engine->get_last_sandbox_failure(trackId) : 0u;
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
    uint8_t get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const {
        return m_engine ? m_engine->get_sandbox_plugin_state_error(trackId, sandboxIndex) : 5u;
    }
    rust::String get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
        return rust::String(m_engine ? m_engine->get_sandbox_plugin_state_error_text(trackId, sandboxIndex)
                                     : "state-unavailable");
    }
    bool set_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                              uint32_t parameterId, float value) const {
        return m_engine && m_engine->set_plugin_parameter(tid, pluginIndex, parameterId, value);
    }
    bool set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId, float value) const {
        return m_engine && m_engine->set_plugin_parameter_without_undo(
            tid, pluginIndex, parameterId, value);
    }
    float get_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                               uint32_t parameterId) const {
        return m_engine ? m_engine->get_plugin_parameter(tid, pluginIndex, parameterId) : 0.0f;
    }
    uint32_t get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const {
        return m_engine ? m_engine->get_plugin_parameter_count(tid, pluginIndex) : 0;
    }
    rust::String get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                           uint32_t parameterId) const {
        return rust::String(m_engine ? m_engine->get_plugin_parameter_name(tid, pluginIndex, parameterId) : "");
    }
    bool set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) const {
        return m_engine && m_engine->set_plugin_bypass(tid, pluginIndex, bypassed);
    }
    bool get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
        return m_engine && m_engine->get_plugin_bypass(tid, pluginIndex);
    }
    float get_track_latency_ms(uint32_t tid) const {
        return m_engine ? m_engine->get_track_latency_ms(tid) : 0.0f;
    }
    float get_track_pdc_compensation_ms(uint32_t tid) const {
        return m_engine ? m_engine->get_track_pdc_compensation_ms(tid) : 0.0f;
    }
    bool bind_midi_cc_to_macro(uint8_t channel, uint8_t cc, uint32_t macroIndex,
                               float minimum, float maximum, float curve,
                               bool pickup) const {
        return m_engine && m_engine->bind_midi_cc_to_macro(
            channel, cc, macroIndex, minimum, maximum, curve, pickup);
    }
    void handle_midi_cc(uint8_t channel, uint8_t cc, uint8_t value) const noexcept {
        if (m_engine) m_engine->handle_midi_cc(channel, cc, value);
    }
    void handle_midi_cc14(uint8_t channel, uint16_t controller, uint16_t value) const noexcept {
        if (m_engine) m_engine->handle_midi_cc14(channel, controller, value);
    }
    bool bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller, uint32_t macro_index,
                                 float minimum, float maximum, float curve,
                                 bool pickup) const noexcept {
        return m_engine && m_engine->bind_midi_cc14_to_macro(
            channel, controller, macro_index, minimum, maximum, curve, pickup);
    }
    void set_preview_synth_engine(uint32_t engine) const { if (m_engine) m_engine->set_preview_synth_engine(engine); }
    bool set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled) const {
        return m_engine && m_engine->set_route_gain(sourceId, destId, gain, enabled);
    }
    bool set_send_route(uint32_t sourceId, uint32_t destId, float gain,
                        bool preFader, bool enabled) const {
        return m_engine && m_engine->set_send_route(sourceId, destId, gain, preFader, enabled);
    }
    bool set_feedback_route(uint32_t sourceId, uint32_t destId, float gain, bool enabled) const {
        return m_engine && m_engine->set_feedback_route(sourceId, destId, gain, enabled);
    }
    bool set_sidechain_link(uint32_t sourceId, uint32_t destId,
                            uint32_t pluginIndex, uint32_t tapPoint, bool enabled) const {
        return m_engine && m_engine->set_sidechain_link(sourceId, destId, pluginIndex, tapPoint, enabled);
    }
    bool has_sidechain_link(uint32_t sourceId, uint32_t destId, uint32_t pluginIndex) const {
        return m_engine && m_engine->has_sidechain_link(sourceId, destId, pluginIndex);
    }
    bool save_project(rust::Str path) const { return m_engine && m_engine->saveProject(std::string_view(path.data(), path.size())); }
    bool load_project(rust::Str path) const { return m_engine && m_engine->loadProject(std::string_view(path.data(), path.size())); }
    bool bounce_project(rust::Str path, uint32_t format) const { return m_engine && m_engine->bounce_project(std::string_view(path.data(), path.size()), format); }
    rust::String read_wav_diagnostic_json(rust::Str path, uint32_t format) const {
        // The supported-format gate and its diagnostic code are Rust-owned.
        const std::string input(path.data(), path.size());
        return rust::String(::Hirari::IO::WavLoader::loadDiagnosticJson(input, format == 1));
    }
    bool bounce_project_async(rust::Str path, uint32_t format) const { return m_engine && m_engine->bounce_project_async(std::string_view(path.data(), path.size()), format); }
    float get_bounce_progress() const noexcept { return m_engine ? m_engine->get_bounce_progress() : 0.0f; }
    uint32_t get_bounce_state() const noexcept { return m_engine ? m_engine->get_bounce_state() : 0; }
    bool has_plugin_native_editor(uint32_t trackId, uint32_t pluginIndex) const noexcept {
        return m_engine && m_engine->has_plugin_native_editor(trackId, pluginIndex);
    }
    uint64_t open_plugin_native_editor(uint32_t trackId, uint32_t pluginIndex,
                                       uint64_t parent) const {
        if (!m_engine || !m_engine->has_plugin_native_editor(trackId, pluginIndex)) return 0;
        uint64_t session = 0;
        if (m_nativeEditorHost.open(trackId, pluginIndex, static_cast<uintptr_t>(parent), session))
            return session;
        // When no platform wrapper is bound, let a direct VST3/AU processor
        // attach its native view to the supplied parent surface. This keeps
        // the SDK-enabled path usable from the same bridge instead of
        // silently falling back to parameter controls.
        session = m_engine->open_plugin_native_editor(
            trackId, pluginIndex, static_cast<uintptr_t>(parent));
        if (session != 0) m_nativeEditorHost.adopt(trackId, pluginIndex, session);
        return session;
    }
    bool close_plugin_native_editor(uint32_t trackId, uint32_t pluginIndex) const {
        const uint64_t session = m_nativeEditorHost.session(trackId, pluginIndex);
        const bool processorClosed = session != 0 && m_engine &&
            m_engine->close_plugin_native_editor(trackId, pluginIndex, session);
        const bool wrapperClosed = m_nativeEditorHost.close(trackId, pluginIndex);
        if (processorClosed && !wrapperClosed) m_nativeEditorHost.forget(trackId, pluginIndex);
        return wrapperClosed || processorClosed;
    }
    bool plugin_native_editor_embedded(uint32_t trackId, uint32_t pluginIndex) const {
        return m_nativeEditorHost.embedded(trackId, pluginIndex);
    }
    bool cancel_bounce() const noexcept { return m_engine && m_engine->cancel_bounce(); }
    bool pause_bounce() const noexcept { return m_engine && m_engine->pause_bounce(); }
    bool resume_bounce() const noexcept { return m_engine && m_engine->resume_bounce(); }
    bool execute_vocal_remover(uint32_t tid) const { return m_engine && m_engine->execute_vocal_remover(tid); }
    bool set_articulation_map(uint32_t tid, uint32_t mapHash) const { return m_engine && m_engine->set_articulation_map(tid, mapHash); }
    bool set_expression_map(uint32_t tid, rust::Vec<uint32_t> packed, rust::String json,
                            uint32_t articulationId) const {
        if (!m_engine) return false;
        std::vector<uint32_t> native(packed.begin(), packed.end());
        return m_engine->set_expression_map(tid, native, std::string(json.data(), json.size()), articulationId);
    }
    bool set_track_eq(uint32_t tid, float lowBoostDb, float lowCutDb,
                      float highBoostDb, float highCutDb) const {
        return m_engine && m_engine->set_track_eq(tid, lowBoostDb, lowCutDb, highBoostDb, highCutDb);
    }
    float get_track_correlation(uint32_t tid) const { return m_engine ? m_engine->get_track_correlation(tid) : 0.0f; }
    bool execute_auto_mixing() const { return m_engine && m_engine->execute_auto_mixing(); }
    bool execute_auto_arrangement() const { return m_engine && m_engine->execute_auto_arrangement(); }
    bool set_project_scale(int32_t root, int32_t type) const {
        return m_engine && m_engine->set_project_scale(root, type);
    }

    bool split_region(uint32_t tid, uint32_t rid, double split) const { return m_engine && m_engine->split_region(tid, rid, split); }
    uint32_t split_region_with_right_id(uint32_t tid, uint32_t rid, double split) const { return m_engine ? m_engine->split_region_with_right_id(tid, rid, split) : 0; }
    bool set_region_pitch_preserve_warp(uint32_t tid, uint32_t rid, bool enabled) const { return m_engine && m_engine->set_region_pitch_preserve_warp(tid, rid, enabled); }
    bool set_region_audio_note_segment(uint32_t tid, uint32_t rid, double startSeconds,
                                       double endSeconds, double pitchOffsetCents,
                                       double formantOffsetCents) const {
        return m_engine && m_engine->set_region_audio_note_segment(
            tid, rid, startSeconds, endSeconds, pitchOffsetCents, formantOffsetCents);
    }
    bool set_region_audio_note_anchor(uint32_t tid, uint32_t rid, double segmentStartSeconds,
                                      double positionSeconds, double pitchCents,
                                      double formantCents) const {
        return m_engine && m_engine->set_region_audio_note_anchor(
            tid, rid, segmentStartSeconds, positionSeconds, pitchCents, formantCents);
    }
    bool set_region_audio_note_pitch_anchor(uint32_t tid, uint32_t rid,
                                            double segmentStartSeconds,
                                            double positionSeconds,
                                            double pitchCents) const {
        return m_engine && m_engine->set_region_audio_note_pitch_anchor(
            tid, rid, segmentStartSeconds, positionSeconds, pitchCents);
    }
    bool set_region_audio_note_formant_anchor(uint32_t tid, uint32_t rid,
                                              double segmentStartSeconds,
                                              double positionSeconds,
                                              double formantCents) const {
        return m_engine && m_engine->set_region_audio_note_formant_anchor(
            tid, rid, segmentStartSeconds, positionSeconds, formantCents);
    }
    bool move_region_audio_note_anchor(uint32_t tid, uint32_t rid,
                                       double segmentStartSeconds,
                                       double oldPositionSeconds,
                                       double newPositionSeconds,
                                       double valueCents,
                                       bool editFormant) const {
        return m_engine && m_engine->move_region_audio_note_anchor(
            tid, rid, segmentStartSeconds, oldPositionSeconds, newPositionSeconds,
            valueCents, editFormant);
    }
    bool warp_region_audio_note_segment(uint32_t tid, uint32_t rid, double segmentStart,
                                        double newStart, double newEnd) const {
        return m_engine && m_engine->warp_region_audio_note_segment(tid, rid, segmentStart, newStart, newEnd);
    }
    bool remove_region_audio_note_segment(uint32_t tid, uint32_t rid, double segmentStart) const {
        return m_engine && m_engine->remove_region_audio_note_segment(tid, rid, segmentStart);
    }

    bool start_region_audio_note_analysis(uint32_t tid, uint32_t rid, double sampleRate) const {
        return m_engine && m_engine->start_region_audio_note_analysis(tid, rid, sampleRate);
    }
    uint8_t region_audio_note_analysis_status(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->region_audio_note_analysis_status(tid, rid) : 0;
    }
    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, rust::Str operation, float parameter, bool enabled) const {
        return m_engine && m_engine->append_region_processing(tid, rid, stepId, std::string_view(operation.data(), operation.size()), parameter, enabled);
    }
    bool clear_region_processing(uint32_t tid, uint32_t rid) const { return m_engine && m_engine->clear_region_processing(tid, rid); }
    void clear_midi_notes() const { if (m_engine) m_engine->clear_midi_notes(); }
    bool remove_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample) const { return m_engine && m_engine->remove_midi_notes_range(trackId, startSample, endSample); }
    bool transpose_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int32_t semitones) const { return m_engine && m_engine->transpose_midi_notes_range(trackId, startSample, endSample, semitones); }
    bool move_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int64_t deltaSamples) const { return m_engine && m_engine->move_midi_notes_range(trackId, startSample, endSample, deltaSamples); }
    void set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity, uint64_t startSample, uint64_t lengthSamples) const {
        if (m_engine) m_engine->set_midi_note(trackId, pitch, velocity, startSample, lengthSamples);
    }
    bool replace_midi_notes(rust::Vec<uint64_t> packed, bool recordUndo) const {
        // Field ranges, the note cap and the start/length overflow check are
        // Rust-owned (hirari_midi_notes_unpack); a rejected vector writes nothing.
        if (!m_engine) return false;
        const size_t count = hirari_midi_notes_unpack(
            packed.data(), packed.size(), nullptr, 0);
        if (count == SIZE_MAX) return false;
        std::vector<::Hirari::Core::Engine::HirariUnifiedEngine::ScheduledMidiNote> notes(count);
        if (hirari_midi_notes_unpack(
                packed.data(), packed.size(),
                reinterpret_cast<HirariScheduledMidiNote*>(notes.data()), count) != count) {
            return false;
        }
        return m_engine->replace_midi_notes(notes, recordUndo);
    }
    rust::Vec<uint64_t> midi_notes_snapshot() const {
        rust::Vec<uint64_t> packed;
        if (!m_engine) return packed;
        const auto notes = m_engine->midi_notes_snapshot();
        const auto* raw = reinterpret_cast<const HirariScheduledMidiNote*>(notes.data());
        const size_t required = hirari_midi_notes_pack(raw, notes.size(), nullptr, 0);
        if (required == SIZE_MAX) return packed;
        packed.reserve(required);
        for (size_t i = 0; i < required; ++i) packed.push_back(0);
        (void)hirari_midi_notes_pack(raw, notes.size(), packed.data(), packed.size());
        return packed;
    }
    bool set_automation_record_mode(uint32_t mode) const {
        return m_engine && m_engine->set_automation_record_mode(mode);
    }
    rust::String plugin_compatibility_snapshot_json() const { return m_engine ? rust::String(m_engine->plugin_compatibility_snapshot_json()) : rust::String("[]"); }
    // Compatibility API: stop only this wrapper's device.  The underlying
    // HirariUnifiedEngine is a process-wide singleton and must not be shut down
    // when one Rust/FFI handle is dropped.
    void shutdown() const;

    // Explicit process-lifetime operation.  Call this only from the host's
    // final shutdown path, after all AudioEngine handles are gone.
    static void shutdown_process() noexcept;

    bool audio_range_overflowed() const noexcept {
        return m_engine && m_engine->audio_range_overflowed();
    }
    rust::Vec<uint8_t> get_video_frame() const;
    uint64_t get_video_frame_revision() const noexcept {
        return m_engine ? m_engine->get_video_frame_revision() : 0;
    }
    bool request_video_frame(double seconds) const { return m_engine && m_engine->request_video_frame(seconds); }
    bool load_video(rust::Str path) const {
        return m_engine && m_engine->load_video(std::string_view(path.data(), path.size()));
    }
    rust::Vec<float> get_region_waveform(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->get_region_waveform(tid, rid, 128) : rust::Vec<float>{};
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
    rust::Vec<float> poll_region_waveform(uint64_t request) const {
        rust::Vec<float> result;
        if (request == 0) return result;
        (void)hirari_waveform_snapshot_poll(
            request, &result,
            +[](void* opaque, const float* samples, size_t count) noexcept {
                auto* output = static_cast<rust::Vec<float>*>(opaque);
                if (!output || (!samples && count != 0)) return;
                for (size_t i = 0; i < count; ++i) output->push_back(samples[i]);
            });
        return result;
    }
    bool region_waveform_pending(uint64_t request) const noexcept {
        return request != 0 && hirari_waveform_snapshot_pending(request);
    }
    rust::Vec<float> get_region_audio_interleaved_range(uint32_t tid, uint32_t rid,
                                                        uint64_t startFrame,
                                                        uint32_t frames,
                                                        uint32_t channels) const {
        return m_engine ? m_engine->get_region_audio_interleaved_range(
            tid, rid, startFrame, frames, channels) : rust::Vec<float>{};
    }
    float get_cpu_total_v() const;
    rust::String get_project_layout_json() const;
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
    rust::String get_routing_snapshot_json() const;
    uint64_t get_project_generation() const {
        return m_engine ? m_engine->get_project_generation() : 0;
    }
    uint64_t get_project_layout_revision() const {
        return m_engine ? m_engine->get_project_layout_revision() : 0;
    }
    uint64_t get_project_state_revision() const {
        return m_engine ? m_engine->get_project_state_revision() : 0;
    }
    void mark_project_layout_changed() const {
        if (m_engine) m_engine->mark_project_layout_changed();
    }

    // Command Dispatch (admission lives in Rust; the engine keeps the queue)
    bool push_command(CommandType type, uint32_t tid, float val, uint64_t ts,
                      uint64_t expectedProjectGeneration,
                      uint64_t expectedAudioGeneration) const {
        return m_engine && hirari_engine_push_command(
            m_engine.get(), static_cast<uint32_t>(type), tid, val, ts,
            expectedProjectGeneration, expectedAudioGeneration);
    }
    uint8_t get_last_command_error() const noexcept {
        return m_engine ? hirari_engine_last_command_error(m_engine.get()) : 1u;
    }
    
    // Core Access for telemetry shims
    const ::Hirari::Core::Engine::HirariUnifiedEngine& get_core() const { return *m_engine; }
