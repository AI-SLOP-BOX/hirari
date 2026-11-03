public:
    enum Type { Audio=0, Midi=1, Instrument=2, Bus=3, Vocal=4 };
    using AutomationList = std::vector<Hirari::Core::ProjectAutomationPoint>;
    using PluginAutomationList =
        std::vector<Hirari::Core::ProjectPluginAutomationLane>;

    Track(uint32_t id, const std::string& n, Type t,
          void* sidechainEngine = nullptr,
          std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : m_layoutRevision(std::move(layoutRevision)), m_id(id), m_name(n), m_type(t),
          m_effectChain(sidechainEngine) {
        if (t == Type::Midi || t == Type::Instrument) {
            m_builtInMidiState = hirari_builtin_midi_instrument_create();
        }
        m_articulationState = hirari_track_midi_articulation_create();
        m_pdcDelayState = hirari_track_pdc_delay_create();
        hirari_region_store_install(this, m_id);
    }

    virtual ~Track() {
        hirari_region_store_forget(this);
        hirari_builtin_midi_instrument_destroy(m_builtInMidiState);
        hirari_track_midi_articulation_destroy(m_articulationState);
        hirari_track_pdc_delay_destroy(m_pdcDelayState);
        hirari_track_automation_store_destroy(m_automationStore);
        hirari_sandbox_slots_destroy(m_sandboxSlots);
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

    // The engine publishes only events addressed to this track before it
    // enters the graph. Keep the buffer owned by the track so MIDI reaches
    // its instrument/effect chain without allocating on the audio thread.
    void clearMidiInput() noexcept { m_pluginMidi.clear(); }
    // The articulation runtime is Rust-owned; the expression-map edit host
    // resolves it by track id instead of going through the track wrapper.
    void* articulationStateHandle() const noexcept { return m_articulationState; }
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
    template <class Callback>
    void forEachTimedKeySwitchOutput(uint8_t articulationId, Callback&& callback) const noexcept {
        std::array<HirariTimedKeySwitchOutput, 16> outputs{};
        const size_t count = hirari_track_midi_timed_outputs(
            m_articulationState, articulationId, false, outputs.data(), outputs.size());
        for (size_t slot = 0; slot < count; ++slot) {
            callback(outputs[slot].channel, outputs[slot].pitch, outputs[slot].length_ticks);
        }
    }
    template <class Callback>
    void forEachTimedTransitionKeySwitchOutput(uint8_t articulationId,
                                                Callback&& callback) const noexcept {
        std::array<HirariTimedKeySwitchOutput, 16> outputs{};
        const size_t count = hirari_track_midi_timed_outputs(
            m_articulationState, articulationId, true, outputs.data(), outputs.size());
        for (size_t slot = 0; slot < count; ++slot) {
            callback(outputs[slot].channel, outputs[slot].pitch, outputs[slot].length_ticks);
        }
    }
    bool scheduleExpressionNoteOff(uint8_t zeroBasedChannel, uint8_t pitch,
                                   uint64_t absoluteSample,
                                   uint64_t fallbackOffset) noexcept {
        if (hirari_track_midi_schedule_note_off(
                m_articulationState, zeroBasedChannel, pitch, absoluteSample)) return true;
        // An overloaded pathological schedule must never leave the
        // instrument's keyswitch physically held forever.
        m_pluginMidi.addNoteOff(static_cast<uint8_t>(zeroBasedChannel + 1),
                                pitch, fallbackOffset);
        return false;
    }
    void addGeneratedMidiEvents(const HirariGeneratedMidiEvent* events,
                                size_t count) noexcept {
        static_assert(sizeof(HirariGeneratedMidiEvent) == 16);
        for (size_t index = 0; index < count; ++index) {
            const auto& event = events[index];
            m_pluginMidi.addEvent(event.sample_offset, event.data, event.size,
                                  event.articulation_id);
        }
    }
    void addMidiInputNoteOn(uint8_t channel, uint8_t pitch, uint8_t velocity,
                            uint64_t sampleOffset, uint8_t articulationId = 0) noexcept {
        std::array<HirariGeneratedMidiEvent, 128> generated{};
        const size_t count = hirari_track_midi_note_on(
            m_articulationState, channel, pitch, velocity, sampleOffset,
            articulationId, generated.data(), generated.size());
        addGeneratedMidiEvents(generated.data(), count);
    }
    void addMidiInputNoteOff(uint8_t channel, uint8_t pitch,
                             uint64_t sampleOffset, uint8_t articulationId = 0) noexcept {
        std::array<HirariGeneratedMidiEvent, 128> generated{};
        const size_t count = hirari_track_midi_note_off(
            m_articulationState, channel, pitch, sampleOffset, articulationId,
            generated.data(), generated.size());
        addGeneratedMidiEvents(generated.data(), count);
    }
    void addMidiInputAllNotesOff(uint64_t sampleOffset) noexcept {
        constexpr size_t kMaxGeneratedEvents = MidiBuffer::kMaxEventsPerBlock;
        std::array<HirariGeneratedMidiEvent, kMaxGeneratedEvents> generated{};
        const size_t capacity = std::min(m_pluginMidi.remainingCapacity(), generated.size());
        const size_t count = hirari_track_midi_all_notes_off(
            m_articulationState, sampleOffset, capacity ? generated.data() : nullptr, capacity);
        addGeneratedMidiEvents(generated.data(), count);
    }
    void flushExpressionNoteOffs(uint64_t playhead, uint32_t blockSize) noexcept {
        std::array<HirariGeneratedMidiEvent, MidiBuffer::kMaxEventsPerBlock> generated{};
        const size_t capacity = std::min(m_pluginMidi.remainingCapacity(), generated.size());
        const size_t count = hirari_track_midi_flush_scheduled_note_offs(
            m_articulationState, playhead, blockSize,
            capacity ? generated.data() : nullptr, capacity);
        addGeneratedMidiEvents(generated.data(), count);
    }

    // Render a small built-in voice for MIDI tracks without an instrument
    // plug-in. It is generated into the track's input buffer so the normal
    // channel strip, inserts, fader, pan, sends, and routing all process it.
    // This runs on the audio thread with fixed voice storage and no allocation.
    void renderBuiltInMidiInstrument(AudioBuffer& buffer, uint32_t frames) noexcept {
        if ((m_type != Type::Midi && m_type != Type::Instrument) ||
            m_midiInstrumentPluginCount.load(std::memory_order_acquire) != 0 ||
            buffer.getNumChannels() < 2 || buffer.getNumSamples() < frames || frames == 0) {
            return;
        }
        float* left = buffer.getWritePointer(0);
        float* right = buffer.getWritePointer(1);
        if (left && right) {
            hirari_builtin_midi_instrument_process(
                m_builtInMidiState, m_pluginMidi.getEvents(), m_pluginMidi.size(),
                left, right, frames, m_sampleRate);
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

    // Bus tracks have a second insert chain. Defer the shared channel output
    // stage so their inserts, EQ, sends, and fader stay in serial order.
    bool processAccumulatedDeferredOutput(uint32_t sz, uint64_t playhead) {
        if (sz == 0 || m_activeBuffer.getNumChannels() < 2 ||
            m_activeBuffer.getNumSamples() < sz) return false;
        process(m_activeBuffer, sz, playhead, false, true);
        return true;
    }

    // Transport state is published once per audio block by the engine. The
    // processor context must never fall back to a fictional 120 BPM or claim
    // playback while the transport is stopped.
    void setTransportState(double bpm, bool playing) noexcept {
        if (std::isfinite(bpm) && bpm > 0.0) {
            m_transportBpm.store(std::clamp(bpm, 1.0, 999.0), std::memory_order_relaxed);
        }
        m_transportPlaying.store(playing, std::memory_order_relaxed);
    }

    double getTransportBpm() const noexcept {
        return m_transportBpm.load(std::memory_order_relaxed);
    }

    bool isTransportPlaying() const noexcept {
        return m_transportPlaying.load(std::memory_order_relaxed);
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

    void process(AudioBuffer& buffer, uint32_t sz, uint64_t playhead, bool clearInput,
                 bool deferOutput = false) {
        constexpr size_t kMaxPluginAutomationEventsPerBlock =
            Hirari::Core::Plugins::SandboxProtocol::kMaxParameterChanges;
        static_assert(sizeof(DSP::TimedParameterEvent) == 16);
        static_assert(offsetof(DSP::TimedParameterEvent, sampleOffset) == 8);
        static_assert(sizeof(Hirari::Core::ProjectAutomationPoint) == 16);
        std::array<DSP::TimedParameterEvent, kMaxPluginAutomationEventsPerBlock>
            pluginAutomationEvents{};
        size_t pluginAutomationEventCount = 0;
        const auto* regionInputs = m_regionInputs.load(std::memory_order_acquire);
        const size_t regionInputCount = m_regionInputCount.load(std::memory_order_acquire);
        const uint8_t regionPublished = m_regionPublished.load(std::memory_order_acquire);
        size_t trackDelayCount = 0;
        const auto* trackDelayAutomation = static_cast<const Hirari::Core::ProjectAutomationPoint*>(
            hirari_track_automation_store_points(m_automationStore, 2, &trackDelayCount));
        size_t pluginLaneCount = 0;
        const auto* pluginLanes = hirari_track_automation_store_plugin_lane_views(
            m_automationStore, &pluginLaneCount);
        const auto* frozen = static_cast<const AudioBuffer*>(
            hirari_track_frozen_store_current(m_frozenStore));
        uint32_t trackDelaySamples = m_trackDelaySamples.load(std::memory_order_relaxed);
        struct MidiPreparationContext {
            Track* track;
            AudioBuffer* buffer;
        } midiContext{this, &buffer};
        const HirariTrackProcessRequest request{
            buffer.getArrayOfWritePointers(), buffer.getNumChannels(), buffer.getNumSamples(),
            sz, playhead, static_cast<uint8_t>(clearInput),
            static_cast<uint8_t>(m_muted.load(std::memory_order_relaxed)),
            static_cast<uint8_t>(hirari_track_frozen_store_is_frozen(m_frozenStore)),
            frozen ? frozen->getArrayOfReadPointers() : nullptr,
            frozen ? frozen->getNumChannels() : 0,
            frozen ? frozen->getNumSamples() : 0,
            hirari_track_frozen_store_total_samples(m_frozenStore),
            regionInputs, regionInputCount, regionPublished,
            static_cast<uint8_t>(m_phaseInvert.load(std::memory_order_relaxed)),
            trackDelayAutomation && trackDelayCount != 0 ? trackDelayAutomation : nullptr,
            trackDelayCount, hirari_track_automation_store_hint(m_automationStore, 2),
            &trackDelaySamples, kMaxPdcDelaySamples,
            pluginLanes, pluginLaneCount,
            static_cast<uint8_t>(m_transportPlaying.load(std::memory_order_relaxed)),
            pluginAutomationEvents.data(), pluginAutomationEvents.size(),
            &pluginAutomationEventCount,
            m_preInsertBuffer.getArrayOfWritePointers(),
            m_preInsertBuffer.getNumChannels(), m_preInsertBuffer.getNumSamples(),
            +[](void* context, uint64_t blockPlayhead, uint32_t frames) {
                auto* midi = static_cast<MidiPreparationContext*>(context);
                midi->track->flushExpressionNoteOffs(blockPlayhead, frames);
                midi->track->m_pluginMidi.sort();
                midi->track->renderBuiltInMidiInstrument(*midi->buffer, frames);
            },
            &midiContext};
        if (!hirari_track_process_prepare(&request)) return;
        m_trackDelaySamples.store(trackDelaySamples, std::memory_order_relaxed);

        // Queue chronological offsets for isolated plugin workers. Preserve
        // the existing native sort policy for equal-key event ordering.
        m_effectChain.setParameterAutomationRealtime(
            pluginAutomationEvents.data(), pluginAutomationEventCount);
        if (buffer.getNumChannels() >= 2 && sz > 0) {
            DSP::ProcessContext context{};
            context.playhead = playhead;
            context.blockStart = playhead;
            context.blockEnd = playhead <= std::numeric_limits<uint64_t>::max() - sz
                ? playhead + sz
                : std::numeric_limits<uint64_t>::max();
            context.sampleRate = m_sampleRate;
            context.bpm = m_transportBpm.load(std::memory_order_relaxed);
            context.blockSize = sz;
            context.isPlaying = m_transportPlaying.load(std::memory_order_relaxed);
            context.anyoneSoloed = false;
            context.numOutputChannels = buffer.getNumChannels();
            m_effectChain.process(buffer, m_pluginMidi, context, m_id);
            // Processors share the mutable MIDI buffer for this one block.
            // Drop it before returning so alternate render paths cannot replay
            // events that were already consumed by the track chain.
            m_pluginMidi.clear();
        }

        if (!deferOutput) finalizeTrackOutput(sz, playhead);
    }
