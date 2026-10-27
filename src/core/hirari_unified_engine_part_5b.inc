    // Public bounce formats map onto the Rust stream ABI: PCM16, WAVE64 float,
    // float32 RIFF/RF64, PCM24.
    constexpr uint8_t kStreamFormats[4] = {1, 3, 0, 2};
    streamOut.handle = hirari_bounce_stream_create(
        reinterpret_cast<const uint8_t*>(path.data()), path.size(), totalSamples,
        outputRate, kStreamFormats[format]);
    if (!streamOut.handle) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_OPEN_FAILED");
        return false;
    }
    // Tracks size their per-track buffers to the engine block size, and
    // Track::canProcess() silently skips any block larger than that. A fixed
    // 1024-sample render block therefore bounced every track prepared at a
    // smaller device block size (256/512) as full-length silence. Render in
    // chunks that never exceed the prepared block size so offline output
    // matches the live graph. The chunk size is deterministic per engine
    // configuration, keeping bounces reproducible.
    const uint32_t engineBlockSize = get_block_size();
    const uint32_t renderChunk =
        engineBlockSize > 0 ? std::min(1024u, engineBlockSize) : 1024u;
    AudioBuffer blockBuffer(2, renderChunk);
    // The render must be reproducible: two bounces of the same project have to
    // produce the same file. The dither seed therefore comes from a fixed pair
    // of constants rather than the wall clock. The channels use different
    // seeds so their noise stays decorrelated while both remain deterministic.
    ::Hirari::DSP::Utils::TPDFDither ditherLeft{Hirari::DSP::Utils::kTpdfDitherLeftSeed};
    ::Hirari::DSP::Utils::TPDFDither ditherRight{Hirari::DSP::Utils::kTpdfDitherRightSeed};

    uint64_t originalPlayhead = m_playhead.load();
    bool wasPlaying = m_isPlaying.load();
    // The render path temporarily drives the live transport.  Every return
    // path below must restore the caller's transport state, including an
    // allocation failure or an exception from a plug-in/render callback.
    struct OfflinePlaybackStateGuard final {
        std::atomic<uint64_t>* playhead;
        std::atomic<bool>* playing;
        uint64_t originalPlayhead;
        bool originalPlaying;
        ~OfflinePlaybackStateGuard() noexcept {
            playhead->store(originalPlayhead, std::memory_order_release);
            playing->store(originalPlaying, std::memory_order_release);
        }
    } playbackStateGuard{&m_playhead, &m_isPlaying, originalPlayhead, wasPlaying};

    m_playhead.store(plan.playhead_start);
    m_isPlaying.store(true);
    // Offline renders must be reproducible. Clear the master limiter's
    // look-ahead history so a previous playback/render cannot contaminate the
    // first blocks of this bounce.
    if (m_masterLimiter) m_masterLimiter->reset();
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (track) track->resetForOfflineRender();
        }
    }

    for (uint32_t s = 0; s < totalSamples; s += renderChunk) {
        if (!waitForBounceControl()) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
            return false;
        }
        uint32_t sz = std::min(renderChunk, totalSamples - s);
        processBlock(blockBuffer, 0, sz);
        // Silence is valid output from the actual project graph. Never
        // replace it with direct track rendering: that would bypass solo,
        // routing, sends, and bus processing, and advance stateful processors
        // a second time after processBlock already rendered them.
        if (format == 0 || format == 3) {
            float* left = blockBuffer.getWritePointer(0);
            float* right = blockBuffer.getWritePointer(1);
            const float depthScale = format == 0 ? 256.0f : 1.0f;
            for (uint32_t sample = 0; sample < sz; ++sample) {
                left[sample] += ditherLeft.process() * depthScale;
                right[sample] += ditherRight.process() * depthScale;
            }
        }
        const float* channels[2] = {
            blockBuffer.getReadPointer(0), blockBuffer.getReadPointer(1)};
        if (!hirari_bounce_stream_write(streamOut.handle, channels, sz)) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_WRITE_FAILED");
            return false;
        }
        m_bounceProgress.store(
            static_cast<float>(s + sz) / static_cast<float>(totalSamples),
            std::memory_order_release);
    }

    m_playhead.store(originalPlayhead);
    m_isPlaying.store(wasPlaying);

    // Each streaming writer participates in the same cancellation boundary
    // and publishes only a complete, synced file.
    {
        bool success = false;
        {
            std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
            if (!m_bounceCancelRequested.load(std::memory_order_acquire)) {
                success = hirari_bounce_stream_finish(streamOut.handle) != 0;
            }
        }
        if (!success) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            const bool cancelled = m_bounceCancelRequested.load(std::memory_order_acquire);
            m_bounceState.store(static_cast<uint32_t>(cancelled ? BounceState::Cancelled : BounceState::Failed),
                                std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0,
                cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_STREAM_PUBLISH_FAILED");
            return false;
        }
        finishOfflineRender();
        m_bounceProgress.store(1.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Completed), std::memory_order_release);
        return true;
    }
}

