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
        if (!hirari_bounce_state_terminal(state)) return false;
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
    if (!hirari_bounce_state_cancellable(state)) return false;
    {
        std::lock_guard<std::mutex> publishLock(m_bouncePublishMutex);
        m_bounceCancelRequested.store(true, std::memory_order_release);
        m_bouncePauseRequested.store(false, std::memory_order_release);
    }
    return true;
}

bool HirariUnifiedEngine::pause_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (!hirari_bounce_state_pausable(state)) return false;
    m_bouncePauseRequested.store(true, std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::resume_bounce() noexcept {
    const uint32_t state = m_bounceState.load(std::memory_order_acquire);
    if (!hirari_bounce_state_resumable(state)) return false;
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



void HirariUnifiedEngine::installExpressionMapEditHost() {
    const HirariExpressionMapEditHostOps ops{
        [](void* host, uint32_t trackId) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const auto& state = engine->m_audioStates[
                engine->m_activeStateIdx.load(std::memory_order_acquire)];
            for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
                auto* track = state.activeTracks[i];
                if (track && track->getId() == trackId) return track->articulationStateHandle();
            }
            return nullptr;
        },
        [](void* host, uint32_t trackId, uint8_t kind, const uint8_t* data,
           size_t length) -> bool {
            if (!host || (length != 0 && !data)) return false;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const auto& state = engine->m_audioStates[
                engine->m_activeStateIdx.load(std::memory_order_acquire)];
            for (uint32_t i = 0; i < state.trackCount && i < 256; ++i) {
                auto* track = state.activeTracks[i];
                if (!track || track->getId() != trackId) continue;
                std::string json(reinterpret_cast<const char*>(data), length);
                if (kind == 0) track->setExpressionMapJson(std::move(json));
                else track->setExpressionMapProJson(std::move(json));
                return true;
            }
            return false;
        },
    };
    hirari_expression_map_edit_host_install(&ops);
}

namespace {
[[maybe_unused]] const bool kExpressionMapEditHostInstalled = [] {
    HirariUnifiedEngine::installExpressionMapEditHost();
    return true;
}();
}  // namespace
