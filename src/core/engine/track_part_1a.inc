public:
    enum Type { Audio=0, Midi=1, Instrument=2, Bus=3, Vocal=4 };

    Track(uint32_t id, const std::string& n, Type t,
          std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : m_layoutRevision(std::move(layoutRevision)), m_id(id), m_type(t) {
        // The name is Rust state, so the engine's own construction hands the
        // initial value to the store the track keeps.
        hirari_track_name_store_set(m_nameStore,
                                    reinterpret_cast<const uint8_t*>(n.data()), n.size());
        if (t == Type::Midi || t == Type::Instrument) {
            m_builtInMidiState = hirari_builtin_midi_instrument_create();
        }
        m_articulationState = hirari_track_midi_articulation_create();
        m_pdcDelayState = hirari_track_pdc_delay_create();
        // The Rust runtime renders whole blocks, so it is bound to every DSP
        // handle and tap buffer the track owns. It is destroyed first.
        m_runtime = hirari_track_runtime_create(
            id, static_cast<uint32_t>(t), m_chain, m_strip, m_automationStore, m_frozenStore,
            m_spatialPanner, m_pdcDelayState, m_activeBuffer, m_preInsertBuffer,
            m_preFaderBuffer, m_sendPreFaderBuffer, m_sendPostFaderBuffer, m_pluginMidi,
            m_articulationState, m_builtInMidiState);
        hirari_region_store_install(this, m_id);
    }

    virtual ~Track() {
        hirari_track_runtime_destroy(m_runtime);
        hirari_midi_buffer_destroy(m_pluginMidi);
        hirari_region_store_forget(this);
        hirari_builtin_midi_instrument_destroy(m_builtInMidiState);
        hirari_track_midi_articulation_destroy(m_articulationState);
        hirari_track_pdc_delay_destroy(m_pdcDelayState);
        hirari_track_automation_store_destroy(m_automationStore);
        hirari_track_name_store_destroy(m_nameStore);
        hirari_effect_chain_destroy(m_chain);
        hirari_channel_strip_destroy(m_strip);
        hirari_track_holographic_panner_destroy(m_spatialPanner);
        hirari_track_frozen_store_destroy(m_frozenStore);
        hirari_audio_buffer_storage_destroy(m_activeBuffer);
        hirari_audio_buffer_storage_destroy(m_preFaderBuffer);
        hirari_audio_buffer_storage_destroy(m_sendPreFaderBuffer);
        hirari_audio_buffer_storage_destroy(m_sendPostFaderBuffer);
        hirari_audio_buffer_storage_destroy(m_preInsertBuffer);
    }

    /**
     * @brief PROCESS: Performs region lookup and signal processing with industrial precision.
     * INDUSTRIAL: Delegating temporal resolution and processing orchestration to the Rust 'TrackOrchestrator'.
     */
    void process(void* buffer, uint32_t sz, uint64_t playhead) {
        process(buffer, sz, playhead, true);
    }

    // The articulation runtime is Rust-owned; the expression-map edit host
    // resolves it by track id instead of going through the track wrapper.
    void* articulationStateHandle() const noexcept { return m_articulationState; }
    // The Rust block head stages timeline MIDI straight into this buffer.
    void* pluginMidiHandle() const noexcept { return m_pluginMidi; }
    void setExpressionMapJson(std::string json) {
        (void)hirari_track_midi_expression_map_set(
            m_articulationState, 0, reinterpret_cast<const uint8_t*>(json.data()), json.size());
        markProjectLayoutChanged();
    }
    void setExpressionMapProJson(std::string json) {
        (void)hirari_track_midi_expression_map_set(
            m_articulationState, 1, reinterpret_cast<const uint8_t*>(json.data()), json.size());
        markProjectLayoutChanged();
    }
    enum class OfflineRenderTap : uint8_t { PostFader = 0, PreInsert = 1, PreFader = 2 };

    // Process the already accumulated work buffer without clearing it first.
    // The audio graph uses this at a topological boundary after upstream
    // tracks have added their outputs into this track's input buffer.
    virtual bool processAccumulated(uint32_t sz, uint64_t playhead) {
        if (sz == 0 || hirari_audio_buffer_storage_channels(m_activeBuffer) < 2 ||
            hirari_audio_buffer_storage_samples(m_activeBuffer) < sz) {
            return false;
        }
        process(m_activeBuffer, sz, playhead, false);
        return true;
    }

    // Transport state is published once per audio block by the engine. The
    // processor context must never fall back to a fictional 120 BPM or claim
    // playback while the transport is stopped.
    void setTransportState(double bpm, bool playing) noexcept {
        if (std::isfinite(bpm) && bpm > 0.0) {
            bpm = std::clamp(bpm, 1.0, 999.0);
        } else {
            bpm = hirari_track_runtime_transport_bpm(m_runtime);
        }
        hirari_track_runtime_set_transport(m_runtime, bpm, static_cast<uint8_t>(playing));
    }

    double getTransportBpm() const noexcept {
        return hirari_track_runtime_transport_bpm(m_runtime);
    }

private:
    void process(void* buffer, uint32_t sz, uint64_t playhead, bool clearInput) {
        hirari_track_process_block(m_runtime, buffer, sz, playhead,
                                   static_cast<uint8_t>(clearInput));
    }
