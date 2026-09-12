    std::unique_ptr<AudioBuffer> bounceL;
    std::unique_ptr<AudioBuffer> bounceR;
    if (format == 0) {
        pcmStream = std::make_unique<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter>(
            outputPath, totalSamples, outputRate);
        if (!pcmStream->isOpen()) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_OPEN_FAILED");
            return false;
        }
    } else {
        bounceL = std::make_unique<AudioBuffer>(1, totalSamples);
        bounceR = std::make_unique<AudioBuffer>(1, totalSamples);
    }
    AudioBuffer blockBuffer(2, 1024);
    bool usedDirectTrackFallback = false;
    
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
    
    m_playhead.store(rangeActive ? rangeStart : 0);
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
    
    for (uint32_t s = 0; s < totalSamples; s += 1024) {
        if (!waitForBounceControl()) {
            m_playhead.store(originalPlayhead);
            m_isPlaying.store(wasPlaying);
            finishOfflineRender();
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
            return false;
        }
        uint32_t sz = std::min(1024u, totalSamples - s);
        processBlock(blockBuffer, 0, sz);
        // A stale callback snapshot can fail closed during a device handoff.
        // If the prepared project clearly contains audio but the graph
        // produced an entirely empty block, render the immutable track
        // snapshots directly as a bounded recovery path. This keeps export
        // useful while the realtime graph publishes its next generation and
        // avoids silently shipping a valid-looking empty WAV.
        bool blockHasSignal = false;
        for (uint32_t sample = 0; sample < sz && !blockHasSignal; ++sample) {
            blockHasSignal = std::abs(blockBuffer.getReadPointer(0)[sample]) > 1.0e-12f ||
                              std::abs(blockBuffer.getReadPointer(1)[sample]) > 1.0e-12f;
        }
        if (!blockHasSignal && projectEnd > 0) {
            usedDirectTrackFallback = true;
            AudioBuffer directTrackBuffer(2, 1024);
            std::lock_guard<std::mutex> lock(m_tracksMutex);
            for (const auto& track : m_tracks) {
                if (!track || track->getType() == Track::Type::Bus ||
                    track->getType() == Track::Type::Midi) continue;
                directTrackBuffer.clear(sz);
                if (track->processInto(directTrackBuffer, sz,
                                       (rangeActive ? rangeStart : 0) + s))
                    blockBuffer.addFrom(directTrackBuffer, sz);
            }
        }
        if (format == 0) {
            if (!pcmStream->writeFrames(blockBuffer.getReadPointer(0),
                                        blockBuffer.getReadPointer(1), sz)) {
                m_playhead.store(originalPlayhead);
                m_isPlaying.store(wasPlaying);
                finishOfflineRender();
                m_bounceProgress.store(0.0f, std::memory_order_release);
                m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
                ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_STREAM_WRITE_FAILED");
                return false;
            }
        } else {
            std::memcpy(bounceL->getWritePointer(0) + s, blockBuffer.getReadPointer(0), sz * sizeof(float));
            std::memcpy(bounceR->getWritePointer(0) + s, blockBuffer.getReadPointer(1), sz * sizeof(float));
        }
        m_bounceProgress.store(
            static_cast<float>(s + sz) / static_cast<float>(totalSamples),
            std::memory_order_release);
    }
    
    m_playhead.store(originalPlayhead); 
    m_isPlaying.store(wasPlaying);
    
    // The streaming writer owns PCM16 publication. It still participates in
    // the same cancellation boundary as the buffered WAVE64 path.
    if (format == 0) {
        bool success = false;
        {
            std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
            if (!m_bounceCancelRequested.load(std::memory_order_acquire)) {
                success = pcmStream->finish();
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
            ::Aura::Core::Diagnostics::LogBuffer::post(0, 0,
                cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_STREAM_PUBLISH_FAILED");
            return false;
        }
        finishOfflineRender();
        if (usedDirectTrackFallback) {
            ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_DIRECT_TRACK_FALLBACK_USED");
        }
        m_bounceProgress.store(1.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Completed), std::memory_order_release);
        return true;
    }

    // A thread hash alone is not a publication identity: a later render in
    // another process (or a recycled worker thread) can produce the same
    // value.  Include a process-local monotonic token so a stale renderer can
    // never accidentally target a newer temporary pathname.
    static std::atomic<uint64_t> renderPublicationSequence{0};
    const uint64_t publicationSequence = renderPublicationSequence.fetch_add(
        1, std::memory_order_relaxed) + 1;
    const std::string temporaryOutputPath = outputPath + ".tmp-render-" +
        std::to_string(owner) + "-" + std::to_string(publicationSequence);
    // Cancellation can arrive after the last audio block but before the
    // temporary file is published.  Never let a cancelled render replace a
    // newer result at the destination.
    if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
        m_playhead.store(originalPlayhead);
        m_isPlaying.store(wasPlaying);
        finishOfflineRender();
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
        return false;
    }
    bool success = format == 1
        ? ::Aura::Core::Utils::WavWriter::writeWave64(
            temporaryOutputPath, bounceL->getReadPointer(0), bounceR->getReadPointer(0), totalSamples, outputRate)
        : ::Aura::Core::Utils::WavWriter::write(
            temporaryOutputPath, bounceL->getReadPointer(0), bounceR->getReadPointer(0), totalSamples, outputRate);
    if (success) {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
            success = false;
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
        } else {
            std::error_code publishError;
            std::filesystem::rename(temporaryOutputPath, outputPath, publishError);
            success = !publishError;
        }
    }
    if (!success) {
        std::error_code cleanupError;
        std::filesystem::remove(temporaryOutputPath, cleanupError);
    }
    if (!success) {
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_FAILED");
    }
    finishOfflineRender();
    m_bounceProgress.store(success ? 1.0f : 0.0f, std::memory_order_release);
    const bool cancelled = m_bounceCancelRequested.load(std::memory_order_acquire);
    m_bounceState.store(
        static_cast<uint32_t>(cancelled ? BounceState::Cancelled
                                        : (success ? BounceState::Completed : BounceState::Failed)),
        std::memory_order_release);
    return success;
}

