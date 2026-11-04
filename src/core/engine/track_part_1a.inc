public:
    enum Type { Audio=0, Midi=1, Instrument=2, Bus=3, Vocal=4 };
    using AutomationList = std::vector<Hirari::Core::ProjectAutomationPoint>;
    using PluginAutomationList =
        std::vector<Hirari::Core::ProjectPluginAutomationLane>;

    Track(uint32_t id, const std::string& n, Type t,
          std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : m_layoutRevision(std::move(layoutRevision)), m_id(id), m_name(n), m_type(t) {
        if (t == Type::Midi || t == Type::Instrument) {
            m_builtInMidiState = hirari_builtin_midi_instrument_create();
        }
        m_articulationState = hirari_track_midi_articulation_create();
        m_pdcDelayState = hirari_track_pdc_delay_create();
        // The Rust runtime renders whole blocks, so it is bound to every DSP
        // handle and tap buffer the track owns. It is destroyed first.
        m_runtime = hirari_track_runtime_create(
            id, m_chain, m_strip, m_automationStore, m_frozenStore, m_spatialPanner,
            m_pdcDelayState, m_activeBuffer.storageHandle(), m_preInsertBuffer.storageHandle(),
            m_preFaderBuffer.storageHandle(), m_sendPreFaderBuffer.storageHandle(),
            m_sendPostFaderBuffer.storageHandle(), m_pluginMidi,
            &Track::prepareMidiBlock, this);
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
        hirari_effect_chain_destroy(m_chain);
        hirari_channel_strip_destroy(m_strip);
        hirari_track_holographic_panner_destroy(m_spatialPanner);
        hirari_track_frozen_store_destroy(m_frozenStore);
    }

    /**
     * @brief PROCESS: Performs region lookup and signal processing with industrial precision.
     * INDUSTRIAL: Delegating temporal resolution and processing orchestration to the Rust 'TrackOrchestrator'.
     */
    void process(AudioBuffer& buffer, uint32_t sz, uint64_t playhead) {
        process(buffer, sz, playhead, true);
    }

    void clearWorkBuffer(uint32_t sz) {
        if (sz == 0 || m_activeBuffer.getNumChannels() == 0) return;
        m_activeBuffer.clear(std::min(sz, m_activeBuffer.getNumSamples()));
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
    std::string getExpressionMapJson() const {
        return copyExpressionMapJson(0);
    }
    void setExpressionMapProJson(std::string json) {
        (void)hirari_track_midi_expression_map_set(
            m_articulationState, 1, reinterpret_cast<const uint8_t*>(json.data()), json.size());
        markProjectLayoutChanged();
    }
    std::string getExpressionMapProJson() const {
        return copyExpressionMapJson(1);
    }
    void addGeneratedMidiEvents(const HirariGeneratedMidiEvent* events,
                                size_t count) noexcept {
        static_assert(sizeof(HirariGeneratedMidiEvent) == 16);
        for (size_t index = 0; index < count; ++index) {
            const auto& event = events[index];
            hirari_midi_buffer_add(m_pluginMidi, event.sample_offset, event.data,
                                   event.size, event.articulation_id);
        }
    }
    void flushExpressionNoteOffs(uint64_t playhead, uint32_t blockSize) noexcept {
        std::array<HirariGeneratedMidiEvent, kHirariMidiMaxEventsPerBlock> generated{};
        const size_t capacity =
            std::min(hirari_midi_buffer_remaining_capacity(m_pluginMidi), generated.size());
        const size_t count = hirari_track_midi_flush_scheduled_note_offs(
            m_articulationState, playhead, blockSize,
            capacity ? generated.data() : nullptr, capacity);
        addGeneratedMidiEvents(generated.data(), count);
    }

    // Render a small built-in voice for MIDI tracks without an instrument
    // plug-in. It is generated into the block the runtime is rendering so the
    // normal channel strip, inserts, fader, pan, sends, and routing all process
    // it. This runs on the audio thread with fixed voice storage and no
    // allocation.
    void renderBuiltInMidiInstrument(float* left, float* right, uint32_t frames) noexcept {
        if ((m_type != Type::Midi && m_type != Type::Instrument) ||
            hirari_effect_chain_midi_instrument_count(m_chain) != 0 || frames == 0) {
            return;
        }
        if (left && right) {
            hirari_builtin_midi_instrument_process(
                m_builtInMidiState, hirari_midi_buffer_event_data(m_pluginMidi),
                hirari_midi_buffer_event_count(m_pluginMidi), left, right, frames,
                m_sampleRate);
        }
    }

    bool addInput(const AudioBuffer& input, uint32_t sz) {
        if (sz == 0 || input.getNumChannels() == 0 || input.getNumSamples() < sz ||
            m_activeBuffer.getNumChannels() == 0 || m_activeBuffer.getNumSamples() < sz) {
            return false;
        }
        m_activeBuffer.addFrom(input, sz);
        return true;
    }

    enum class OfflineRenderTap : uint8_t { PostFader = 0, PreInsert = 1, PreFader = 2 };

    // Process the already accumulated work buffer without clearing it first.
    // The audio graph uses this at a topological boundary after upstream
    // tracks have added their outputs into this track's input buffer.
    virtual bool processAccumulated(uint32_t sz, uint64_t playhead) {
        if (sz == 0 || m_activeBuffer.getNumChannels() < 2 ||
            m_activeBuffer.getNumSamples() < sz) {
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
    std::string copyExpressionMapJson(uint8_t mapKind) const {
        size_t size = 0;
        auto* json = hirari_track_midi_expression_map_take(m_articulationState, mapKind, &size);
        if (!json) return {};
        std::string result(reinterpret_cast<const char*>(json), size);
        hirari_track_midi_expression_map_take_free(json, size);
        return result;
    }

    // One block's MIDI input is fully prepared before the chain runs: expired
    // expression note-offs are queued, the generated events are put in
    // chronological order, and a track without an instrument gets its built-in
    // voice. The runtime hands over the block channels it already resolved.
    static void prepareMidiBlock(void* context, uint64_t playhead, uint32_t frames,
                                 float* left, float* right) noexcept {
        auto& track = *static_cast<Track*>(context);
        track.flushExpressionNoteOffs(playhead, frames);
        hirari_midi_buffer_sort_owned(track.m_pluginMidi);
        track.renderBuiltInMidiInstrument(left, right, frames);
    }

    void process(AudioBuffer& buffer, uint32_t sz, uint64_t playhead, bool clearInput) {
        hirari_track_process_block(m_runtime, buffer.storageHandle(), sz, playhead,
                                   static_cast<uint8_t>(clearInput));
    }
