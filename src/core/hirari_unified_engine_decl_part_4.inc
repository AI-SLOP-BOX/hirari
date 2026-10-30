public:
    bool split_region(uint32_t tid, uint32_t rid, double splitBeat);
    uint32_t split_region_with_right_id(uint32_t tid, uint32_t rid, double splitSample);
    void clear_midi_notes();
    struct ScheduledMidiNote {
        uint32_t trackId = 0;
        uint8_t pitch = 60;
        uint8_t velocity = 100;
        uint8_t midiChannel = 0;
        uint8_t articulationId = 0;
        uint64_t startSample = 0;
        uint64_t lengthSamples = 1;
        uint8_t probability = 100; // Chance in percent for each transport pass.
        uint32_t regionId = 0; // Zero denotes a legacy track-scoped note.
    };
    bool replace_midi_notes(const std::vector<ScheduledMidiNote>& notes, bool recordUndo = true);
    std::vector<ScheduledMidiNote> midi_notes_snapshot() const;
    bool remove_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample);
    bool transpose_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int32_t semitones);
    bool move_midi_notes_range(uint32_t trackId, uint64_t startSample, uint64_t endSample, int64_t deltaSamples);
    void set_midi_note(uint32_t trackId, uint8_t pitch, uint8_t velocity,
                       uint64_t startSample, uint64_t lengthSamples);

    bool append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled);
    bool clear_region_processing(uint32_t tid, uint32_t rid);

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
    bool execute_vocal_remover(uint32_t tid);
    bool set_automation_record_mode(uint32_t mode);
    float get_track_correlation(uint32_t trackId) const;
    rust::Vec<uint8_t> get_video_frame();
    uint64_t get_video_frame_revision() const noexcept;
    bool request_video_frame(double seconds);
    bool load_video(std::string_view path);
    rust::Vec<float> get_region_waveform(uint32_t tid, uint32_t rid, uint32_t numPeaks);
    rust::Vec<float> get_region_audio_interleaved(uint32_t tid, uint32_t rid, uint32_t maxFrames);
    rust::Vec<float> get_region_audio_interleaved_range(uint32_t tid, uint32_t rid,
                                                        uint64_t startFrame,
                                                        uint32_t frames,
                                                        uint32_t channels);
