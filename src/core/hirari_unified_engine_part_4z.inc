// Region edit host bridge.
//
// Rust owns every region edit decision and the undo capture; this file is the
// only place where that policy reaches the engine's region storage. Each
// callback runs on the control thread, under the same locks the previous
// per-method C++ code took.

namespace {

void fillRegionHostState(const Region& region, HirariRegionState& out) {
    out = region.editableState();
}

}  // namespace

// Every host callback resolves ids through the same traversal while holding
// m_tracksMutex; keeping it here means the lookup cannot drift between files.
Track* HirariUnifiedEngine::findTrackLocked(uint32_t tid) {
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid) return track.get();
    }
    return nullptr;
}

const Track* HirariUnifiedEngine::findTrackLocked(uint32_t tid) const {
    for (const auto& track : m_tracks) {
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

std::shared_ptr<Track> HirariUnifiedEngine::findTrackSharedLocked(uint32_t tid) const {
    for (const auto& track : m_tracks) {
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

const Region* HirariUnifiedEngine::findRegionLocked(uint32_t tid, uint32_t rid) const {
    for (const auto& track : m_tracks) {
        if (!track || track->getId() != tid) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id == rid) return &region;
        }
        return nullptr;
    }
    return nullptr;
}

bool HirariUnifiedEngine::regionHostFind(void* host, uint32_t trackId, uint32_t regionId,
                                         HirariRegionState* out) {
    if (!host || !out) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    if (!region) return false;
    fillRegionHostState(*region, *out);
    return true;
}

bool HirariUnifiedEngine::regionHostApply(void* host,
                                          const HirariRegionEditPayload* payload) {
    if (!host || !payload) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (auto& track : engine->m_tracks) {
        if (!track || track->getId() != payload->trackId) continue;
        bool applied = false;
        switch (payload->op) {
        case kHirariRegionEditState:
            applied = track->applyRegionState(payload->regionId, payload->state);
            break;
        case kHirariRegionEditRemove:
            for (const auto& region : track->getRegions()) {
                if (region.id == payload->regionId) { applied = true; break; }
            }
            if (applied) track->removeRegion(payload->regionId);
            break;
        case kHirariRegionEditRestore: {
            const auto* snapshot = reinterpret_cast<const Region*>(payload->blob);
            applied = snapshot != nullptr;
            if (applied) {
                track->removeRegion(payload->regionId);
                track->addRegion(*snapshot);
            }
            break;
        }
        case kHirariRegionEditNoteStore:
            applied = track->editRegionNoteStore(
                payload->regionId, const_cast<uint8_t*>(payload->blob),
                &hirari_region_note_store_restore);
            break;
        case kHirariRegionEditGeometry: {
            if (payload->blobLen % sizeof(HirariWarpMarker) != 0) break;
            const auto markerCount = payload->blobLen / sizeof(HirariWarpMarker);
            if (markerCount != 0 && payload->blob == nullptr) break;
            applied = track->applyRegionGeometry(
                payload->regionId, payload->state,
                reinterpret_cast<const HirariWarpMarker*>(payload->blob), markerCount);
            break;
        }
        case kHirariRegionEditMarkers: {
            if (payload->blobLen % sizeof(HirariWarpMarker) != 0) break;
            const auto markerCount = payload->blobLen / sizeof(HirariWarpMarker);
            if (markerCount != 0 && payload->blob == nullptr) break;
            const auto* markers =
                reinterpret_cast<const HirariWarpMarker*>(payload->blob);
            std::vector<HirariWarpMarker> replacement;
            if (markerCount != 0) replacement.assign(markers, markers + markerCount);
            applied = track->setRegionWarpMarkers(payload->regionId, std::move(replacement));
            break;
        }
        case kHirariRegionEditRangeEdits: {
            if (payload->blobLen % sizeof(HirariRegionRangeEdit) != 0) break;
            const auto count = payload->blobLen / sizeof(HirariRegionRangeEdit);
            if (count != 0 && payload->blob == nullptr) break;
            const auto* edits = reinterpret_cast<const HirariRegionRangeEdit*>(payload->blob);
            std::vector<Region::RangeEdit> replacement;
            replacement.reserve(count);
            for (size_t index = 0; index < count; ++index) {
                replacement.push_back({edits[index].start, edits[index].end,
                                       edits[index].gain, edits[index].fade_in,
                                       edits[index].fade_out});
            }
            applied = track->replaceRegionRangeEdits(payload->regionId,
                                                     std::move(replacement), false);
            break;
        }
        case kHirariRegionEditCompRanges: {
            if (payload->blobLen % sizeof(HirariRegionCompRange) != 0) break;
            const auto count = payload->blobLen / sizeof(HirariRegionCompRange);
            if (count != 0 && payload->blob == nullptr) break;
            const auto* source =
                reinterpret_cast<const HirariRegionCompRange*>(payload->blob);
            std::vector<Region::CompRange> replacement;
            replacement.reserve(count);
            for (size_t index = 0; index < count; ++index) {
                replacement.push_back({source[index].start, source[index].end,
                                       source[index].fade_in, source[index].fade_out});
            }
            applied = track->replaceRegionCompRanges(payload->regionId,
                                                     std::move(replacement),
                                                     payload->managed);
            break;
        }
        case kHirariRegionEditPitchCommit: {
            // The job owns the analysis gate, so the commit bypasses the
            // editor-facing gate check but keeps the single region lock.
            auto* analysis = reinterpret_cast<void*>(const_cast<uint8_t*>(payload->blob));
            applied = analysis != nullptr &&
                      track->commitAudioPitchAnalysis(payload->regionId,
                                                      payload->generation, analysis);
            if (analysis) hirari_audio_pitch_result_destroy(analysis);
            break;
        }
        case kHirariRegionEditReplaceAudio: {
            // Header: audio handle, source rate, timeline rate, then the path.
            static_assert(sizeof(void*) == sizeof(uint64_t));
            constexpr size_t kHeaderBytes = sizeof(uint64_t) + 2 * sizeof(double);
            if (payload->blobLen < kHeaderBytes || payload->blob == nullptr) break;
            uint64_t rawHandle = 0;
            double sourceRate = 0.0;
            double timelineRate = 0.0;
            std::memcpy(&rawHandle, payload->blob, sizeof(rawHandle));
            std::memcpy(&sourceRate, payload->blob + sizeof(rawHandle), sizeof(double));
            std::memcpy(&timelineRate, payload->blob + sizeof(rawHandle) + sizeof(double),
                        sizeof(double));
            auto* owned = reinterpret_cast<std::shared_ptr<AudioBuffer>*>(
                static_cast<uintptr_t>(rawHandle));
            auto audio = owned ? std::move(*owned) : nullptr;
            delete owned;
            const std::string path(
                reinterpret_cast<const char*>(payload->blob + kHeaderBytes),
                payload->blobLen - kHeaderBytes);
            applied = track->replaceRegionAudio(payload->regionId, std::move(path),
                                                std::move(audio), sourceRate, timelineRate);
            break;
        }
        default:
            applied = track->applyRegionScalarFields(payload->regionId, payload->state);
            break;
        }
        if (applied && payload->op != kHirariRegionEditCompRanges) {
            // The comp-range setter published the snapshot without dropping
            // the cached peaks; keep that path unchanged.
            hirari_region_waveform_forget(engine, payload->regionId);
        }
        return applied;
    }
    return false;
}

void* HirariUnifiedEngine::regionHostSnapshot(void* host, uint32_t trackId,
                                              uint32_t regionId) {
    if (!host) return nullptr;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    return region ? new Region(*region) : nullptr;
}

void HirariUnifiedEngine::regionHostReleaseSnapshot(void*, void* handle) {
    delete static_cast<Region*>(handle);
}

size_t HirariUnifiedEngine::regionHostWarpMarkers(void* host, uint32_t trackId,
                                                  uint32_t regionId, HirariWarpMarker* out,
                                                  size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    if (!region) return 0;
    if (out && capacity >= region->warpMarkers.size()) {
        std::copy(region->warpMarkers.begin(), region->warpMarkers.end(), out);
    }
    return region->warpMarkers.size();
}

size_t HirariUnifiedEngine::regionHostRangeEdits(void* host, uint32_t trackId,
                                                 uint32_t regionId,
                                                 HirariRegionRangeEdit* out,
                                                 size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    if (!region) return 0;
    if (out && capacity >= region->rangeEdits.size()) {
        for (size_t index = 0; index < region->rangeEdits.size(); ++index) {
            const auto& edit = region->rangeEdits[index];
            out[index] = {edit.start, edit.end, edit.gain, edit.fadeIn, edit.fadeOut};
        }
    }
    return region->rangeEdits.size();
}

void* HirariUnifiedEngine::regionHostRetainAudio(void* host, uint32_t trackId,
                                                 uint32_t regionId) {
    if (!host) return nullptr;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    if (!region || !region->audio) return nullptr;
    // The shared_ptr keeps the decoded buffer alive for the job even after the
    // region itself moves on; the worker only reads it.
    return new std::shared_ptr<AudioBuffer>(region->audio);
}

uint32_t HirariUnifiedEngine::regionHostAudioView(void* handle, const float** out,
                                                  size_t capacity, uint64_t* samples) {
    if (!handle) return 0;
    const auto audio = *static_cast<const std::shared_ptr<AudioBuffer>*>(handle);
    if (!audio) return 0;
    const uint32_t channels = audio->getNumChannels();
    if (samples) *samples = audio->getNumSamples();
    if (out && capacity >= channels) {
        for (uint32_t channel = 0; channel < channels; ++channel) {
            out[channel] = audio->getReadPointer(channel);
        }
    }
    return channels;
}

void HirariUnifiedEngine::regionHostReleaseAudio(void* handle) {
    delete static_cast<std::shared_ptr<AudioBuffer>*>(handle);
}

bool HirariUnifiedEngine::regionHostTrackToken(void* host, uint32_t trackId,
                                               uint64_t* generation, size_t* snapshot) {
    if (!host) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Track* track = engine->findTrackLocked(trackId);
    if (!track) return false;
    if (generation) *generation = track->regionGeneration();
    if (snapshot) *snapshot = reinterpret_cast<size_t>(track->getRegionSnapshot());
    return true;
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

size_t HirariUnifiedEngine::regionHostListRegions(void* host, uint32_t trackId,
                                                  uint32_t* out, size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Track* track = engine->findTrackLocked(trackId);
    if (!track) return 0;
    size_t count = 0;
    for (const auto& region : track->getRegions()) {
        if (out && count < capacity) out[count] = region.id;
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

size_t HirariUnifiedEngine::regionHostRegionPath(void* host, uint32_t trackId,
                                                 uint32_t regionId, uint8_t* out,
                                                 size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    if (!region) return 0;
    if (out && capacity >= region->path.size()) {
        std::copy(region->path.begin(), region->path.end(), out);
    }
    return region->path.size();
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

uint32_t HirariUnifiedEngine::regionHostInsertRegion(
    void* host, uint32_t trackId, size_t expectedTrack, const uint8_t* path,
    size_t pathLength, uint64_t startSample, double sourceSampleRate,
    double timelineSampleRate, uint64_t length, void* audioHandle) {
    if (!host || !audioHandle || (!path && pathLength != 0)) return 0;
    // The handle is consumed on every path, success or failure. The caller has
    // already validated the decode and derived the timeline length.
    auto* ownedAudio = static_cast<std::shared_ptr<AudioBuffer>*>(audioHandle);
    auto audio = std::move(*ownedAudio);
    delete ownedAudio;
    if (!audio) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    try {
        std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
        const auto current = std::find_if(engine->m_tracks.begin(), engine->m_tracks.end(),
            [trackId, expectedTrack](const auto& track) {
                return track && track->getId() == trackId &&
                    reinterpret_cast<size_t>(track.get()) == expectedTrack;
            });
        if (current == engine->m_tracks.end()) return 0;
        Region region{};
        region.id = engine->allocateRegionIdLocked();
        if (region.id == 0) return 0;
        region.path.assign(reinterpret_cast<const char*>(path), pathLength);
        region.start = startSample;
        region.sourceSampleRate = sourceSampleRate;
        region.timelineSampleRate = timelineSampleRate;
        region.len = length;
        region.muted = false;
        region.name = region.path;
        region.audio = std::move(audio);
        (*current)->addRegion(region);
        return region.id;
    } catch (...) {
        return 0;
    }
}

size_t HirariUnifiedEngine::regionHostTrackIdentity(void* host, uint32_t trackId) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    return reinterpret_cast<size_t>(engine->findTrackLocked(trackId));
}

void* HirariUnifiedEngine::regionHostRetainTrack(void* host, uint32_t trackId) {
    if (!host) return nullptr;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    auto track = engine->findTrackSharedLocked(trackId);
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
    if (!track) return nullptr;
    return track->cloneRegionNoteStore(regionId);
}

uint32_t HirariUnifiedEngine::regionHostClone(void* host, uint32_t trackId,
                                              uint32_t regionId,
                                              const HirariRegionState* state) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    Track* track = engine->findTrackLocked(trackId);
    const Region* region =
        track ? engine->findRegionLocked(trackId, regionId) : nullptr;
    if (!region) return 0;
    Region copy = *region;
    if (state) copy.applyEditableState(*state);
    copy.id = engine->allocateRegionIdLocked();
    if (copy.id == 0) return 0;
    track->addRegion(copy);
    return copy.id;
}

size_t HirariUnifiedEngine::regionHostCompRanges(void* host, uint32_t trackId,
                                                 uint32_t regionId,
                                                 HirariRegionCompRange* out, size_t capacity,
                                                 bool* managed) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    const Region* region = engine->findRegionLocked(trackId, regionId);
    if (!region) return 0;
    if (managed) *managed = region->compManaged;
    if (out && capacity >= region->compRanges.size()) {
        for (size_t index = 0; index < region->compRanges.size(); ++index) {
            const auto& range = region->compRanges[index];
            out[index] = {range.start, range.end, range.fadeIn, range.fadeOut};
        }
    }
    return region->compRanges.size();
}

size_t HirariUnifiedEngine::regionHostListSyncGroup(void* host, uint32_t group,
                                                    HirariRegionGroupMember* out,
                                                    size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    size_t count = 0;
    for (const auto& track : engine->m_tracks) {
        if (!track) continue;
        for (const auto& region : track->getRegions()) {
            if (region.syncGroup != group) continue;
            if (out && count < capacity) {
                out[count].trackId = track->getId();
                out[count].regionId = region.id;
                out[count].start = region.start;
                out[count].baseStart = region.baseStart;
                out[count].locked = region.locked;
            }
            ++count;
        }
    }
    return count;
}

bool HirariUnifiedEngine::regionHostNoteEdit(void* host, uint32_t trackId, uint32_t regionId,
                                             void* context, HirariRegionNoteMutateFn mutate) {
    if (!host || !mutate) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    Track* track = engine->findTrackLocked(trackId);
    return track && track->editRegionNoteStore(regionId, context, mutate);
}

namespace {

const HirariRegionHostOps kRegionEditHostOps = {
    &HirariUnifiedEngine::regionHostFind,
    &HirariUnifiedEngine::regionHostApply,
    &HirariUnifiedEngine::regionHostListSyncGroup,
    nullptr,  // undo capture lives in the Rust region edit host.
    &HirariUnifiedEngine::regionHostNoteEdit,
    &HirariUnifiedEngine::regionHostSnapshot,
    &HirariUnifiedEngine::regionHostReleaseSnapshot,
    &HirariUnifiedEngine::regionHostWarpMarkers,
    &HirariUnifiedEngine::regionHostRangeEdits,
    &HirariUnifiedEngine::regionHostRetainAudio,
    &HirariUnifiedEngine::regionHostAudioView,
    &HirariUnifiedEngine::regionHostReleaseAudio,
    &HirariUnifiedEngine::regionHostTrackToken,
    &HirariUnifiedEngine::regionHostListTracks,
    &HirariUnifiedEngine::regionHostListRegions,
    &HirariUnifiedEngine::regionHostTempoState,
    &HirariUnifiedEngine::regionHostUndoState,
    &HirariUnifiedEngine::regionHostSampleRate,
    &HirariUnifiedEngine::regionHostRegionPath,
    &HirariUnifiedEngine::regionHostDecodeAudio,
    &HirariUnifiedEngine::regionHostInsertRegion,
    &HirariUnifiedEngine::regionHostTrackIdentity,
    &HirariUnifiedEngine::regionHostRetainTrack,
    &HirariUnifiedEngine::regionHostReleaseTrack,
    &HirariUnifiedEngine::regionHostPitchBegin,
    &HirariUnifiedEngine::regionHostPitchFinish,
    &HirariUnifiedEngine::regionHostNoteSnapshot,
    &HirariUnifiedEngine::regionHostClone,
    &HirariUnifiedEngine::regionHostCompRanges,
};

}  // namespace

// Differential check for the Rust region-table mirror: true when the mirror
// of the track with id trackId equals its authoritative C++ region list, entry for entry.
bool HirariUnifiedEngine::region_table_mirror_matches(uint32_t trackId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() != trackId) continue;
        const auto& regions = track->getRegions();
        const HirariRegionTable* mirror = track->regionTableMirror();
        if (hirari_region_table_len(mirror) != regions.size()) return false;
        for (size_t index = 0; index < regions.size(); ++index) {
            uint32_t id = 0;
            HirariRegionState mirrored;
            if (!hirari_region_table_get(mirror, index, &id, &mirrored)) return false;
            HirariRegionState expected;
            fillRegionHostState(regions[index], expected);
            const auto& r = regions[index];
            const auto sameBits = [](double a, double b) {
                return std::memcmp(&a, &b, sizeof(a)) == 0;
            };
            if (id != r.id ||
                mirrored.start != expected.start || mirrored.len != expected.len ||
                mirrored.baseStart != expected.baseStart ||
                mirrored.baseLength != expected.baseLength ||
                mirrored.sourceOffset != expected.sourceOffset ||
                mirrored.baseSourceOffset != expected.baseSourceOffset ||
                mirrored.sourceLength != expected.sourceLength ||
                mirrored.fadeInSamples != expected.fadeInSamples ||
                mirrored.fadeOutSamples != expected.fadeOutSamples ||
                !sameBits(mirrored.warpRatio, expected.warpRatio) ||
                !sameBits(mirrored.sourceSampleRate, expected.sourceSampleRate) ||
                !sameBits(mirrored.timelineSampleRate, expected.timelineSampleRate) ||
                !sameBits(mirrored.clipGain, expected.clipGain) ||
                !sameBits(mirrored.pitchSemitones, expected.pitchSemitones) ||
                mirrored.loopCount != expected.loopCount ||
                mirrored.syncGroup != expected.syncGroup ||
                mirrored.muted != expected.muted || mirrored.reverse != expected.reverse ||
                mirrored.locked != expected.locked ||
                mirrored.pitchPreserveWarp != expected.pitchPreserveWarp) return false;
        }
        return true;
    }
    return false;
}
