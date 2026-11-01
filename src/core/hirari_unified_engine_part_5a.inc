namespace {

// Rust returns an owned float buffer so the native wrapper can copy it into
// the CXX vector without keeping a second decoder path alive here.
rust::Vec<float> takeFloatBuffer(void* buffer) {
    rust::Vec<float> samples;
    if (!buffer) return samples;
    const float* data = hirari_float_buffer_data(buffer);
    const size_t count = hirari_float_buffer_len(buffer);
    if (data && count != 0) {
        samples.reserve(count);
        for (size_t index = 0; index < count; ++index) samples.push_back(data[index]);
    }
    hirari_float_buffer_destroy(buffer);
    return samples;
}

// Offline-render thread flag carried from the driver's begin to its finish.
thread_local bool g_bounceOfflineRenderPrev = false;

}  // namespace

rust::Vec<float> HirariUnifiedEngine::get_region_waveform(uint32_t tid, uint32_t rid,
                                                          uint32_t numPeaks) {
    return takeFloatBuffer(hirari_region_waveform(this, tid, rid, numPeaks));
}

uint64_t HirariUnifiedEngine::get_video_frame_revision() const noexcept {
#if defined(__APPLE__)
    return ::Hirari::Core::Engine::VideoBridge::frameRevision();
#else
    return 0;
#endif
}

bool HirariUnifiedEngine::request_video_frame(double seconds) {
    if (!std::isfinite(seconds) || seconds < 0.0) return false;
#if defined(__APPLE__)
    return ::Hirari::Core::Engine::VideoBridge::requestFrameAt(seconds);
#else
    (void)seconds;
    return false;
#endif
}

bool HirariUnifiedEngine::load_video(std::string_view path) {
    if (path.empty()) return false;
#if defined(__APPLE__)
    std::string owned(path);
    return ::Hirari::Core::Engine::VideoBridge::loadVideo(owned.c_str());
#else
    (void)path;
    return false;
#endif
}




