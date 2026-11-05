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
            if (engine->handle(kHirariHandleMasterLimiter)) hirari_master_limiter_reset(engine->handle(kHirariHandleMasterLimiter));
            std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
            for (void* track : engine->lockedTracks()) {
                if (track) hirari_track_reset_for_offline_render(track);
            }
        },
        // The plan walk is Rust's (bounce_driver.rs): it pins the tracks, reads
        // the region spans, the MIDI snapshot, the device rate and the limiter
        // tail from the stores it already owns. C++ only names them.
        [](void* host, uint32_t kind) -> void* {
            if (!host) return nullptr;
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            switch (kind) {
                case kHirariBounceStoreTrackTable: return engine->handle(kHirariHandleTrackTable);
                case kHirariBounceStoreEngineBlock: return engine->handle(kHirariHandleEngineBlock);
                case kHirariBounceStoreMidiSnapshot: return engine->handle(kHirariHandleMidiSnapshotStore);
                default: return engine->handle(kHirariHandleMasterLimiter);
            }
        },
        // The driver's channel pairs are wrapped, not copied, so the offline
        // graph renders straight into the caller's block buffers.
        [](void* host, float* left, float* right, uint32_t frames) {
            float* channels[2] = {left, right};
            void* view = hirari_audio_buffer_storage_create();
            (void)hirari_audio_buffer_storage_wrap_channels(view, channels, 2, frames);
            static_cast<HirariUnifiedEngine*>(host)->processBlock(view, 0, frames);
            hirari_audio_buffer_storage_destroy(view);
        },
        // Tracks size their buffers to the engine block size and silently
        // skip larger blocks; never render past the prepared block.
        [](void* host) -> uint32_t {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            const uint32_t block = hirari_engine_block_device_block_frames(engine->handle(kHirariHandleEngineBlock));
            return block > 0 ? std::min(1024u, block) : 1024u;
        },
        [](void* host, uint64_t* playhead, uint8_t* playing) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            *playhead = hirari_engine_block_playhead(engine->handle(kHirariHandleEngineBlock));
            *playing = hirari_engine_block_is_playing(engine->handle(kHirariHandleEngineBlock));
        },
        [](void* host, uint64_t playhead, uint8_t playing) {
            auto* engine = static_cast<HirariUnifiedEngine*>(host);
            hirari_engine_block_set_playhead(engine->handle(kHirariHandleEngineBlock), playhead);
            hirari_engine_block_set_playing(engine->handle(kHirariHandleEngineBlock), playing);
        },
    };
    return &ops;
}

bool HirariUnifiedEngine::bounce_project(std::string_view path, uint32_t format) {
    // Project loads and deferred structural edits publish tracks through the
    // control-side container first. Rebuild the immutable callback snapshot so
    // a bounce can never observe an empty/stale track graph (silent WAV).
    syncStructuralChanges();
    return hirari_bounce_render_driver(this, handle(kHirariHandleBounceSession), bounceOps(),
                                       reinterpret_cast<const uint8_t*>(path.data()), path.size(),
                                       format);
}
