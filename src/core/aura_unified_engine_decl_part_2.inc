    // Transport
    bool is_playing() const;
    float get_dsp_load() const noexcept { return m_dspLoad.load(std::memory_order_relaxed); }
    bool audio_range_overflowed() const noexcept {
        return m_audioRangeOverflow.load(std::memory_order_acquire);
    }
    uint32_t get_block_size() const noexcept;
    float get_latency_ms() const noexcept;
    float get_master_tail_ms() const noexcept;
    float get_track_latency_ms(uint32_t tid) const noexcept;
    float get_track_tail_ms(uint32_t tid) const noexcept;
    float get_track_pdc_compensation_ms(uint32_t tid) const noexcept;
    bool set_low_latency_mode(bool active) noexcept;
    bool low_latency_mode() const noexcept;
    bool set_tonal_scale(int32_t root, uint32_t scale_type) noexcept;
    bool is_note_in_tonal_scale(int32_t midi_note) const noexcept;
    int32_t tonal_root() const noexcept;
    uint32_t tonal_scale_type() const noexcept;
    void set_playing(bool playing);
    uint64_t get_playhead() const;
    void set_playhead(uint64_t pos);
    double get_playhead_beats() const;
    double samples_to_beats(uint64_t samples) const;
    uint64_t beats_to_samples(double beats) const;
    std::vector<double> get_tempo_events() const;
    std::vector<double> get_time_signature_events() const;
    void clear_time_signature_events();
    bool set_time_signature_event(double beat, uint8_t numerator, uint8_t denominator);
    void clear_tempo_events(double initial_bpm);
    bool set_tempo_event(double beat, double bpm, bool ramp);
    bool remove_tempo_event(double beat);
    bool move_tempo_event(double from_beat, double to_beat);
    float get_tempo() const;
    bool set_tempo(float bpm);
    bool add_marker(uint64_t sample, std::string_view name, uint32_t color) { return m_arrangement.upsertMarker({sample, std::string(name), color}); }
    bool remove_marker(uint64_t sample) { return m_arrangement.removeMarker(sample); }
    std::vector<uint64_t> marker_positions() const { std::vector<uint64_t> out; for (const auto& m : m_arrangement.markers()) out.push_back(m.sample); return out; }
    bool set_arranger_parts(std::vector<::Aura::Core::Composition::ArrangerPart> parts) { return m_arrangement.setArrangerParts(std::move(parts)); }
    std::vector<::Aura::Core::Composition::ArrangerPart> arranger_parts() const { return m_arrangement.arrangerParts(); }
    void tap_tempo(uint64_t timestamp_ms) noexcept { m_tapTempo.tap(timestamp_ms); const double bpm = m_tapTempo.bpm(); if (bpm > 0.0) m_tempo.store(static_cast<float>(bpm), std::memory_order_relaxed); }
    void clear_tap_tempo() noexcept { m_tapTempo.clear(); }
    double tapped_tempo() const noexcept { return m_tapTempo.bpm(); }
    void midi_clock_tick(uint64_t timestamp) noexcept { m_externalSync.onClockTick(timestamp); }
    uint64_t midi_clock_ticks() const noexcept { return m_externalSync.tickCount(); }
    uint64_t midi_clock_last_tick() const noexcept { return m_externalSync.lastTick(); }
    double midi_clock_rate() const noexcept { return m_externalSync.clockRate(); }
    void set_midi_clock_rate(double bpm) noexcept { m_externalSync.setClockRate(bpm); }
    bool set_master_gain(float value);
    bool add_control_room_speaker(std::string_view name, float gain = 1.0f) {
        return m_controlRoom.addSpeakerSet(std::string(name), gain);
    }
    void reset_control_room() { m_controlRoom.resetForProject(); }
    bool select_control_room_speaker(uint32_t index) noexcept {
        return m_controlRoom.selectSpeakerSet(index);
    }
    bool rename_control_room_speaker(uint32_t index, std::string_view name) {
        return m_controlRoom.renameSpeakerSet(index, std::string(name));
    }
    bool remove_control_room_speaker(uint32_t index) { return m_controlRoom.removeSpeakerSet(index); }
    bool set_control_room_speaker_gain(uint32_t index, float gain) { return m_controlRoom.setSpeakerGain(index, gain); }
    bool set_control_room_speaker_enabled(uint32_t index, bool enabled) { return m_controlRoom.setSpeakerEnabled(index, enabled); }
    bool upsert_control_room_cue(uint32_t id, float gain, bool enabled = true) { return m_controlRoom.upsertCueMix(id, gain, enabled); }
    bool remove_control_room_cue(uint32_t id) { return m_controlRoom.removeCueMix(id); }
    bool set_control_room_cue_enabled(uint32_t id, bool enabled) { return m_controlRoom.setCueMixEnabled(id, enabled); }
    float control_room_cue_gain(uint32_t id) const noexcept { return m_controlRoom.cueGain(id); }
    void set_control_room_dim(bool enabled) noexcept { m_controlRoom.setDim(enabled); }
    void set_control_room_talkback(bool enabled, float gain = 1.0f) noexcept {
        m_controlRoom.setTalkback(enabled, gain);
    }
    bool control_room_dimmed() const noexcept { return m_controlRoom.isDimmed(); }
    bool control_room_talkback_enabled() const noexcept { return m_controlRoom.talkbackEnabled(); }
    float control_room_monitor_gain() const noexcept { return m_controlRoom.monitorGain(); }
    bool control_room_validate() const noexcept { return m_controlRoom.validate(); }
    void process_control_room_monitor(float* left, float* right, const float* talkback,
                                      uint32_t frames) const noexcept {
        m_controlRoom.processMonitorWithTalkback(left, right, talkback, frames);
    }
    bool save_track_preset(std::string_view name, uint32_t track_id);
    bool apply_track_preset(std::string_view name, uint32_t track_id);
    bool remove_track_preset(std::string_view name);
    std::string plugin_compatibility_snapshot_json() const;
    void record_plugin_scan_failure(std::string_view id, std::string_view error);
    void record_plugin_crash(std::string_view id);
    void set_plugin_blacklisted(std::string_view id, bool value);
    bool add_automation_lane(uint32_t id) { return m_automationLanes.add(id); }
    bool set_automation_lane_linked(uint32_t id, bool linked) { return m_automationLanes.link(id, linked); }
    bool set_automation_lane_protected(uint32_t id, bool protectedLane) { return m_automationLanes.protect(id, protectedLane); }
    bool preview_automation_lane(uint32_t id, float value) { return m_automationLanes.preview(id, value); }
    float get_master_gain() const noexcept;
    bool set_track_volume(uint32_t tid, float value);
    float get_track_volume(uint32_t tid) const;
    bool set_track_pan(uint32_t tid, float value);
    bool set_track_mute(uint32_t tid, bool muted);
    bool set_track_solo(uint32_t tid, bool solo);
    bool set_track_solo_for_offline_render(uint32_t tid, bool solo);
    // Offline render target selection is control-side state. When active,
    // the audio graph keeps only the target and its upstream route closure,
    // then publishes the target output to the master buffer.
    bool set_offline_render_target(uint32_t tid);
    void clear_offline_render_target() noexcept;
    void set_offline_render_tail_seconds(float seconds) noexcept;
    void set_offline_render_options(bool pre_fader, bool include_inserts) noexcept;
    bool set_offline_render_range(uint64_t start_sample, uint64_t end_sample) noexcept;
    void clear_offline_render_range() noexcept;
    bool set_phase_invert(uint32_t tid, bool inverted);
    bool set_track_delay_samples(uint32_t tid, uint32_t samples);
    uint32_t get_track_delay_samples(uint32_t tid) const;
    bool set_track_armed(uint32_t tid, bool armed);
    bool set_track_input_monitor(uint32_t tid, bool enabled);
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