bool AuraUnifiedEngine::bounce_project_async(std::string_view path, uint32_t format) {
    if (format > 1 || path.empty()) {
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
        ::Aura::Core::Diagnostics::LogBuffer::post(
            success ? 2 : (cancelled ? 1 : 0), 0,
            success ? "BOUNCE_COMPLETE" : (cancelled ? "BOUNCE_CANCELLED" : "BOUNCE_FAILED"));
        m_bounceRunning.store(false, std::memory_order_release);
    });
    return true;
}

float AuraUnifiedEngine::get_bounce_progress() const noexcept {
    return std::clamp(m_bounceProgress.load(std::memory_order_acquire), 0.0f, 1.0f);
}

uint32_t AuraUnifiedEngine::get_bounce_state() const noexcept {
    return m_bounceState.load(std::memory_order_acquire);
}

bool AuraUnifiedEngine::cancel_bounce() noexcept {
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

bool AuraUnifiedEngine::pause_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Rendering)) return false;
    m_bouncePauseRequested.store(true, std::memory_order_release);
    return true;
}

bool AuraUnifiedEngine::resume_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (state != static_cast<uint32_t>(BounceState::Paused)) return false;
    m_bouncePauseRequested.store(false, std::memory_order_release);
    return true;
}

bool AuraUnifiedEngine::set_project_scale(int32_t root, int32_t type) {
    if (root < 0 || root > 11 || type < 0 || type > 32) return false;
    m_rootNote.store(root, std::memory_order_relaxed);
    m_scaleType.store(type, std::memory_order_relaxed);
    return true;
}

bool AuraUnifiedEngine::set_articulation_map(uint32_t trackId, uint32_t mapHash) {
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
