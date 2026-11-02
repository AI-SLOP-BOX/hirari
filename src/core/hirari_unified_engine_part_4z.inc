// Region edit host bridge.
//
// The Rust region store owns the table, its undo capture and its snapshot
// publication; everything that used to reach into C++ region storage lives
// there now. What remains here is only the engine surface the store cannot
// own: the track registry, the audio decode/cache path and the pitch-analysis
// gate on the Track itself.

// Every host callback resolves ids through the same traversal while holding
// m_tracksMutex; keeping it here means the lookup cannot drift between files.
Track* HirariUnifiedEngine::findTrackLocked(uint32_t tid) {
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid) return track.get();
    }
    return nullptr;
}

std::shared_ptr<Track> HirariUnifiedEngine::findTrackSharedLocked(uint32_t tid) {
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid) return track;
    }
    return nullptr;
}

Track* HirariUnifiedEngine::findPublishedTrack(uint32_t tid) const {
    const auto& state = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < state.trackCount && i < kMaxTracks; ++i) {
        if (state.activeTracks[i] && state.activeTracks[i]->getId() == tid)
            return state.activeTracks[i];
    }
    return nullptr;
}

size_t HirariUnifiedEngine::regionHostListTracks(void* host, uint32_t* out,
                                                 size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    size_t count = 0;
    for (const auto& track : engine->m_tracks) {
        if (!track) continue;
        if (out && count < capacity) out[count] = track->getId();
        ++count;
    }
    return count;
}

void* HirariUnifiedEngine::regionHostTempoState(void* host) {
    return host ? static_cast<HirariUnifiedEngine*>(host)->m_tempoState : nullptr;
}

void* HirariUnifiedEngine::regionHostUndoState(void* host) {
    if (!host) return nullptr;
    return static_cast<HirariUnifiedEngine*>(host)->m_undoState;
}

double HirariUnifiedEngine::regionHostSampleRate(void* host) {
    if (!host) return 0.0;
    return static_cast<HirariUnifiedEngine*>(host)->m_sampleRate.load(std::memory_order_relaxed);
}

std::shared_ptr<AudioBuffer> HirariUnifiedEngine::importAudioFile(const std::string& path,
                                                                 double* sampleRateOut) {
    if (sampleRateOut) *sampleRateOut = 0.0;
    const auto* pathBytes = reinterpret_cast<const uint8_t*>(path.data());
    double cachedRate = 0.0;
    void* cachedHandle = nullptr;
    const int32_t outcome =
        hirari_audio_import_cache_acquire(pathBytes, path.size(), &cachedRate, &cachedHandle);
    if (outcome == 0) {
        std::shared_ptr<AudioBuffer> cached;
        if (auto* owned = static_cast<std::shared_ptr<AudioBuffer>*>(cachedHandle)) {
            cached = std::move(*owned);
            delete owned;
        }
        if (!cached || cached->getNumSamples() == 0 || cached->getNumChannels() == 0) {
            return nullptr;
        }
        if (sampleRateOut) *sampleRateOut = cachedRate;
        return cached;
    }
    // The cache only reports "needs decode" for a readable file; an empty or
    // missing path is a terminal rejection.
    if (outcome != 3) return nullptr;

    const auto decoded = hirari_import_audio_file(pathBytes, path.size());
    struct DecodedAudioGuard {
        float* samples;
        size_t count;
        ~DecodedAudioGuard() { hirari_free_decoded_audio(samples, count); }
    } decodedGuard{decoded.samples, decoded.sample_count};
    if (decoded.status != 0 || !decoded.samples || decoded.channels == 0) return nullptr;

    auto audio = std::make_shared<AudioBuffer>();
    try {
        const size_t frames = decoded.sample_count / decoded.channels;
        if (!audio->resize(decoded.channels, static_cast<uint32_t>(frames)) ||
            !hirari_audio_buffer_deinterleave_interleaved(
                decoded.samples, decoded.sample_count, audio->getArrayOfWritePointers(),
                decoded.channels, static_cast<uint32_t>(frames))) {
            return nullptr;
        }
    } catch (...) {
        return nullptr;
    }
    if (sampleRateOut) *sampleRateOut = decoded.sample_rate;
    // The cache takes ownership of the weak handle and releases it when the
    // entry is replaced, evicted, or refused.
    hirari_audio_import_cache_store(pathBytes, path.size(), decoded.sample_rate,
                                    new std::weak_ptr<AudioBuffer>(audio));
    return audio;
}

