public:
    enum Type { Audio=0, Midi=1, Instrument=2, Bus=3, Vocal=4 };
    using AutomationList = std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>;
    using PluginAutomationList =
        std::vector<Hirari::Core::ProjectSerializer::PluginAutomationLane>;

    Track(uint32_t id, const std::string& n, Type t,
          SidechainManager* sidechainManager = nullptr,
          std::shared_ptr<ProjectLayoutRevision> layoutRevision = {})
        : m_layoutRevision(std::move(layoutRevision)), m_id(id), m_name(n), m_type(t),
          m_effectChain(sidechainManager), m_analysis(&m_activeBuffer) {
        if (t == Type::Midi || t == Type::Instrument) {
            m_builtInMidiState = hirari_builtin_midi_instrument_create();
        }
        m_articulationState = hirari_track_midi_articulation_create();
        m_trackAutomationCaptureState = hirari_track_automation_capture_create();
        m_pdcDelayState = hirari_track_pdc_delay_create();
        m_regionResampleKernel = hirari_region_resampler_prepare();
        publishRegionSnapshot();
    }

    virtual ~Track() {
        hirari_builtin_midi_instrument_destroy(m_builtInMidiState);
        hirari_track_midi_articulation_destroy(m_articulationState);
        hirari_track_automation_capture_destroy(m_trackAutomationCaptureState);
        hirari_track_pdc_delay_destroy(m_pdcDelayState);
    }

    void addMidiRegion(std::shared_ptr<Hirari::Core::MidiRegion> region) {
        if (!region || !std::isfinite(region->getStartBeat()) ||
            !std::isfinite(region->getLengthBeats()) || region->getLengthBeats() <= 0.0) return;
        // MIDI regions are part of the rendered signal just like audio
        // regions. A frozen track must never keep serving the pre-edit audio
        // after a new note region is attached.
        invalidateFrozenForEdit();
        m_midiRegions.push_back(std::move(region));
        markProjectLayoutChanged();
    }

    const std::vector<std::shared_ptr<Hirari::Core::MidiRegion>>& getMidiRegions() const noexcept {
        return m_midiRegions;
    }

    /**
     * @brief Adds a region with industrial-grade management and temporal sovereignty.
     * INDUSTRIAL: Delegating region management and sorting to the Rust 'TrackOrchestrator'.
     */
    void addRegion(const Region& r) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        Region normalized = r;
        // Keep audio-buffer ownership in C++; delegate timeline, loop, gain,
        // pitch and source-range invariants to the Rust core.
        auto addMetadata = regionAddMetadataForRust(normalized);
        const bool hasAudio = static_cast<bool>(normalized.audio);
        const uint64_t audioSamples = hasAudio ? normalized.audio->getNumSamples() : 0;
        const uint32_t audioChannels = hasAudio ? normalized.audio->getNumChannels() : 0;
        if (!hirari_region_normalize_add_metadata(
                &addMetadata, hasAudio, audioSamples, audioChannels)) return;
        applyRegionAddMetadata(normalized, addMetadata);
        // VariAudio-style note segments are immutable render metadata.  Keep
        // only bounded, well-formed segments in the published snapshot so a
        // malformed project cannot inject NaN/Inf pitch ratios into the audio
        // callback.  Ordering is deterministic for fast segment lookup.
        if (normalized.audioNoteSegments.size() > 4096) {
            normalized.audioNoteSegments.resize(4096);
        }
        normalized.audioNoteSegments.erase(
            std::remove_if(normalized.audioNoteSegments.begin(),
                           normalized.audioNoteSegments.end(),
                           [](const auto& segment) {
                               return !segment.valid() ||
                                      segment.anchors.size() > 4096;
                           }),
            normalized.audioNoteSegments.end());
        if (normalized.warpMarkers.size() > 100'000) {
            normalized.warpMarkers.resize(100'000);
        }
        const uint64_t warpSourceSpan = normalized.sourceLength > 0
            ? normalized.sourceLength : normalized.len;
        normalized.warpMarkers.resize(hirari_region_normalize_warp_markers(
            normalized.warpMarkers.data(), normalized.warpMarkers.size(),
            warpSourceSpan, normalized.len));
        for (auto& segment : normalized.audioNoteSegments) {
            const size_t anchorCount = hirari_audio_note_curve_normalize_anchors(
                segment.anchors.data(), segment.anchors.size(),
                segment.startSeconds, segment.endSeconds);
            if (anchorCount == std::numeric_limits<size_t>::max()) continue;
            segment.anchors.resize(anchorCount);
            segment.rebuildPitchRatioIntegral();
        }
        // The Rust render core owns validation and normalization so the audio
        // callback receives bounded, ordered range data.
        static_assert(sizeof(Region::RangeEdit) == sizeof(HirariRegionRangeEdit));
        static_assert(offsetof(Region::RangeEdit, start) == offsetof(HirariRegionRangeEdit, start));
        const size_t validRangeEditCount = hirari_region_normalize_range_edits(
            reinterpret_cast<HirariRegionRangeEdit*>(normalized.rangeEdits.data()),
            normalized.rangeEdits.size(), normalized.len);
        normalized.rangeEdits.resize(validRangeEditCount);
        std::sort(normalized.audioNoteSegments.begin(),
                  normalized.audioNoteSegments.end(),
                  [](const auto& a, const auto& b) {
                      return a.startSeconds < b.startSeconds;
                  });
        hirari::editing::rebuildAudioNotePhasePrefixes(normalized.audioNoteSegments);
        normalized.fadeInSamples = std::min(normalized.fadeInSamples, normalized.len);
        normalized.fadeOutSamples = std::min(normalized.fadeOutSamples, normalized.len);
        auto it = std::lower_bound(
            m_regions.begin(),
            m_regions.end(),
            normalized.start,
            [](const Region& lhs, uint64_t start) { return lhs.start < start; }
        );
        m_regions.insert(it, std::move(normalized));
        publishRegionSnapshot();
    }

    void removeRegion(uint32_t rid) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        auto it = std::remove_if(
            m_regions.begin(),
            m_regions.end(),
            [rid](const Region& r) { return r.id == rid; }
        );
        if (it != m_regions.end()) {
            m_regions.erase(it, m_regions.end());
            publishRegionSnapshot();
        }
    }

    bool moveRegion(uint32_t rid, uint64_t newStart) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        for (auto& region : m_regions) {
            if (region.id == rid) {
                const auto input = regionGeometryForRust(region);
                HirariNativeRegionGeometry output{};
                if (!hirari_region_move_geometry(&input, newStart, &output)) return false;
                region.start = output.start;
                region.baseStart = output.base_start;
                std::sort(m_regions.begin(), m_regions.end(),
                          [](const Region& a, const Region& b) { return a.start < b.start; });
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool replaceRegionAudio(uint32_t rid, std::string path, std::shared_ptr<AudioBuffer> audio,
                            double sourceSampleRate, double timelineSampleRate) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (path.empty() || !audio || audio->getNumSamples() == 0 || audio->getNumChannels() == 0 ||
            !std::isfinite(sourceSampleRate) || sourceSampleRate < 8000.0 ||
            sourceSampleRate > 384000.0 || !std::isfinite(timelineSampleRate) ||
            timelineSampleRate < 8000.0 || timelineSampleRate > 384000.0) return false;
        for (auto& region : m_regions) {
            if (region.id == rid) {
                region.path = std::move(path);
                region.name = region.path;
                region.audio = std::move(audio);
                region.sourceSampleRate = sourceSampleRate;
                region.timelineSampleRate = timelineSampleRate;
                region.sourceOffset = 0;
                region.baseStart = region.start;
                region.baseSourceOffset = 0;
                region.warpMarkers.clear();
                region.sourceLength = region.audio->getNumSamples();
                region.len = static_cast<uint64_t>(std::ceil(
                    static_cast<long double>(region.sourceLength) /
                    region.sourceFramesPerTimelineFrame()));
                region.baseLength = region.sourceLength;
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
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
    bool beginArticulationMapUpdate(uint8_t articulationId) noexcept {
        return hirari_track_midi_articulation_begin(m_articulationState, articulationId);
    }
    bool setArticulationOutput(uint8_t articulationId, uint8_t slot, uint8_t kind,
                               uint8_t channel, uint8_t a, uint8_t b, uint8_t c) noexcept {
        return hirari_track_midi_articulation_set_output(
            m_articulationState, articulationId, slot, kind, channel, a, b, c);
    }
    bool finishArticulationMapUpdate(uint8_t articulationId, uint8_t count) noexcept {
        return hirari_track_midi_articulation_finish(m_articulationState, articulationId, count);
    }
    void getArticulationOutputMap(std::vector<uint32_t>& packed) const {
        const size_t required = hirari_track_midi_articulation_pack(
            m_articulationState, nullptr, 0);
        packed.resize(required);
        if (required != 0) {
            (void)hirari_track_midi_articulation_pack(
                m_articulationState, packed.data(), packed.size());
        }
    }
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

    bool prepareInput(const AudioBuffer& input, uint32_t sz) {
        if (sz == 0 || input.getNumChannels() == 0 || input.getNumSamples() < sz ||
            m_activeBuffer.getNumChannels() == 0 || m_activeBuffer.getNumSamples() < sz) {
            return false;
        }
        if (&input == &m_activeBuffer) return true;
        clearWorkBuffer(sz);
        m_activeBuffer.addFrom(input, sz);
        return true;
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

    bool processInto(AudioBuffer& output, uint32_t sz, uint64_t playhead,
                     OfflineRenderTap tap = OfflineRenderTap::PostFader) {
        if (sz == 0 || output.getNumChannels() == 0 || output.getNumSamples() < sz ||
            m_activeBuffer.getNumChannels() < 2 || m_activeBuffer.getNumSamples() < sz) {
            return false;
        }
        process(m_activeBuffer, sz, playhead, false);
        const AudioBuffer* source = &m_activeBuffer;
        if (tap == OfflineRenderTap::PreInsert) source = &m_preInsertBuffer;
        else if (tap == OfflineRenderTap::PreFader) source = &m_preFaderBuffer;
        if (source->getNumChannels() < 2 || source->getNumSamples() < sz) return false;
        output.addFrom(*source, sz);
        return true;
    }

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
    static HirariNativeRegionAddMetadata regionAddMetadataForRust(const Region& region) {
        static_assert(std::is_standard_layout_v<HirariNativeRegionAddMetadata>);
        static_assert(offsetof(HirariNativeRegionAddMetadata, clip_gain) == 60);
        static_assert(offsetof(HirariNativeRegionAddMetadata, warp_ratio) == 64);
        static_assert(offsetof(HirariNativeRegionAddMetadata, pitch_semitones) == 72);
        static_assert(sizeof(HirariNativeRegionAddMetadata) == 80);
        return {region.start, region.len, region.sourceLength, region.sourceOffset,
                region.baseStart, region.baseSourceOffset, region.baseLength,
                region.loopCount, region.clipGain, region.warpRatio, region.pitchSemitones};
    }

    static void applyRegionAddMetadata(Region& region,
                                       const HirariNativeRegionAddMetadata& metadata) {
        region.start = metadata.start;
        region.len = metadata.length;
        region.sourceLength = metadata.source_length;
        region.sourceOffset = metadata.source_offset;
        region.baseStart = metadata.base_start;
        region.baseSourceOffset = metadata.base_source_offset;
        region.baseLength = metadata.base_length;
        region.loopCount = metadata.loop_count;
        region.clipGain = metadata.clip_gain;
        region.warpRatio = metadata.warp_ratio;
        region.pitchSemitones = metadata.pitch_semitones;
    }

    static HirariNativeRegionGeometry regionGeometryForRust(const Region& region) {
        static_assert(std::is_standard_layout_v<HirariNativeRegionGeometry>);
        static_assert(offsetof(HirariNativeRegionGeometry, warp_ratio) == 80);
        static_assert(offsetof(HirariNativeRegionGeometry, source_sample_rate) == 88);
        return {region.start, region.len, region.sourceLength, region.sourceOffset,
                region.baseStart, region.baseSourceOffset, region.baseLength,
                region.loopCount, region.fadeInSamples, region.fadeOutSamples,
                region.warpRatio, region.sourceSampleRate, region.timelineSampleRate};
    }

    static std::vector<HirariRegionRangeEdit> copyRangeEditsToRust(
        const std::vector<Region::RangeEdit>& edits) {
        static_assert(sizeof(Region::RangeEdit) == sizeof(HirariRegionRangeEdit));
        static_assert(offsetof(Region::RangeEdit, start) == offsetof(HirariRegionRangeEdit, start));
        static_assert(offsetof(Region::RangeEdit, end) == offsetof(HirariRegionRangeEdit, end));
        static_assert(offsetof(Region::RangeEdit, gain) == offsetof(HirariRegionRangeEdit, gain));
        static_assert(offsetof(Region::RangeEdit, fadeIn) == offsetof(HirariRegionRangeEdit, fade_in));
        static_assert(offsetof(Region::RangeEdit, fadeOut) == offsetof(HirariRegionRangeEdit, fade_out));
        std::vector<HirariRegionRangeEdit> output;
        output.reserve(edits.size());
        for (const auto& edit : edits) {
            output.push_back({edit.start, edit.end, edit.gain, edit.fadeIn, edit.fadeOut});
        }
        return output;
    }

    static std::vector<Region::RangeEdit> copyRangeEditsFromRust(
        const std::vector<HirariRegionRangeEdit>& edits) {
        std::vector<Region::RangeEdit> output;
        output.reserve(edits.size());
        for (const auto& edit : edits) {
            output.push_back({edit.start, edit.end, edit.gain, edit.fade_in, edit.fade_out});
        }
        return output;
    }

    std::string copyExpressionMapJson(uint8_t mapKind) const {
        size_t required = hirari_track_midi_expression_map_copy(
            m_articulationState, mapKind, nullptr, 0);
        std::string result(required, '\0');
        for (;;) {
            const size_t copied = hirari_track_midi_expression_map_copy(
                m_articulationState, mapKind,
                reinterpret_cast<uint8_t*>(result.data()), result.size());
            if (copied <= result.size()) {
                result.resize(copied);
                return result;
            }
            result.resize(copied);
        }
    }

    void process(AudioBuffer& buffer, uint32_t sz, uint64_t playhead, bool clearInput,
                 bool deferOutput = false) {
        constexpr size_t kMaxPluginAutomationEventsPerBlock =
            Hirari::Core::Plugins::SandboxProtocol::kMaxParameterChanges;
        static_assert(sizeof(DSP::TimedParameterEvent) == 16);
        static_assert(offsetof(DSP::TimedParameterEvent, sampleOffset) == 8);
        static_assert(sizeof(Hirari::Core::ProjectSerializer::AutomationPoint) == 16);
        std::array<DSP::TimedParameterEvent, kMaxPluginAutomationEventsPerBlock>
            pluginAutomationEvents{};
        size_t pluginAutomationEventCount = 0;
        const auto* regions = m_regionSnapshot.load(std::memory_order_acquire);
        const auto* trackDelayAutomation = m_trackDelayAutomation.load(std::memory_order_acquire);
        const auto* pluginAutomation = m_pluginAutomation.load(std::memory_order_acquire);
        const AudioBuffer* frozen = m_frozenAudio.load(std::memory_order_acquire);
        uint32_t trackDelaySamples = m_trackDelaySamples.load(std::memory_order_relaxed);
        struct MidiPreparationContext {
            Track* track;
            AudioBuffer* buffer;
        } midiContext{this, &buffer};
        const HirariTrackProcessRequest request{
            buffer.getArrayOfWritePointers(), buffer.getNumChannels(), buffer.getNumSamples(),
            sz, playhead, static_cast<uint8_t>(clearInput),
            static_cast<uint8_t>(m_muted.load(std::memory_order_relaxed)),
            static_cast<uint8_t>(m_frozen.load(std::memory_order_acquire)),
            frozen ? frozen->getArrayOfReadPointers() : nullptr,
            frozen ? frozen->getNumChannels() : 0,
            frozen ? frozen->getNumSamples() : 0,
            m_frozenTotalSamples.load(std::memory_order_relaxed),
            regions && !regions->renderInputs.empty() ? regions->renderInputs.data() : nullptr,
            regions ? regions->renderInputs.size() : 0,
            static_cast<uint8_t>(regions != nullptr),
            static_cast<uint8_t>(m_phaseInvert.load(std::memory_order_relaxed)),
            trackDelayAutomation && !trackDelayAutomation->empty()
                ? trackDelayAutomation->data() : nullptr,
            trackDelayAutomation ? trackDelayAutomation->size() : 0,
            &m_trackDelayAutomationHint, &trackDelaySamples, kMaxPdcDelaySamples,
            pluginAutomation ? pluginAutomation->views.data() : nullptr,
            pluginAutomation ? pluginAutomation->views.size() : 0,
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

        // Activate control-thread edits only at a block boundary. The chain's
        // syncToAudioThread() uses try_lock(), so the audio thread never waits
        // for a UI/plugin-edit lock here.
        m_effectChain.syncToAudioThread();
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
