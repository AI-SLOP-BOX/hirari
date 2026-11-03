public:
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
    rust::Vec<float> get_region_waveform(uint32_t tid, uint32_t rid, uint32_t numPeaks);
