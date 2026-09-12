public:
    bool split_region(uint32_t tid, uint32_t rid, double splitBeat);
    bool remove_region(uint32_t tid, uint32_t rid);
    uint32_t duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample);
    bool move_region(uint32_t tid, uint32_t rid, double startBeat);
    bool set_region_gain(uint32_t tid, uint32_t rid, float gain);
    bool set_region_muted(uint32_t tid, uint32_t rid, bool muted);
    void clear_midi_notes();
    struct ScheduledMidiNote {
        uint32_t trackId = 0;
        uint8_t pitch = 60;
        uint8_t velocity = 100;
        uint64_t startSample = 0;
        uint64_t lengthSamples = 1;
    };
    bool replace_midi_notes(const std::vector<ScheduledMidiNote>& notes, bool recordUndo = true);
    std::vector<ScheduledMidiNote> midi_notes_snapshot() const;
    bool remove_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample);
    bool transpose_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int32_t semitones);
    bool move_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int64_t deltaSamples);
    void set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity,
                       uint64_t startSample, uint64_t lengthSamples);
    bool set_region_fades(uint32_t tid, uint32_t rid, float fadeIn, float fadeOut);
    bool set_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start, uint64_t end,
                               float gain, uint64_t fadeIn, uint64_t fadeOut);
    bool clear_region_range_edit(uint32_t tid, uint32_t rid, uint64_t start, uint64_t end);
    bool clear_region_range_edits(uint32_t tid, uint32_t rid);
    bool set_region_reverse(uint32_t tid, uint32_t rid, bool reverse);
    bool set_region_warp_ratio(uint32_t tid, uint32_t rid, double ratio);
    bool set_region_pitch_semitones(uint32_t tid, uint32_t rid, float semitones);
    bool set_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                       double startSeconds, double endSeconds,
                                       double pitchOffsetCents, double formantOffsetCents);
    bool set_region_audio_note_anchor(uint32_t tid, uint32_t rid,
                                      double segmentStartSeconds, double positionSeconds,
                                      double pitchCents, double formantCents);
    bool clear_region_audio_note_segments(uint32_t tid, uint32_t rid);
    bool warp_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                        double segmentStartSeconds,
                                        double newStartSeconds, double newEndSeconds);
    bool remove_region_audio_note_segment(uint32_t tid, uint32_t rid,
                                          double segmentStartSeconds);
    bool analyze_region_audio_note_segments(uint32_t tid, uint32_t rid, double sampleRate);
    bool set_region_loop_count(uint32_t tid, uint32_t rid, uint32_t count);
    bool set_region_locked(uint32_t tid, uint32_t rid, bool locked);
    bool set_region_sync_group(uint32_t tid, uint32_t rid, uint32_t group);
    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled);
    bool clear_region_processing(uint32_t tid, uint32_t rid);
    bool set_region_trim(uint32_t tid, uint32_t rid, float startNorm, float endNorm);

    // Project
    bool saveProject(std::string_view path);
    bool loadProject(std::string_view path);
    bool bounce_project(std::string_view outputPath, uint32_t format);
    bool bounce_project_async(std::string_view outputPath, uint32_t format);
    // Lock-free status reads for UI/control threads. These never wait on the
    // audio callback or the bounce worker.
    float get_bounce_progress() const noexcept;
    uint32_t get_bounce_state() const noexcept;
    bool cancel_bounce() noexcept;
    bool pause_bounce() noexcept;
    bool resume_bounce() noexcept;
    bool generate_drum_fill(uint32_t track_id, uint32_t bar, float complexity);
    void set_show_video(bool s);
    void set_ascended_mode(bool enabled);
    void execute_restoration(uint32_t trackId, float intensity);
    bool execute_vocal_remover(uint32_t tid);
    void set_automation_record_mode(uint32_t mode);
    void set_automation_punch_range(uint64_t start, uint64_t end) noexcept;
    float get_track_correlation(uint32_t trackId) const;
    void sync_sharding_node(uint32_t node_id);
    void browser_preview(std::string_view path);
    rust::Vec<uint8_t> get_video_frame();
    uint64_t get_video_frame_revision() const noexcept;
    bool request_video_frame(double seconds);
    bool load_video(std::string_view path);
    rust::Vec<float> get_region_waveform(uint32_t tid, uint32_t rid, uint32_t numPeaks);
    rust::Vec<float> get_region_audio_samples(uint32_t tid, uint32_t rid, uint32_t maxSamples);
    rust::Vec<float> get_region_audio_interleaved(uint32_t tid, uint32_t rid, uint32_t maxFrames);
    double get_region_sample_rate(uint32_t tid, uint32_t rid);
    uint32_t get_region_channel_count(uint32_t tid, uint32_t rid);

