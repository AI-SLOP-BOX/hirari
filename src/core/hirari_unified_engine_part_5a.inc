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

}  // namespace

rust::Vec<float> HirariUnifiedEngine::get_region_waveform(uint32_t tid, uint32_t rid,
                                                          uint32_t numPeaks) {
    return takeFloatBuffer(hirari_region_waveform(this, tid, rid, numPeaks));
}

rust::Vec<float> HirariUnifiedEngine::get_region_audio_interleaved(uint32_t tid, uint32_t rid,
                                                                   uint32_t maxFrames) {
    return takeFloatBuffer(hirari_region_audio_interleaved(this, tid, rid, maxFrames));
}

rust::Vec<float> HirariUnifiedEngine::get_region_audio_interleaved_range(
    uint32_t tid, uint32_t rid, uint64_t startFrame, uint32_t frames, uint32_t channels) {
    return takeFloatBuffer(
        hirari_region_audio_interleaved_range(this, tid, rid, startFrame, frames, channels));
}




bool HirariUnifiedEngine::bounce_project(std::string_view path, uint32_t format) {
    // Formats 0/3 are PCM16/PCM24 RIFF/RF64, 1 is Float32 WAVE64, and 2 is
    // Float32 RIFF/RF64. Container acceptance is Rust-owned so an unsupported
    // value can never silently produce a different container.
    if (!hirari_bounce_output_path_supported(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), format)) {
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        ::Hirari::Core::Diagnostics::LogBuffer::post(1, 0, "BOUNCE_UNSUPPORTED_FORMAT_OR_PATH");
        return false;
    }

    // Project loads and deferred structural edits publish tracks through the
    // control-side container first.  Rebuild the immutable callback snapshot
    // before an offline render so a bounce can never observe an empty/stale
    // track graph (which otherwise produces a valid but silent WAV).
    syncStructuralChanges();

    // Async callers clear this before publishing Queued so a cancellation
    // requested while queued is preserved. Direct synchronous callers need a
    // fresh request flag for each invocation.
    if (!m_bounceRunning.load(std::memory_order_acquire)) {
        m_bounceCancelRequested.store(false, std::memory_order_release);
        m_bouncePauseRequested.store(false, std::memory_order_release);
    }

    m_bounceProgress.store(0.0f, std::memory_order_release);
    m_bounceState.store(static_cast<uint32_t>(BounceState::Rendering), std::memory_order_release);

    const uint64_t owner = hirari_callback_thread_token();
    const bool previousOfflineRenderThread = hirari_offline_render_thread;
    hirari_offline_render_thread = true;
    struct OfflineRenderThreadGuard final {
        bool previous;
        ~OfflineRenderThreadGuard() { hirari_offline_render_thread = previous; }
    } offlineRenderThreadGuard{previousOfflineRenderThread};
    bool expected = false;
    const auto offlineAcquireDeadline = std::chrono::steady_clock::now() +
        std::chrono::seconds(5);
    while (!m_offlineRenderActive.compare_exchange_weak(
        expected, true, std::memory_order_acq_rel, std::memory_order_acquire)) {
        expected = false;
        if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
            m_bounceState.store(static_cast<uint32_t>(BounceState::Cancelled), std::memory_order_release);
            return false;
        }
        // Never leave the render worker spinning forever behind a stale
        // callback/offline-render owner. A bounded failure is recoverable;
        // an immortal QUEUED render is not.
        if (std::chrono::steady_clock::now() >= offlineAcquireDeadline) {
            m_bounceProgress.store(0.0f, std::memory_order_release);
            m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
            ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "BOUNCE_OFFLINE_RENDER_BUSY_TIMEOUT");
            return false;
        }
        std::this_thread::yield();
    }
    m_offlineRenderOwner.store(owner, std::memory_order_release);

    const auto finishOfflineRender = [this]() {
        m_offlineRenderOwner.store(0, std::memory_order_release);
        m_offlineRenderActive.store(false, std::memory_order_release);
    };
    // Every path after acquiring the offline-render ownership must release it,
    // including allocation/IO failures and future early returns.  Keep this
    // guard deliberately tiny: it only publishes the ownership transition and
    // does not touch render state or the audio graph.
    struct OfflineRenderGuard final {
        std::atomic<uint64_t>* owner;
        std::atomic<bool>* active;
        ~OfflineRenderGuard() {
            owner->store(0, std::memory_order_release);
            active->store(false, std::memory_order_release);
        }
    } offlineRenderGuard{&m_offlineRenderOwner, &m_offlineRenderActive};

    // Pause is an offline-render control-plane operation.  It is deliberately
    // checked between blocks so the audio callback is never blocked and the
    // last completed block remains a valid, deterministic checkpoint.
    const auto waitForBounceControl = [this]() -> bool {
        for (;;) {
            if (m_bounceCancelRequested.load(std::memory_order_acquire)) {
                return false;
            }
            if (!m_bouncePauseRequested.load(std::memory_order_acquire)) {
                m_bounceState.store(static_cast<uint32_t>(BounceState::Rendering),
                                    std::memory_order_release);
                return true;
            }
            m_bounceState.store(static_cast<uint32_t>(BounceState::Paused),
                                std::memory_order_release);
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
    };

    // The render extent is Rust-owned: arrangement spans, MIDI note ends, the
    // empty-project fallback, the selected range, the effect tail and the
    // master limiter's look-ahead all feed one plan. This function only
    // gathers the live spans and maps the plan's failure reason.
    std::vector<HirariBounceRegionSpan> regionSpans;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track) continue;
            for (const auto& region : track->getRegions()) {
                regionSpans.push_back({region.start, region.len, region.loopCount});
            }
        }
    }
    // MIDI-only projects do not have audio regions, but their scheduled notes
    // still define the render extent. Snapshot the active note list under the
    // control-side mutex so a note publication cannot race this calculation.
    std::vector<uint64_t> midiNoteEnds;
    const auto notes = midi_notes_snapshot();
    midiNoteEnds.reserve(notes.size());
    for (const auto& note : notes) {
        midiNoteEnds.push_back(hirari_midi_scheduled_note_end(
            reinterpret_cast<const HirariScheduledMidiNote*>(&note)));
    }

    HirariBouncePlanInput planInput{};
    planInput.regions = regionSpans.data();
    planInput.region_count = regionSpans.size();
    planInput.midi_note_ends = midiNoteEnds.data();
    planInput.midi_note_count = midiNoteEnds.size();
    planInput.sample_rate = m_sampleRate.load(std::memory_order_acquire);
    planInput.range_active =
        m_offlineRenderRangeActive.load(std::memory_order_acquire) ? 1 : 0;
    planInput.range_start = m_offlineRenderStartSample.load(std::memory_order_acquire);
    planInput.range_end = m_offlineRenderEndSample.load(std::memory_order_acquire);
    planInput.tail_millis = m_offlineRenderTailMillis.load(std::memory_order_acquire);
    planInput.limiter_tail = m_masterLimiter ? m_masterLimiter->getTailSamples() : 0;

    HirariBouncePlan plan{};
    const int32_t planStatus = hirari_bounce_render_extent(&planInput, &plan);
    if (planStatus != 0) {
        // Every rejected plan must publish a terminal state. Leaving the
        // render at Rendering here made a bad range look like an immortal
        // render to the UI and to retry logic.
        m_bounceProgress.store(0.0f, std::memory_order_release);
        m_bounceState.store(static_cast<uint32_t>(BounceState::Failed), std::memory_order_release);
        ::Hirari::Core::Diagnostics::LogBuffer::post(
            1, 0, planStatus == -2 ? "BOUNCE_INVALID_RANGE" : "BOUNCE_INVALID_SAMPLE_RATE");
        return false;
    }
    const uint32_t outputRate = plan.output_rate;
    const uint32_t totalSamples = static_cast<uint32_t>(plan.total_samples);
    // Every supported output format streams completed blocks directly to the
    // Rust-owned writer. Long exports therefore do not allocate full-length
    // float buffers before the first byte reaches disk.
    struct BounceStreamGuard final {
        void* handle = nullptr;
        ~BounceStreamGuard() { hirari_bounce_stream_free(handle); }
    } streamOut;
