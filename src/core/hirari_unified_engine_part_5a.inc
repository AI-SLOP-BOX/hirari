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

// The Rust driver owns the bounce session; this table is only the part of a
// render that needs the native graph. It has static storage because the async
// worker calls through it after the starting frame has returned.
const HirariBounceHostOps* HirariUnifiedEngine::bounceOps() {
    static const HirariBounceHostOps ops{
        [](void* host, const uint8_t* path, size_t length, uint32_t format) -> uint8_t {
            const std::string_view output(reinterpret_cast<const char*>(path), length);
            return static_cast<HirariUnifiedEngine*>(host)->bounce_project(output, format) ? 1 : 0;
        },
        // Clear stateful processor history so a bounce is reproducible.
        [](void* host) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            if (engine->m_masterLimiter) hirari_master_limiter_reset(engine->m_masterLimiter);
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (Track* track : engine->lockedTracks()) {
                if (track) track->resetForOfflineRender();
            }
        },
        [](void* host, HirariBouncePlan* out) -> int32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            std::vector<HirariBounceRegionSpan> regionSpans;
            {
                std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
                for (Track* track : engine->lockedTracks()) {
                    if (!track) continue;
                    const size_t regionCount = hirari_region_store_count(track);
                    for (size_t index = 0; index < regionCount; ++index) {
                        uint64_t start = 0, length = 0;
                        uint32_t loopCount = 0;
                        if (hirari_region_store_span_at(track, index, &start,
                                                        &length, &loopCount))
                            regionSpans.push_back({start, length, loopCount});
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
            input.tail_millis = hirari_bounce_tail_millis(engine->m_bounceSession);
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
    };
    return &ops;
}

bool HirariUnifiedEngine::bounce_project(std::string_view path, uint32_t format) {
    // Project loads and deferred structural edits publish tracks through the
    // control-side container first. Rebuild the immutable callback snapshot so
    // a bounce can never observe an empty/stale track graph (silent WAV).
    syncStructuralChanges();
    return hirari_bounce_render_driver(this, m_bounceSession, bounceOps(),
                                       reinterpret_cast<const uint8_t*>(path.data()), path.size(),
                                       format);
}