bool HirariUnifiedEngine::bounce_project(std::string_view path, uint32_t format) {
    // Project loads and deferred structural edits publish tracks through the
    // control-side container first. Rebuild the immutable callback snapshot so
    // a bounce can never observe an empty/stale track graph (silent WAV).
    syncStructuralChanges();

    // The Rust driver owns the render orchestration (extent, chunking,
    // pause/cancel, deterministic dither, progress/state publication and the
    // publish boundary). These operations are the engine surface it drives.
    static const HirariBounceHostOps ops{
        // Acquires offline-render ownership. 0 = acquired, 1 = cancelled,
        // 2 = busy timeout.
        [](void* host) -> uint8_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            // Async callers clear this before publishing Queued so a
            // cancellation requested while queued is preserved; direct
            // synchronous callers get a fresh flag per invocation.
            if (!engine->m_bounceRunning.load(std::memory_order_acquire)) {
                engine->m_bounceCancelRequested.store(false, std::memory_order_release);
                engine->m_bouncePauseRequested.store(false, std::memory_order_release);
            }
            engine->m_bounceProgress.store(0.0f, std::memory_order_release);
            engine->m_bounceState.store(
                static_cast<uint32_t>(BounceState::Rendering), std::memory_order_release);
            g_bounceOfflineRenderPrev = hirari_offline_render_thread;
            hirari_offline_render_thread = true;
            const uint64_t owner = hirari_callback_thread_token();
            bool expected = false;
            const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
            while (!engine->m_offlineRenderActive.compare_exchange_weak(
                expected, true, std::memory_order_acq_rel, std::memory_order_acquire)) {
                expected = false;
                if (engine->m_bounceCancelRequested.load(std::memory_order_acquire)) {
                    hirari_offline_render_thread = g_bounceOfflineRenderPrev;
                    engine->m_bounceState.store(
                        static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
                    return 1;
                }
                // A bounded failure is recoverable; an immortal render is not.
                if (std::chrono::steady_clock::now() >= deadline) {
                    hirari_offline_render_thread = g_bounceOfflineRenderPrev;
                    engine->m_bounceProgress.store(0.0f, std::memory_order_release);
                    engine->m_bounceState.store(
                        static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
                    hirari_log_post(
                        0, 0, "BOUNCE_OFFLINE_RENDER_BUSY_TIMEOUT");
                    return 2;
                }
                std::this_thread::yield();
            }
            engine->m_offlineRenderOwner.store(owner, std::memory_order_release);
            return 0;
        },
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_offlineRenderOwner.store(0, std::memory_order_release);
            engine->m_offlineRenderActive.store(false, std::memory_order_release);
            hirari_offline_render_thread = g_bounceOfflineRenderPrev;
        },
        // Clear stateful processor history so a bounce is reproducible.
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (engine->m_masterLimiter) hirari_master_limiter_reset(engine->m_masterLimiter);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (const auto& track : engine->m_tracks) {
                if (track) track->resetForOfflineRender();
            }
        },
        [](void* host, HirariBouncePlan* out) -> int32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::vector<HirariBounceRegionSpan> regionSpans;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (const auto& track : engine->m_tracks) {
                    if (!track) continue;
                    for (const auto& region : track->getRegions()) {
                        regionSpans.push_back({region.start, region.len, region.loopCount});
                    }
                }
            }
            // MIDI-only projects define their extent through scheduled notes.
            std::vector<uint64_t> midiNoteEnds;
            size_t midiNoteCount = 0;
            auto* midiNotes = hirari_midi_notes_snapshot_take(
                reinterpret_cast<const void*>(engine->midi_snapshot_store()), &midiNoteCount);
            midiNoteEnds.reserve(midiNoteCount);
            for (size_t i = 0; i < midiNoteCount; ++i)
                midiNoteEnds.push_back(hirari_midi_scheduled_note_end(midiNotes + i));
            hirari_midi_notes_snapshot_take_free(midiNotes, midiNoteCount);
            HirariBouncePlanInput input{};
            input.regions = regionSpans.data();
            input.region_count = regionSpans.size();
            input.midi_note_ends = midiNoteEnds.data();
            input.midi_note_count = midiNoteEnds.size();
            input.sample_rate = engine->m_sampleRate.load(std::memory_order_acquire);
            input.range_active =
                engine->m_offlineRenderRangeActive.load(std::memory_order_acquire) ? 1 : 0;
            input.range_start = engine->m_offlineRenderStartSample.load(std::memory_order_acquire);
            input.range_end = engine->m_offlineRenderEndSample.load(std::memory_order_acquire);
            input.tail_millis = engine->m_offlineRenderTailMillis.load(std::memory_order_acquire);
            input.limiter_tail = engine->m_masterLimiter
                ? hirari_master_limiter_tail(engine->m_masterLimiter) : 0;
            const int32_t status = hirari_bounce_render_extent(&input, out);
            if (status != 0) {
                // A rejected plan must publish a terminal state, not look
                // like an immortal render.
                hirari_log_post(
                    1, 0, status == -2 ? "BOUNCE_INVALID_RANGE" : "BOUNCE_INVALID_SAMPLE_RATE");
            }
            return status;
        },
        // The driver's channel pairs are wrapped, not copied, so the offline
        // graph renders straight into the caller's block buffers.
        [](void* host, float* left, float* right, uint32_t frames) {
            float* channels[2] = {left, right};
            ::Hirari::Core::AudioBuffer view;
            view.wrapChannels(channels, 2, frames);
            static_cast<HirariUnifiedEngine*>(host)->processBlock(view, 0, frames);
        },
        // Tracks size their buffers to the engine block size and silently
        // skip larger blocks; never render past the prepared block.
        [](void* host) -> uint32_t {
            const uint32_t block = static_cast<HirariUnifiedEngine*>(host)->get_block_size();
            return block > 0 ? std::min(1024u, block) : 1024u;
        },
        [](void* host, float progress, uint8_t state) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_bounceProgress.store(progress, std::memory_order_release);
            engine->m_bounceState.store(state, std::memory_order_release);
        },
        // Pause/cancel are control-plane requests checked between blocks.
        [](void* host) -> uint8_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            uint8_t bits = 0;
            if (engine->m_bounceCancelRequested.load(std::memory_order_acquire)) bits |= 1;
            if (engine->m_bouncePauseRequested.load(std::memory_order_acquire)) bits |= 2;
            return bits;
        },
        [](void* host, uint64_t* playhead, uint8_t* playing) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            *playhead = engine->m_playhead.load(std::memory_order_acquire);
            *playing = engine->m_isPlaying.load(std::memory_order_acquire) ? 1 : 0;
        },
        [](void* host, uint64_t playhead, uint8_t playing) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            engine->m_playhead.store(playhead, std::memory_order_release);
            engine->m_isPlaying.store(playing != 0, std::memory_order_release);
        },
        // A streaming writer only publishes a complete, synced file. The
        // cancel check and the publish share one lock so a cancel either
        // precedes the publish or loses to it.
        [](void* host, void* stream) -> uint8_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::lock_guard<std::mutex> publishLock(engine->m_bouncePublishMutex);
            if (engine->m_bounceCancelRequested.load(std::memory_order_acquire)) return 0;
            return hirari_bounce_stream_finish(stream) != 0 ? 1 : 2;
        },
        [](void*, uint32_t level, const uint8_t* message, size_t length) {
            hirari_log_post(
                level, 0, std::string_view(reinterpret_cast<const char*>(message), length));
        },
    };
    return hirari_bounce_render_driver(
        this, &ops, reinterpret_cast<const uint8_t*>(path.data()), path.size(), format);
}
