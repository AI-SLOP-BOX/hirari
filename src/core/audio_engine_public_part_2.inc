    uint32_t add_track(uint32_t type) const { return m_engine ? m_engine->add_track(type) : 0; }
    bool add_vca_group(uint32_t groupId, float gain) const { return m_engine && m_engine->add_vca_group(groupId, gain); }
    bool assign_track_to_vca(uint32_t trackId, uint32_t groupId) const { return m_engine && m_engine->assign_track_to_vca(trackId, groupId); }
    bool set_vca_group_gain(uint32_t groupId, float gain) const { return m_engine && m_engine->set_vca_group_gain(groupId, gain); }
    float get_vca_track_gain(uint32_t trackId) const { return m_engine ? m_engine->get_vca_track_gain(trackId) : 1.0f; }
    rust::String get_vca_snapshot_json() const { return m_engine ? m_engine->get_vca_snapshot_json() : rust::String("[]"); }
    void clear_vca_groups() const { if (m_engine) m_engine->clear_vca_groups(); }
    bool remove_track(uint32_t id) const { return m_engine && m_engine->remove_track(id); }
    bool set_track_name(uint32_t id, rust::Str name) const { return m_engine && m_engine->set_track_name(id, std::string_view(name.data(), name.size())); }
    void new_project() const { if (m_engine) m_engine->new_project(); }
    bool set_low_latency_mode(bool active) const { return m_engine && m_engine->set_low_latency_mode(active); }
    bool low_latency_mode() const { return m_engine && m_engine->low_latency_mode(); }
    uint32_t duplicate_track(uint32_t id) const { return m_engine ? m_engine->duplicate_track(id) : 0; }
    bool freeze_track(uint32_t tid, uint64_t totalSamples, uint32_t sampleRate) const {
        return m_engine && m_engine->freeze_track(tid, totalSamples, sampleRate);
    }
    bool freeze_track_to_file(uint32_t tid, rust::Str path, uint64_t totalSamples, uint32_t sampleRate) const {
        return m_engine && m_engine->freeze_track_to_file(
            tid, std::string_view(path.data(), path.size()), totalSamples, sampleRate);
    }
    bool set_track_freeze_cache_path(uint32_t tid, rust::Str path) const {
        return m_engine && m_engine->set_track_freeze_cache_path(
            tid, std::string_view(path.data(), path.size()));
    }
    bool restore_track_freeze_from_file(uint32_t tid, rust::Str path,
                                        uint64_t totalSamples, uint32_t sampleRate) const {
        return m_engine && m_engine->restore_track_freeze_from_file(
            tid, std::string_view(path.data(), path.size()), totalSamples, sampleRate);
    }
    bool unfreeze_track(uint32_t tid) const {
        return m_engine && m_engine->unfreeze_track(tid);
    }
    bool is_track_frozen(uint32_t tid) const {
        return m_engine && m_engine->is_track_frozen(tid);
    }
    bool freeze_track_to_project_end(uint32_t tid, uint32_t sampleRate) const {
        return m_engine && m_engine->freeze_track_to_project_end(tid, sampleRate);
    }
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
    uint8_t complete_region_import(uint64_t ticket) const {
        return m_engine ? m_engine->complete_region_import(ticket) : 3;
    }
    bool replace_region_audio(uint32_t tid, uint32_t rid, rust::Str path) const {
        return m_engine && m_engine->replace_region_audio(tid, rid, std::string_view(path.data(), path.size()));
    }
    
    bool add_plugin(uint32_t tid, uint32_t pluginType) const { return m_engine && m_engine->add_plugin(tid, pluginType); }
    rust::Vec<uint8_t> get_plugin_state(uint32_t tid, uint32_t pluginIndex) const {
        rust::Vec<uint8_t> out;
        if (!m_engine) return out;
        for (const auto byte : m_engine->get_plugin_state(tid, pluginIndex)) out.push_back(byte);
        return out;
    }
    bool set_plugin_state(uint32_t tid, uint32_t pluginIndex,
                          rust::Slice<const uint8_t> state) const {
        if (!m_engine) return false;
        std::vector<uint8_t> bytes;
        bytes.reserve(state.size());
        for (const auto byte : state) bytes.push_back(byte);
        return m_engine->set_plugin_state(tid, pluginIndex, bytes);
    }
    bool remove_plugin(uint32_t tid, uint32_t pluginIndex) const { return m_engine && m_engine->remove_plugin(tid, pluginIndex); }
    bool move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) const { return m_engine && m_engine->move_plugin(tid, fromIndex, toIndex); }
    bool add_sandboxed_plugin(uint32_t tid, rust::Str path) const {
        return m_engine && m_engine->add_sandboxed_plugin(tid, std::string_view(path.data(), path.size()));
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
        rust::Vec<uint32_t> result;
        if (!m_engine) return result;
        result.push_back(Plugins::SandboxProtocol::kStatusHeaderV9);
        if (!m_engine) return result;
        for (const auto& status : m_engine->get_sandbox_statuses()) {
            result.push_back(status.trackId);
            result.push_back(status.pluginIndex);
            result.push_back(status.alive ? 1u : 0u);
            result.push_back(status.canRetry ? 1u : 0u);
            result.push_back(status.failure);
            result.push_back(status.droppedOutputMidi);
            result.push_back(status.mailboxOverruns);
            result.push_back(status.inputMidiTruncations);
            result.push_back(status.recoveryMode);
        }
        return result;
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
    bool set_plugin_automation(uint32_t tid, uint32_t pluginIndex,
                               uint32_t parameterId, rust::Vec<double> packedPoints) const {
        return m_engine && hirari_automation_edit_plugin(
                               m_engine.get(), tid, pluginIndex, parameterId,
                               packedPoints.data(), packedPoints.size());
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
    bool save_plugin_preset(uint32_t tid, uint32_t pluginIndex, rust::Str path) const {
        return m_engine && m_engine->save_plugin_preset(tid, pluginIndex, std::string(path));
    }
    bool load_plugin_preset(uint32_t tid, uint32_t pluginIndex, rust::Str path) const {
        return m_engine && m_engine->load_plugin_preset(tid, pluginIndex, std::string(path));
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
    void set_macro_value(uint32_t macroIndex, float value) const {
        if (m_engine) m_engine->set_macro_value(macroIndex, value);
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
    bool set_route(uint32_t sourceId, uint32_t destId, bool enabled) const {
        return m_engine && m_engine->set_route(sourceId, destId, enabled);
    }
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
    rust::String bounce_project_diagnostic_json(rust::Str path, uint32_t format) const {
        if (!m_engine) return rust::String("{\"code\":\"engine_unavailable\",\"retryable\":true}");
        const std::string_view output(path.data(), path.size());
        if (output.empty() || output.size() < 4 || output.substr(output.size() - 4) != ".wav") {
            return rust::String("{\"code\":\"invalid_render_path\",\"retryable\":false}");
        }
        if (format > 3) return rust::String("{\"code\":\"unsupported_render_format\",\"retryable\":false}");
        const bool ok = m_engine->bounce_project(std::string_view(path.data(), path.size()), format);
        return rust::String(ok ? "{\"ok\":true}" :
                            "{\"code\":\"bounce_failed\",\"retryable\":true}");
    }
    rust::String read_wav_diagnostic_json(rust::Str path, uint32_t format) const {
        const std::string input(path.data(), path.size());
        if (format > 1) {
            return rust::String("{\"ok\":false,\"code\":\"unsupported_wave_format\",\"retryable\":false}");
        }
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
    uint32_t get_undo_count() const {
        return m_engine ? hirari_undo_edit_undo_count(m_engine->undo_state()) : 0;
    }
    uint32_t get_redo_count() const {
        return m_engine ? hirari_undo_edit_redo_count(m_engine->undo_state()) : 0;
    }
    bool execute_vocal_remover(uint32_t tid) const { return m_engine && m_engine->execute_vocal_remover(tid); }
    bool set_articulation_map(uint32_t tid, uint32_t mapHash) const { return m_engine && m_engine->set_articulation_map(tid, mapHash); }
    bool set_expression_map(uint32_t tid, rust::Vec<uint32_t> packed, rust::String json,
                            uint32_t articulationId) const {
        if (!m_engine) return false;
        std::vector<uint32_t> native(packed.begin(), packed.end());
        return m_engine->set_expression_map(tid, native, std::string(json.data(), json.size()), articulationId);
    }
    bool set_expression_map_pro(uint32_t tid, rust::String json) const {
        return m_engine && m_engine->set_expression_map_pro(tid, std::string(json.data(), json.size()));
    }
    bool set_track_eq(uint32_t tid, float lowBoostDb, float lowCutDb,
                      float highBoostDb, float highCutDb) const {
        return m_engine && m_engine->set_track_eq(tid, lowBoostDb, lowCutDb, highBoostDb, highCutDb);
    }
    float get_track_correlation(uint32_t tid) const { return m_engine ? m_engine->get_track_correlation(tid) : 0.0f; }
    uint32_t get_track_count() const { return m_engine ? m_engine->get_track_count() : 0; }
    bool execute_auto_mixing() const { return m_engine && m_engine->execute_auto_mixing(); }
    bool execute_auto_arrangement() const { return m_engine && m_engine->execute_auto_arrangement(); }
    bool set_project_scale(int32_t root, int32_t type) const {
        return m_engine && m_engine->set_project_scale(root, type);
    }
    void undo() const {
        if (m_engine) {
            hirari_undo_edit_undo(m_engine->undo_state(), m_engine->log_state(),
                                  m_engine.get());
        }
    }
    void redo() const {
        if (m_engine) {
            hirari_undo_edit_redo(m_engine->undo_state(), m_engine->log_state(),
                                  m_engine.get());
        }
    }
    bool is_undo_transaction_active() const {
        return m_engine && hirari_undo_edit_transaction_active(m_engine->undo_state());
    }
    void begin_undo_transaction(rust::Str name) const {
        if (m_engine) {
            hirari_undo_edit_begin_transaction(
                m_engine->undo_state(), reinterpret_cast<const uint8_t*>(name.data()),
                name.size());
        }
    }
    bool end_undo_transaction() const {
        return m_engine && hirari_undo_edit_end_transaction(m_engine->undo_state());
    }
    bool abort_undo_transaction() const {
        return m_engine &&
            hirari_undo_edit_abort_transaction(m_engine->undo_state(), m_engine.get());
    }

    bool move_region(uint32_t tid, uint32_t rid, double startBeat) const { return m_engine && m_engine->move_region(tid, rid, startBeat); }
    bool move_region_sync_group(uint32_t tid, uint32_t rid, double startBeat) const { return m_engine && m_engine->move_region_sync_group(tid, rid, startBeat); }
    bool split_region(uint32_t tid, uint32_t rid, double split) const { return m_engine && m_engine->split_region(tid, rid, split); }
    uint32_t split_region_with_right_id(uint32_t tid, uint32_t rid, double split) const { return m_engine ? m_engine->split_region_with_right_id(tid, rid, split) : 0; }
    bool remove_region(uint32_t tid, uint32_t rid) const { return m_engine && m_engine->remove_region(tid, rid); }
    uint32_t duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample) const { return m_engine ? m_engine->duplicate_region(tid, rid, startSample) : 0; }
    bool set_region_gain(uint32_t tid, uint32_t rid, float gain) const { return m_engine && m_engine->set_region_gain(tid, rid, gain); }
    bool set_region_muted(uint32_t tid, uint32_t rid, bool muted) const { return m_engine && m_engine->set_region_muted(tid, rid, muted); }
    bool set_region_comp_ranges(uint32_t tid, uint32_t rid,
                               rust::Slice<const uint64_t> packedRanges, bool managed) const {
        return m_engine && m_engine->set_region_comp_ranges(tid, rid, packedRanges, managed);
    }
    bool set_region_fades(uint32_t tid, uint32_t rid, float fadeIn, float fadeOut) const { return m_engine && m_engine->set_region_fades(tid, rid, fadeIn, fadeOut); }
    bool set_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start, uint64_t end,
                               float gain, uint64_t fadeIn, uint64_t fadeOut) const {
        return m_engine && hirari_region_edit_range_edit_set(m_engine.get(), tid, rid, start,
                                                            end, gain, fadeIn, fadeOut);
    }
    bool clear_region_range_edits(uint32_t tid, uint32_t rid) const {
        return m_engine && hirari_region_edit_range_edits_clear(m_engine.get(), tid, rid);
    }
    bool set_region_reverse(uint32_t tid, uint32_t rid, bool reverse) const { return m_engine && m_engine->set_region_reverse(tid, rid, reverse); }
    bool set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio) const { return m_engine && m_engine->set_region_warp_ratio(tid, rid, ratio); }
    bool set_region_pitch_preserve_warp(uint32_t tid, uint32_t rid, bool enabled) const { return m_engine && m_engine->set_region_pitch_preserve_warp(tid, rid, enabled); }
    bool set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones) const { return m_engine && m_engine->set_region_pitch_semitones(tid, rid, semitones); }
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
    bool clear_region_audio_note_segments(uint32_t tid, uint32_t rid) const {
        return m_engine && m_engine->clear_region_audio_note_segments(tid, rid);
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
    bool finalize_region_audio_note_analysis(uint32_t tid, uint32_t rid) const {
        return m_engine && m_engine->finalize_region_audio_note_analysis(tid, rid);
    }
    bool set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count) const { return m_engine && m_engine->set_region_loop_count(tid, rid, count); }
    bool set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group) const { return m_engine && m_engine->set_region_sync_group(tid, rid, group); }
    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, rust::Str operation, float parameter, bool enabled) const {
        return m_engine && m_engine->append_region_processing(tid, rid, stepId, std::string_view(operation.data(), operation.size()), parameter, enabled);
    }
    bool clear_region_processing(uint32_t tid, uint32_t rid) const { return m_engine && m_engine->clear_region_processing(tid, rid); }
    bool set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm) const { return m_engine && m_engine->set_region_trim(tid, rid, startNorm, endNorm); }
    void clear_midi_notes() const { if (m_engine) m_engine->clear_midi_notes(); }
    bool remove_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample) const { return m_engine && m_engine->remove_midi_notes_range(trackId, startSample, endSample); }
    bool transpose_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int32_t semitones) const { return m_engine && m_engine->transpose_midi_notes_range(trackId, startSample, endSample, semitones); }
    bool move_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int64_t deltaSamples) const { return m_engine && m_engine->move_midi_notes_range(trackId, startSample, endSample, deltaSamples); }
    void set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity, uint64_t startSample, uint64_t lengthSamples) const {
        if (m_engine) m_engine->set_midi_note(trackId, pitch, velocity, startSample, lengthSamples);
    }
    bool replace_midi_notes(rust::Vec<uint64_t> packed, bool recordUndo) const {
        if (!m_engine || packed.size() % 9 != 0 || packed.size() / 9 > 100000) return false;
        std::vector<::Hirari::Core::Engine::HirariUnifiedEngine::ScheduledMidiNote> notes;
        notes.reserve(packed.size() / 9);
        for (size_t i = 0; i < packed.size(); i += 9) {
            const uint64_t track = packed[i];
            const uint64_t pitch = packed[i + 1];
            const uint64_t velocity = packed[i + 2];
            const uint64_t midiChannel = packed[i + 3];
            if (track > UINT32_MAX || pitch > 127 || velocity == 0 || velocity > 127 ||
                midiChannel > 15 || packed[i + 5] == 0 || packed[i + 6] > 100 ||
                packed[i + 7] > UINT32_MAX || packed[i + 8] > 127 ||
                packed[i + 4] > UINT64_MAX - packed[i + 5]) return false;
            notes.push_back({static_cast<uint32_t>(track), static_cast<uint8_t>(pitch),
                static_cast<uint8_t>(velocity), static_cast<uint8_t>(midiChannel),
                static_cast<uint8_t>(packed[i + 8]),
                packed[i + 4], packed[i + 5], static_cast<uint8_t>(packed[i + 6]),
                static_cast<uint32_t>(packed[i + 7])});
        }
        return m_engine->replace_midi_notes(notes, recordUndo);
    }
    rust::Vec<uint64_t> midi_notes_snapshot() const {
        rust::Vec<uint64_t> packed;
        if (!m_engine) return packed;
        const auto notes = m_engine->midi_notes_snapshot();
        packed.reserve(notes.size() * 9);
        for (const auto& note : notes) {
            packed.push_back(note.trackId);
            packed.push_back(note.pitch);
            packed.push_back(note.velocity);
            packed.push_back(note.midiChannel);
            packed.push_back(note.startSample);
            packed.push_back(note.lengthSamples);
            packed.push_back(note.probability);
            packed.push_back(note.regionId);
            packed.push_back(note.articulationId);
        }
        return packed;
    }
    bool set_spatial_position(uint32_t tid, float x, float y, float z) const { return m_engine && m_engine->set_spatial_position(tid, x, y, z); }
    bool set_hrtf_kernel(uint32_t tid, rust::Vec<float> left,
                         rust::Vec<float> right) const {
        if (!m_engine || left.empty() || left.size() != right.size()) return false;
        std::vector<float> left_copy(left.begin(), left.end());
        std::vector<float> right_copy(right.begin(), right.end());
        return m_engine->set_hrtf_kernel(tid, left_copy, right_copy);
    }
    bool clear_hrtf_kernel(uint32_t tid) const {
        return m_engine && m_engine->clear_hrtf_kernel(tid);
    }
    bool set_automation_record_mode(uint32_t mode) const {
        return m_engine && m_engine->set_automation_record_mode(mode);
    }
    rust::String plugin_compatibility_snapshot_json() const { return m_engine ? rust::String(m_engine->plugin_compatibility_snapshot_json()) : rust::String("[]"); }
    bool set_automation_data(uint32_t tid, uint32_t paramId,
                             rust::Vec<double> packedPoints) const {
        if (!m_engine) return false;
        const int32_t kind = paramId == 0 ? kHirariAutomationVolume : kHirariAutomationPan;
        return hirari_automation_edit_track(m_engine.get(), tid, kind, paramId,
                                            packedPoints.data(), packedPoints.size());
    }
    bool set_track_delay_automation(uint32_t tid, rust::Vec<double> packedPoints) const {
        return m_engine && hirari_automation_edit_track(
                               m_engine.get(), tid, kHirariAutomationTrackDelay, 0,
                               packedPoints.data(), packedPoints.size());
    }
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
    rust::Vec<float> get_region_audio_interleaved(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->get_region_audio_interleaved(tid, rid, 16u * 1024u * 1024u)
                        : rust::Vec<float>{};
    }
    rust::Vec<float> get_region_audio_interleaved_range(uint32_t tid, uint32_t rid,
                                                        uint64_t startFrame,
                                                        uint32_t frames,
                                                        uint32_t channels) const {
        return m_engine ? m_engine->get_region_audio_interleaved_range(
            tid, rid, startFrame, frames, channels) : rust::Vec<float>{};
    }
    uint64_t get_region_source_frame_count(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->get_region_source_frame_count(tid, rid) : 0;
    }
    double get_region_sample_rate(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->get_region_sample_rate(tid, rid) : 0.0;
    }
    uint32_t get_region_channel_count(uint32_t tid, uint32_t rid) const {
        return m_engine ? m_engine->get_region_channel_count(tid, rid) : 0;
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

    // Command Dispatch
    bool push_command(CommandType type, uint32_t tid, float val, uint64_t ts,
                      uint64_t expectedProjectGeneration,
                      uint64_t expectedAudioGeneration) const {
        return m_engine && m_engine->push_command(type, tid, val, ts,
                                                   expectedProjectGeneration,
                                                   expectedAudioGeneration);
    }
    uint8_t get_last_command_error() const noexcept {
        return m_engine ? m_engine->get_last_command_error() : 1u;
    }
    
    // Core Access for telemetry shims
    const ::Hirari::Core::Engine::HirariUnifiedEngine& get_core() const { return *m_engine; }