bool HirariUnifiedEngine::bounce_project_async(std::string_view path, uint32_t format) {
    if (format > 3 || path.empty()) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        return false;
    }
    std::lock_guard<std::mutex> bounceLifecycleLock(m_bounceLifecycleMutex);
    if (m_bounceRunning.load(std::memory_order_acquire)) {
        const uint32_t state = m_bounceState.load(std::memory_order_acquire);
        const bool terminal = state == static_cast<uint32_t>(BounceState::Completed) ||
                              state == static_cast<uint32_t>(BounceState::Failed) ||
                              state == static_cast<uint32_t>(BounceState::Cancelled);
        if (!terminal) return false;
        // A terminal state is published just before the worker wrapper clears
        // m_bounceRunning. Join here so a retry cannot be rejected during
        // that narrow retirement window.
        if (m_bounceThread.joinable()) m_bounceThread.join();
        m_bounceRunning.store(false, std::memory_order_release);
    } else if (m_bounceThread.joinable()) {
        m_bounceThread.join();
    }
    m_bounceRunning.store(true, std::memory_order_release);
    m_bounceCancelRequested.store(false, std::memory_order_release);
    m_bouncePauseRequested.store(false, std::memory_order_release);
    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Queued), std::memory_order_release);
    const std::string outputPath(path);
    m_bounceThread = std::thread([this, outputPath, format]() {
        const bool success = bounce_project(outputPath, format);
        const bool cancelled = get_bounce_state() == static_cast<uint32_t>(BounceState::Cancelled);
        ::Hirari::Core::Diagnostics::LogBuffer::post(
            success ? 2 : (cancelled ? 1 : 0), 0,
            success ? "BOUNCE_COMPLETE" : (cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_FAILED"));
        m_bounceRunning.store(false, std::memory_order_release);
    });
    return true;
}

float HirariUnifiedEngine::get_bounce_progress() const noexcept {
    return std::clamp(m_bounceProgress.load(std::memory_order_acquire), 0.0f, 1.0f);
}

uint32_t HirariUnifiedEngine::get_bounce_state() const noexcept {
    return m_bounceState.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::cancel_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Queued) &&
        state != static_cast<uint32_t>(BounceState::Rendering) &&
        state != static_cast<uint32_t>(BounceState::Paused)) {
        return false;
    }
    {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        m_bounceCancelRequested.store(true, std::memory_order_release);
        m_bouncePauseRequested.store(false, std::memory_order_release);
    }
    return true;
}

bool HirariUnifiedEngine::pause_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Rendering)) return false;
    m_bouncePauseRequested.store(true, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::resume_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Paused)) return false;
    m_bouncePauseRequested.store(false, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::set_project_scale(int32_t root, int32_t type) {
    // The bridge validates the ranges before this is reached.
    m_rootNote.store(root, std::memory_order_relaxed);
    m_scaleType.store(type, std::memory_order_relaxed);
    return true;
}

bool HirariUnifiedEngine::set_articulation_map(uint32_t trackId, uint32_t mapHash) {
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    bool found = false;
    for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
        if (state.activeTracks[i] && state.activeTracks[i]->getId() == trackId) {
            found = true;
            break;
        }
    }
    if (!found) return false;
    m_articulationMapHash.store(mapHash, std::memory_order_relaxed);
    return true;
}