bool HirariUnifiedEngine::regionHostDecodeAudio(void* host, const uint8_t* path,
                                                size_t length, void** outHandle,
                                                double* outRate) {
    if (!host || (!path && length != 0) || !outHandle) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    double rate = 0.0;
    auto audio = engine->importAudioFile(
        std::string(reinterpret_cast<const char*>(path), length), &rate);
    if (!audio || audio->getNumSamples() == 0 || audio->getNumChannels() == 0) {
        return false;
    }
    *outHandle = new std::shared_ptr<AudioBuffer>(std::move(audio));
    if (outRate) {
        *outRate = std::isfinite(rate) && rate > 0.0 ? rate : 0.0;
    }
    return true;
}

size_t HirariUnifiedEngine::regionHostTrackIdentity(void* host, uint32_t trackId) {
    return withHostTrack(host, trackId, size_t{0},
                         [](Track& track) { return reinterpret_cast<size_t>(&track); });
}

void* HirariUnifiedEngine::regionHostRetainTrack(void* host, uint32_t trackId) {
    if (!host) return nullptr;
    auto track = static_cast<HirariUnifiedEngine*>(host)->takeTrackHandle(trackId);
    return track ? new std::shared_ptr<Track>(std::move(track)) : nullptr;
}

void HirariUnifiedEngine::regionHostReleaseTrack(void* handle) {
    delete static_cast<std::shared_ptr<Track>*>(handle);
}

bool HirariUnifiedEngine::regionHostPitchBegin(void* trackHandle) {
    if (!trackHandle) return false;
    const auto track = *static_cast<const std::shared_ptr<Track>*>(trackHandle);
    return track && track->beginAudioPitchAnalysis();
}

void HirariUnifiedEngine::regionHostPitchFinish(void* trackHandle) {
    if (!trackHandle) return;
    const auto track = *static_cast<const std::shared_ptr<Track>*>(trackHandle);
    if (track) track->finishAudioPitchAnalysis();
}

void* HirariUnifiedEngine::regionHostNoteSnapshot(void* trackHandle, uint32_t regionId) {
    if (!trackHandle) return nullptr;
    const auto track = *static_cast<const std::shared_ptr<Track>*>(trackHandle);
    return track ? hirari_region_store_note_snapshot(track.get(), regionId) : nullptr;
}

namespace {

const HirariRegionHostOps kRegionEditHostOps = {
    nullptr,  // The Rust region store answers the region-data callbacks.
    nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
    &Hirari::Core::Engine::Track::regionHostAudioView,
    &Hirari::Core::Engine::Track::regionHostReleaseHandle,
    nullptr,
    &HirariUnifiedEngine::regionHostListTracks,
    nullptr,
    &HirariUnifiedEngine::regionHostTempoState,
    &HirariUnifiedEngine::regionHostUndoState,
    &HirariUnifiedEngine::regionHostSampleRate,
    nullptr,
    &HirariUnifiedEngine::regionHostDecodeAudio,
    nullptr,
    &HirariUnifiedEngine::regionHostTrackIdentity,
    &HirariUnifiedEngine::regionHostRetainTrack,
    &HirariUnifiedEngine::regionHostReleaseTrack,
    &HirariUnifiedEngine::regionHostPitchBegin,
    &HirariUnifiedEngine::regionHostPitchFinish,
    &HirariUnifiedEngine::regionHostNoteSnapshot,
    nullptr, nullptr,
};

}  // namespace
