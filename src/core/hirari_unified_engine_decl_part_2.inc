    // Transport
    bool is_playing() const;
    float get_dsp_load() const noexcept { return m_dspLoad.load(std::memory_order_relaxed); }
    bool audio_range_overflowed() const noexcept {
        return m_audioRangeOverflow.load(std::memory_order_acquire);
    }
    uint32_t get_block_size() const noexcept;
    float get_latency_ms() const noexcept;
    float get_track_latency_ms(uint32_t tid) const noexcept;
    float get_track_pdc_compensation_ms(uint32_t tid) const noexcept;
    bool set_low_latency_mode(bool active) noexcept;
    bool low_latency_mode() const noexcept;
    void set_playing(bool playing);
    uint64_t get_playhead() const;
    void set_playhead(uint64_t pos);
    double samples_to_beats(uint64_t samples) const;
    uint64_t beats_to_samples(double beats) const;
    // Tempo and time-signature editing moved to Rust; only these two reads
    // survive here, for the driver config and the native contract test.
    float get_tempo() const;
    bool set_tempo(float bpm);
    bool add_marker(uint64_t sample, std::string_view name, uint32_t color) { return m_arrangement.upsertMarker({sample, std::string(name), color}); }
    bool remove_marker(uint64_t sample) { return m_arrangement.removeMarker(sample); }
    std::vector<uint64_t> marker_positions() const { std::vector<uint64_t> out; for (const auto& m : m_arrangement.markers()) out.push_back(m.sample); return out; }
    bool set_arranger_parts(std::vector<::Hirari::Core::Composition::ArrangerPart> parts) { return m_arrangement.setArrangerParts(std::move(parts)); }
    std::vector<::Hirari::Core::Composition::ArrangerPart> arranger_parts() const { return m_arrangement.arrangerParts(); }
    void tap_tempo(uint64_t timestamp_ms) noexcept {
        hirari_tap_tempo_tap(m_tapTempo, timestamp_ms);
        const double bpm = hirari_tap_tempo_bpm(m_tapTempo);
        if (bpm > 0.0) hirari_tempo_map_set_current_bpm(m_tempoState, static_cast<float>(bpm));
    }
    void clear_tap_tempo() noexcept { hirari_tap_tempo_clear(m_tapTempo); }
    double tapped_tempo() const noexcept { return hirari_tap_tempo_bpm(m_tapTempo); }
    void midi_clock_tick(uint64_t timestamp) noexcept { hirari_external_sync_tick(m_externalSync, timestamp); }
    uint64_t midi_clock_ticks() const noexcept { return hirari_external_sync_ticks(m_externalSync); }
    uint64_t midi_clock_last_tick() const noexcept { return hirari_external_sync_last_tick(m_externalSync); }
    double midi_clock_rate() const noexcept { return hirari_external_sync_rate(m_externalSync); }
    void set_midi_clock_rate(double bpm) noexcept { hirari_external_sync_set_rate(m_externalSync, bpm); }
    bool set_master_gain(float value);
    void* control_room_state() const noexcept { return m_controlRoomState; }
    // Rust owns the tempo map, the undo history and the diagnostic ring; these
    // hand those addresses to Rust callers so an edit can work on the very
    // objects this engine's render path and history read, instead of crossing
    // into C++ only to be sent straight back.
    void* tempo_state() const noexcept { return m_tempoState; }
    void* undo_state() const noexcept { return m_undoManager.rustState(); }
    void* log_state() const noexcept {
        return ::Hirari::Core::Diagnostics::LogBuffer::state();
    }
    bool bus_track_exists(uint32_t busTrackId) const {
        if (busTrackId == 0) return false;
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        return std::find_if(m_tracks.begin(), m_tracks.end(), [busTrackId](const auto& track) {
            return track && track->getId() == busTrackId &&
                   dynamic_cast<BusTrack*>(track.get()) != nullptr;
        }) != m_tracks.end();
    }
    std::string plugin_compatibility_snapshot_json() const;
    bool add_automation_lane(uint32_t id) { return hirari_automation_lane_group_add(m_automationLanes, id); }
    bool set_automation_lane_linked(uint32_t id, bool linked) { return hirari_automation_lane_group_link(m_automationLanes, id, linked); }
    bool set_automation_lane_protected(uint32_t id, bool protectedLane) { return hirari_automation_lane_group_protect(m_automationLanes, id, protectedLane); }
    bool preview_automation_lane(uint32_t id, float value) { return hirari_automation_lane_group_preview(m_automationLanes, id, value); }
    float get_master_gain() const noexcept;
    float get_track_volume(uint32_t tid) const;
    bool set_track_solo_for_offline_render(uint32_t tid, bool solo);
    // Offline render target selection is control-side state. When active,
    // the audio graph keeps only the target and its upstream route closure,
    // then publishes the target output to the master buffer.
    bool set_offline_render_target(uint32_t tid);
    void clear_offline_render_target() noexcept;
    void set_offline_render_tail_seconds(float seconds) noexcept;
    void set_offline_render_options(bool pre_fader, bool include_inserts) noexcept;
    bool set_phase_invert(uint32_t tid, bool inverted);
    bool set_track_delay_samples(uint32_t tid, uint32_t samples);
    uint32_t get_track_delay_samples(uint32_t tid) const;
    bool set_track_armed(uint32_t tid, bool armed);
    bool set_track_input_monitor(uint32_t tid, bool enabled);
    bool set_track_input_monitor_channels(uint32_t tid, bool enabled,
                                          uint32_t leftChannel, uint32_t rightChannel);
    double get_sample_rate() const { return m_sampleRate.load(std::memory_order_relaxed); }
    uint64_t get_audio_config_generation() const noexcept {
        return m_audioConfigGeneration.load(std::memory_order_acquire);
    }
    void set_loop(bool enabled);
    bool set_cycle_range(uint64_t start_sample, uint64_t end_sample, bool enabled);
    bool is_loop_enabled() const noexcept;
    uint64_t cycle_start() const noexcept;
    uint64_t cycle_end() const noexcept;
    void set_metronome_enabled(bool enabled) noexcept;
    bool is_metronome_enabled() const noexcept;
    void set_test_tone(bool enabled);
    bool bind_midi_cc_to_macro(uint8_t channel, uint8_t cc, uint32_t macro_index,
                               float minimum, float maximum, float curve,
                               bool pickup) noexcept;
    bool bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller, uint32_t macro_index,
                                 float minimum, float maximum, float curve,
                                 bool pickup) noexcept;
    void handle_midi_cc(uint8_t channel, uint8_t cc, uint8_t value) noexcept;
    void handle_midi_cc14(uint8_t channel, uint16_t controller, uint16_t value) noexcept;
    // Preview sampler bridge. The sample is replaced off the audio thread;
    // the callback only reads an immutable shared snapshot.
    void set_preview_sample(const float* samples, size_t count, double sourceRate);
    void trigger_preview_sample();
    void clear_preview_sample();
