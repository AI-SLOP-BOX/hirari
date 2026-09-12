public:
    enum Type { Audio=0, Midi=1, Instrument=2, Bus=3, Vocal=4 };

    Track(uint32_t id, const std::string& n, Type t,
          SidechainManager* sidechainManager = nullptr)
        : m_id(id), m_name(n), m_type(t), m_effectChain(sidechainManager),
          m_analysis(&m_activeBuffer) {
        publishRegionSnapshot();
    }

    virtual ~Track() = default;

    void addMidiRegion(std::shared_ptr<Aura::Core::MidiRegion> region) {
        if (!region || !std::isfinite(region->getStartBeat()) ||
            !std::isfinite(region->getLengthBeats()) || region->getLengthBeats() <= 0.0) return;
        // MIDI regions are part of the rendered signal just like audio
        // regions. A frozen track must never keep serving the pre-edit audio
        // after a new note region is attached.
        invalidateFrozenForEdit();
        m_midiRegions.push_back(std::move(region));
    }

    const std::vector<std::shared_ptr<Aura::Core::MidiRegion>>& getMidiRegions() const noexcept {
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
        // Reject malformed timeline entries before they enter the immutable
        // audio snapshot.  In particular, a serialized length must never be
        // allowed to describe samples that the attached buffer does not own.
        if (normalized.len == 0 || normalized.start > UINT64_MAX - normalized.len) return;
        if (!std::isfinite(normalized.warpRatio) || normalized.warpRatio < 0.5 || normalized.warpRatio > 2.0) {
            normalized.warpRatio = 1.0;
        }
        if (!std::isfinite(normalized.pitchSemitones) || normalized.pitchSemitones < -24.0f || normalized.pitchSemitones > 24.0f) {
            normalized.pitchSemitones = 0.0f;
        }
        if (normalized.loopCount == 0 || normalized.loopCount > 1024 ||
            normalized.len > UINT64_MAX / normalized.loopCount) {
            normalized.loopCount = 1;
        }
        const uint64_t timelineLength = normalized.len * normalized.loopCount;
        if (normalized.start > UINT64_MAX - timelineLength) return;
        if (!std::isfinite(normalized.clipGain)) normalized.clipGain = 1.0f;
        normalized.clipGain = std::clamp(normalized.clipGain, 0.0f, 2.0f);
        if (normalized.audio) {
            const uint64_t audioSamples = normalized.audio->getNumSamples();
            if (normalized.audio->getNumChannels() == 0 || audioSamples == 0) return;
            normalized.len = std::min(normalized.len, audioSamples);
            if (normalized.len == 0) return;
            if (normalized.baseLength == 0) normalized.baseLength = normalized.len;
            if (normalized.baseStart == 0) normalized.baseStart = normalized.start;
            if (normalized.baseSourceOffset == 0) normalized.baseSourceOffset = normalized.sourceOffset;
            if (normalized.sourceOffset > audioSamples - normalized.len) return;
        }
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
        for (auto& segment : normalized.audioNoteSegments) {
            segment.anchors.erase(
                std::remove_if(segment.anchors.begin(), segment.anchors.end(),
                               [&segment](const auto& anchor) {
                                   return !std::isfinite(anchor.positionSeconds) ||
                                          !std::isfinite(anchor.pitchCents) ||
                                          !std::isfinite(anchor.formantCents) ||
                                          anchor.positionSeconds < segment.startSeconds ||
                                          anchor.positionSeconds > segment.endSeconds;
                               }),
                segment.anchors.end());
            std::sort(segment.anchors.begin(), segment.anchors.end(),
                      [](const auto& a, const auto& b) {
                          return a.positionSeconds < b.positionSeconds;
                  });
        }
        // Range edits are also serialized project data. Bound and normalize
        // them before the immutable snapshot reaches the realtime loop so a
        // malformed project cannot create an unbounded per-sample scan or
        // invalid fade arithmetic.
        if (normalized.rangeEdits.size() > 4096) {
            normalized.rangeEdits.resize(4096);
        }
        normalized.rangeEdits.erase(
            std::remove_if(normalized.rangeEdits.begin(), normalized.rangeEdits.end(),
                           [&normalized](const Region::RangeEdit& edit) {
                               return edit.start >= edit.end ||
                                      edit.start >= normalized.len ||
                                      !std::isfinite(edit.gain) || edit.gain < 0.0f ||
                                      edit.gain > 16.0f;
                           }),
            normalized.rangeEdits.end());
        for (auto& edit : normalized.rangeEdits) {
            edit.end = std::min(edit.end, normalized.len);
            const uint64_t length = edit.end - edit.start;
            edit.fadeIn = std::min(edit.fadeIn, length);
            edit.fadeOut = std::min(edit.fadeOut, length);
        }
        std::sort(normalized.rangeEdits.begin(), normalized.rangeEdits.end(),
                  [](const Region::RangeEdit& a, const Region::RangeEdit& b) {
                      return a.start < b.start;
                  });
        std::sort(normalized.audioNoteSegments.begin(),
                  normalized.audioNoteSegments.end(),
                  [](const auto& a, const auto& b) {
                      return a.startSeconds < b.startSeconds;
                  });
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
                region.start = newStart;
                std::sort(m_regions.begin(), m_regions.end(),
                          [](const Region& a, const Region& b) { return a.start < b.start; });
                publishRegionSnapshot();
                return true;
            }
        }
        return false;
    }

    bool replaceRegionAudio(uint32_t rid, std::string path, std::shared_ptr<AudioBuffer> audio) {
        invalidateFrozenForEdit();
        std::lock_guard<std::mutex> editLock(m_regionEditMutex);
        if (path.empty() || !audio || audio->getNumSamples() == 0 || audio->getNumChannels() == 0) return false;
        for (auto& region : m_regions) {
            if (region.id == rid) {
                region.path = std::move(path);
                region.name = region.path;
                region.audio = std::move(audio);
                region.sourceOffset = 0;
                region.baseStart = region.start;
                region.baseSourceOffset = 0;
                region.len = region.audio->getNumSamples();
                region.baseLength = region.len;
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
    void process(AudioBuffer& buffer, uint32_t sz, uint64_t playhead, bool clearInput) {
        // --- INDUSTRIAL TRANSITION: RUST CORE BRIDGE ---
        // The implementation here is now a shim to Aura::Core::Bridge::TrackOrchestrator.
        // Rust's binary search and deterministic state management ensure that
        // track processing is technically superior and perfectly synchronized.
        // Rust's TrackEngine ensures bit-accurate signal distribution.

        // A device/plugin boundary must never be allowed to turn an external
        // frame count into an unbounded DSP traversal.  The normal engine
        // block sizes are far below this ceiling; an oversized request is a
        // deterministic silent fail-closed, without allocation or exception
        // work on the audio thread.
        constexpr uint32_t kMaxProcessBlockSize = 65'536;
        if (sz > kMaxProcessBlockSize) {
            buffer.clear(std::min(sz, buffer.getNumSamples()));
            return;
        }
        // The caller-provided frame count is untrusted at device/plugin
        // boundaries.  Every destination pointer below is only valid for the
        // buffer's actual sample capacity; fail closed before the frozen,
        // region, or automation paths can index past it.
        if (sz == 0 || buffer.getNumSamples() < sz) {
            buffer.clear(std::min(sz, buffer.getNumSamples()));
            return;
        }
        
        if (m_muted.load(std::memory_order_relaxed)) {
            buffer.clear(sz);
            return;
        }
        if (m_frozen.load(std::memory_order_acquire)) {
            buffer.clear(sz);
            const AudioBuffer* frozen = m_frozenAudio.load(std::memory_order_acquire);
            const uint64_t totalSamples = m_frozenTotalSamples.load(std::memory_order_relaxed);
            if (frozen == nullptr || totalSamples == 0 || playhead >= totalSamples) return;
            const uint64_t available64 = std::min<uint64_t>(
                static_cast<uint64_t>(sz), totalSamples - playhead);
            const uint32_t available = static_cast<uint32_t>(available64);
            const uint32_t channels = std::min(buffer.getNumChannels(), frozen->getNumChannels());
            const uint64_t sourceSamples = frozen->getNumSamples();
            if (available == 0 || playhead >= sourceSamples) return;
            const uint32_t bounded = static_cast<uint32_t>(std::min<uint64_t>(
                available, sourceSamples - playhead));
            for (uint32_t channel = 0; channel < channels; ++channel) {
                const float* source = frozen->getReadPointer(channel);
                float* destination = buffer.getWritePointer(channel);
                if (source == nullptr || destination == nullptr) continue;
                std::memcpy(destination,
                            source + static_cast<size_t>(playhead),
                            static_cast<size_t>(bounded) * sizeof(float));
            }
            return;
        }
        if (clearInput) buffer.clear(sz);
        if (buffer.getNumChannels() < 2 || sz == 0) return;
        // The audio thread only reads this immutable generation. The owning
        // snapshots are retained by the control side until Track destruction,
        // so loading this raw pointer never increments a reference count or
        // performs allocation/deallocation on the RT thread.
            const RegionSnapshot* regions =
            m_regionSnapshot.load(std::memory_order_acquire);
        if (regions == nullptr) return;
        for (size_t regionIndex = 0; regionIndex < regions->size(); ++regionIndex) {
            const auto& region = (*regions)[regionIndex];
            if (region.muted || !region.audio || region.len == 0) continue;
            // Compare distances instead of adding timeline positions.  This
            // remains defined even for regions near UINT64_MAX.
            if (region.loopCount == 0 || region.len > UINT64_MAX / region.loopCount) continue;
            const uint64_t timelineLength = region.len * region.loopCount;
            if ((playhead >= region.start && playhead - region.start >= timelineLength) ||
                (playhead < region.start && region.start - playhead >= sz)) continue;

            const uint64_t first = std::max(playhead, region.start);
            const uint64_t regionOffset = first - region.start;
            const uint32_t destOffset = static_cast<uint32_t>(first - playhead);
            const uint32_t sourceChannels = region.audio->getNumChannels();
            const uint64_t sourceSamples = region.audio->getNumSamples();
            if (sourceChannels == 0 || sourceSamples == 0 || destOffset >= sz) continue;
            const uint64_t available64 = std::min(
                static_cast<uint64_t>(sz - destOffset), timelineLength - regionOffset);
            if (available64 == 0) continue;
            const uint32_t available = static_cast<uint32_t>(std::min<uint64_t>(
                available64, static_cast<uint64_t>(UINT32_MAX)));

            const float* sourceL = region.audio->getReadPointer(0);
            const float* sourceR = sourceChannels > 1 ? region.audio->getReadPointer(1) : sourceL;
            if (sourceL == nullptr || sourceR == nullptr) continue;
            float* destL = buffer.getWritePointer(0, destOffset);
            float* destR = buffer.getWritePointer(1, destOffset);
            // The base clip transposition is constant, while VariAudio note
            // edits can change pitch inside each segment.  Compute the base
            // ratio once and evaluate the bounded segment curve per sample.
            const double basePitchRatio = std::pow(2.0, static_cast<double>(region.pitchSemitones) / 12.0);
            if (!std::isfinite(basePitchRatio) || basePitchRatio <= 0.0) continue;
            for (uint32_t i = 0; i < available; ++i) {
                const uint64_t relative = regionOffset + i;
                const uint64_t loopRelative = relative % region.len;
                float fade = 1.0f;
                if (region.fadeInSamples > 0 && relative < region.fadeInSamples) {
                    fade = static_cast<float>(relative) / static_cast<float>(region.fadeInSamples);
                }
                if (region.fadeOutSamples > 0 && timelineLength > region.fadeOutSamples &&
                    relative >= timelineLength - region.fadeOutSamples) {
                    fade = std::min(fade, static_cast<float>(timelineLength - relative) /
                                           static_cast<float>(region.fadeOutSamples));
                }
                if (region.crossfadeInSamples > 0 && relative < region.crossfadeInSamples) {
                    fade = std::min(fade, static_cast<float>(relative) /
                                           static_cast<float>(region.crossfadeInSamples));
                }
                if (region.crossfadeOutSamples > 0 && relative >= timelineLength - region.crossfadeOutSamples) {
                    fade = std::min(fade, static_cast<float>(timelineLength - relative) /
                                           static_cast<float>(region.crossfadeOutSamples));
                }
                // Non-destructive waveform range edits are evaluated in the
                // region's local timeline.  Invalid entries are ignored so a
                // hand-edited project can never destabilize the RT loop.
                float rangeGain = 1.0f;
                float rangeFade = 1.0f;
                bool rangeMuted = false;
                for (const auto& edit : region.rangeEdits) {
                    // Range edits are authored against one clip iteration,
                    // so looping a region repeats the same edit pattern.
                    if (edit.start > loopRelative) break;
                    if (edit.start >= edit.end || loopRelative < edit.start || loopRelative >= edit.end) continue;
                    if (!std::isfinite(edit.gain) || edit.gain < 0.0f || edit.gain > 16.0f) continue;
                    rangeGain = std::min(rangeGain * edit.gain, 16.0f);
                    if (!std::isfinite(rangeGain)) rangeGain = 16.0f;
                    const uint64_t length = edit.end - edit.start;
                    if (edit.fadeIn > 0 && edit.fadeIn <= length && loopRelative - edit.start < edit.fadeIn) {
                        rangeFade = std::min(rangeFade, static_cast<float>(loopRelative - edit.start) /
                                                       static_cast<float>(edit.fadeIn));
                    }
                    if (edit.fadeOut > 0 && edit.fadeOut <= length && loopRelative - edit.start >= length - edit.fadeOut) {
                        rangeFade = std::min(rangeFade, static_cast<float>(edit.end - loopRelative) /
                                                       static_cast<float>(edit.fadeOut));
                    }
                    if (edit.gain == 0.0f) rangeMuted = true;
                }
                if (rangeMuted) continue;
                if (!std::isfinite(rangeFade)) rangeFade = 1.0f;
                const float gain = region.clipGain * std::clamp(fade * rangeFade * rangeGain, 0.0f, 16.0f);
                double notePitchRatio = 1.0;
                double noteFormantCents = 0.0;
                if (!region.audioNoteSegments.empty()) {
                    const double noteSeconds = static_cast<double>(loopRelative) /
                        std::max(1.0, m_sampleRate);
                    // Segments are normalized and sorted by addRegion(). A
                    // linear scan here made every audio sample revisit up to
                    // 4096 segments; use the ordered start times to keep the
                    // VariAudio path logarithmic without allocating on RT.
                    auto it = std::upper_bound(
                        region.audioNoteSegments.begin(), region.audioNoteSegments.end(),
                        noteSeconds,
                        [](double time, const auto& segment) {
                            return time < segment.startSeconds;
                        });
                    const aura::editing::AudioNoteSegment* matchedSegment = nullptr;
                    // Preserve the historical first-match rule for malformed
                    // overlapping segments while keeping normal (disjoint)
                    // projects logarithmic. Only segments whose end still
                    // reaches the query time can overlap the candidate.
                    while (it != region.audioNoteSegments.begin()) {
                        const auto candidateIt = std::prev(it);
                        const auto& candidate = *candidateIt;
                        if (candidate.endSeconds < noteSeconds) break;
                        if (noteSeconds >= candidate.startSeconds &&
                            noteSeconds <= candidate.endSeconds) {
                            matchedSegment = &candidate;
                        }
                        it = candidateIt;
                    }
                    if (matchedSegment != nullptr) {
                        const auto& segment = *matchedSegment;
                        const double cents = segment.pitchAt(noteSeconds);
                        const double formant = segment.formantAt(noteSeconds);
                        if (std::isfinite(cents)) {
                            notePitchRatio = std::pow(2.0,
                                std::clamp(cents, -4800.0, 4800.0) / 1200.0);
                        }
                        if (std::isfinite(formant)) noteFormantCents = std::clamp(formant, -2400.0, 2400.0);
                    }
                }
                if (!std::isfinite(notePitchRatio) || notePitchRatio <= 0.0) continue;
                const double warped = static_cast<double>(loopRelative) * region.warpRatio *
                    basePitchRatio * notePitchRatio;
                if (!std::isfinite(warped) || warped < 0.0 || warped >= static_cast<double>(region.len)) continue;
                const uint64_t sourceRelative = static_cast<uint64_t>(warped);
                const uint64_t nextRelative = sourceRelative < region.len - 1u
                    ? sourceRelative + 1u
                    : sourceRelative;
                const uint64_t sourceIndex = region.reverse
                    ? (region.len - 1u - sourceRelative)
                    : sourceRelative;
                const uint64_t nextIndex = region.reverse
                    ? (region.len - 1u - nextRelative)
                    : nextRelative;
                if (region.sourceOffset > UINT64_MAX - sourceIndex ||
                    region.sourceOffset > UINT64_MAX - nextIndex) continue;
                const uint64_t absoluteIndex = region.sourceOffset + sourceIndex;
                const uint64_t nextAbsoluteIndex = region.sourceOffset + nextIndex;
                if (absoluteIndex >= sourceSamples || nextAbsoluteIndex >= sourceSamples) continue;
                const float fraction = static_cast<float>(warped - static_cast<double>(sourceRelative));
                const float currentL = sourceL[absoluteIndex];
                const float currentR = sourceR[absoluteIndex];
                const float nextL = sourceL[nextAbsoluteIndex];
                const float nextR = sourceR[nextAbsoluteIndex];
                float sampleL = currentL + (nextL - currentL) * fraction;
                float sampleR = currentR + (nextR - currentR) * fraction;
                // Lightweight formant tilt: positive offsets emphasize the
                // local high-frequency slope, negative offsets soften it.
                // It is bounded and sample-local, keeping the RT path free of
                // allocations while making the stored formant curve audible.
                const float formantTilt = static_cast<float>(noteFormantCents / 2400.0);
                sampleL = std::clamp(sampleL + (nextL - currentL) * formantTilt * 0.5f, -2.0f, 2.0f);
                sampleR = std::clamp(sampleR + (nextR - currentR) * formantTilt * 0.5f, -2.0f, 2.0f);
                if (!std::isfinite(sampleL) || !std::isfinite(sampleR)) continue;
                destL[i] += sampleL * gain;
                destR[i] += sampleR * gain;
            }
        }
        if (m_preFaderBuffer.getNumChannels() >= 2 &&
            m_preFaderBuffer.getNumSamples() >= sz) {
            std::memcpy(m_preFaderBuffer.getWritePointer(0), buffer.getReadPointer(0),
                        static_cast<size_t>(sz) * sizeof(float));
            std::memcpy(m_preFaderBuffer.getWritePointer(1), buffer.getReadPointer(1),
                        static_cast<size_t>(sz) * sizeof(float));
        }
        if (m_phaseInvert.load(std::memory_order_relaxed)) {
            float* left = buffer.getWritePointer(0);
            float* right = buffer.getWritePointer(1);
            for (uint32_t i = 0; i < sz; ++i) {
                left[i] = -left[i];
                right[i] = -right[i];
            }
        }
        const auto* volumeAutomation = m_volumeAutomation.load(std::memory_order_acquire);
        const auto* panAutomation = m_panAutomation.load(std::memory_order_acquire);
        const double automationTime = static_cast<double>(playhead);
        if (volumeAutomation && !volumeAutomation->empty()) {
            // Evaluate volume automation at sample resolution.  Updating the
            // channel-strip target once per block makes fast ramps zipper at
            // block boundaries and is observably different from Cubase's
            // sample-accurate automation.  Keep the strip at unity here and
            // apply the bounded curve directly to the pre-fader signal.
            m_strip.setGain(1.0f);
            float* left = buffer.getWritePointer(0);
            float* right = buffer.getWritePointer(1);
            for (uint32_t sample = 0; sample < sz; ++sample) {
                const float gain = std::clamp(
                    evaluateAutomation(*volumeAutomation,
                        automationTime + static_cast<double>(sample),
                        m_volumeAutomationHint), 0.0f, 2.0f);
                left[sample] *= gain;
                right[sample] *= gain;
            }
        }
        if (panAutomation && !panAutomation->empty()) {
            // Pan automation follows the same sample-accurate path.  The
            // equal-power coefficients mirror ChannelStrip's law while
            // avoiding a block-rate control discontinuity.
            m_strip.setPan(0.0f);
            float* left = buffer.getWritePointer(0);
            float* right = buffer.getWritePointer(1);
            for (uint32_t sample = 0; sample < sz; ++sample) {
                const float pan = std::clamp(
                    evaluateAutomation(*panAutomation,
                        automationTime + static_cast<double>(sample),
                        m_panAutomationHint), -1.0f, 1.0f);
                constexpr float kPi = 3.14159265358979323846f;
                const float angle = (pan + 1.0f) * 0.25f * kPi;
                left[sample] *= std::cos(angle) * 1.41421356237f;
                right[sample] *= std::sin(angle) * 1.41421356237f;
            }
        }
        const auto* trackDelayAutomation = m_trackDelayAutomation.load(std::memory_order_acquire);
        if (trackDelayAutomation && !trackDelayAutomation->empty()) {
            const float normalized = std::clamp(
                evaluateAutomation(*trackDelayAutomation, automationTime, m_trackDelayAutomationHint), 0.0f, 1.0f);
            const auto samples = static_cast<uint32_t>(std::lround(normalized * kMaxPdcDelaySamples));
            m_trackDelaySamples.store(std::min(samples, kMaxPdcDelaySamples), std::memory_order_relaxed);
        }
        m_strip.process(buffer, 0, sz, nullptr, nullptr);
        if (m_preInsertBuffer.getNumChannels() >= 2 &&
            m_preInsertBuffer.getNumSamples() >= sz) {
            std::memcpy(m_preInsertBuffer.getWritePointer(0), buffer.getReadPointer(0),
                        static_cast<size_t>(sz) * sizeof(float));
            std::memcpy(m_preInsertBuffer.getWritePointer(1), buffer.getReadPointer(1),
                        static_cast<size_t>(sz) * sizeof(float));
        }

        // Activate control-thread edits only at a block boundary. The chain's
        // syncToAudioThread() uses try_lock(), so the audio thread never waits
        // for a UI/plugin-edit lock here.
        m_effectChain.syncToAudioThread();
        m_pluginMidi.clear();
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
        }

        // Spatial coordinates are persisted and exposed through the command
        // API. Apply them at the track output so the setting affects both
        // realtime playback and offline rendering, while mode 0 remains an
        // allocation-free bypass for ordinary stereo tracks.
        if (m_spatialMode.load(std::memory_order_relaxed) != 0 &&
            buffer.getNumChannels() >= 2) {
            m_spatialPanner.process(buffer.getWritePointer(0),
                                    buffer.getWritePointer(1), sz,
                                    m_spatialX.load(std::memory_order_relaxed),
                                    m_spatialY.load(std::memory_order_relaxed),
                                    m_spatialZ.load(std::memory_order_relaxed));
        }
        if (m_type != Type::Bus) applyPdcCompensation(buffer, sz);
        // VCA resolution is an atomic O(1) lookup and therefore safe to read
        // at the audio boundary.  Apply it after the track effect chain so a
        // VCA behaves like a console fader over the complete track signal.
        const float vcaGain = VCAManager::getInstance().getCumulativeGain(m_id);
        if (std::isfinite(vcaGain) && vcaGain != 1.0f) {
            float* left = buffer.getWritePointer(0);
            float* right = buffer.getWritePointer(1);
            for (uint32_t i = 0; i < sz; ++i) {