bool HirariUnifiedEngine::set_expression_map(uint32_t trackId,
                                             const std::vector<uint32_t>& packed,
                                             const std::string& json,
                                             uint32_t articulationId) {
    if (!hirari_expression_map_validate_packed(
            packed.data(), packed.size(), json.size(), articulationId)) return false;
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
        auto* track = state.activeTracks[i];
        if (!track || track->getId() != trackId) continue;
        std::vector<uint32_t> beforePacked;
        track->getArticulationOutputMap(beforePacked);
        const std::string beforeJson = track->getExpressionMapJson();
        if (beforePacked == packed && beforeJson == json) return true;
        const auto applySnapshot = [this, trackId](const std::vector<uint32_t>& values,
                                                   const std::string& serialized) {
            const auto& current = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
            for (uint32_t index = 0; index < current.trackCount && index < 256; ++index) {
                auto* target = current.activeTracks[index];
                if (!target || target->getId() != trackId) continue;
                struct ApplyContext { Track* track; } context{target};
                const bool applied = hirari_expression_map_visit_packed(
                    values.data(), values.size(), &context,
                    +[](void* opaque, uint8_t operation, uint8_t id, uint8_t slot,
                        uint32_t kind, uint32_t channel, uint32_t a, uint32_t b,
                        uint32_t c, uint8_t outputCount) -> bool {
                        auto* track = static_cast<ApplyContext*>(opaque)->track;
                        if (operation == 0) return track->beginArticulationMapUpdate(id);
                        if (operation == 1) return track->setArticulationOutput(
                            id, slot, static_cast<uint8_t>(kind), static_cast<uint8_t>(channel),
                            static_cast<uint8_t>(a), static_cast<uint8_t>(b), static_cast<uint8_t>(c));
                        if (operation == 2) return track->finishArticulationMapUpdate(id, outputCount);
                        return false;
                    });
                if (!applied) return;
                target->setExpressionMapJson(serialized);
                return;
            }
        };
        applySnapshot(packed, json);
        const std::string actionName = "Edit MIDI Expression Map T" + std::to_string(trackId) +
            (articulationId == 0 ? std::string{} : " A" + std::to_string(articulationId));
        m_undoManager.recordAppliedAction(actionName,
            [applySnapshot, beforePacked, beforeJson] { applySnapshot(beforePacked, beforeJson); },
            [applySnapshot, afterPacked = packed, afterJson = json] { applySnapshot(afterPacked, afterJson); },
            true);
        return true;
    }
    return false;
}

bool HirariUnifiedEngine::set_expression_map_pro(uint32_t trackId, const std::string& json) {
    if (trackId == 0 || json.size() > 1024 * 1024) return false;
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
        auto* track = state.activeTracks[i];
        if (!track || track->getId() != trackId) continue;
        const std::string before = track->getExpressionMapProJson();
        if (before == json) return true;
        const auto applySnapshot = [this, trackId](const std::string& snapshot) {
            const auto& current = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
            for (uint32_t index = 0; index < current.trackCount && index < 256; ++index) {
                auto* target = current.activeTracks[index];
                if (target && target->getId() == trackId) {
                    target->setExpressionMapProJson(snapshot);
                    return;
                }
            }
        };
        track->setExpressionMapProJson(json);
        m_undoManager.recordAppliedAction(
            "Edit MIDI Expression Map Pro T" + std::to_string(trackId),
            [applySnapshot, before] { applySnapshot(before); },
            [applySnapshot, after = json] { applySnapshot(after); }, true);
        return true;
    }
    return false;
}
