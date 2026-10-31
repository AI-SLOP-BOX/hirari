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

// The undo registry keeps its callbacks alive for the life of the process, so
// a note-table snapshot handed over by Rust is owned here and freed with the
// undo entry.
struct OwnedRegionEdit {
    HirariRegionEditPayload payload;
    std::shared_ptr<HirariRegionVariAudioStore> noteStore;
    // Whole-region copy owned by a Restore undo entry.
    std::shared_ptr<Region> regionSnapshot;
    // Warp markers owned by a Geometry or Markers undo entry.
    std::shared_ptr<std::vector<HirariWarpMarker>> warpMarkers;
    // Range edits owned by a RangeEdits undo entry.
    std::shared_ptr<std::vector<HirariRegionRangeEdit>> rangeEdits;
};

}  // namespace

bool HirariUnifiedEngine::regionHostFind(void* host, uint32_t trackId, uint32_t regionId,
                                         HirariRegionState* out) {
    if (!host || !out) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            fillRegionHostState(region, *out);
            return true;
        }
        return false;
    }
    return false;
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
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id == regionId) return new Region(region);
        }
        return nullptr;
    }
    return nullptr;
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
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            if (out && capacity >= region.warpMarkers.size()) {
                std::copy(region.warpMarkers.begin(), region.warpMarkers.end(), out);
            }
            return region.warpMarkers.size();
        }
        return 0;
    }
    return 0;
}

size_t HirariUnifiedEngine::regionHostRangeEdits(void* host, uint32_t trackId,
                                                 uint32_t regionId,
                                                 HirariRegionRangeEdit* out,
                                                 size_t capacity) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            if (out && capacity >= region.rangeEdits.size()) {
                for (size_t index = 0; index < region.rangeEdits.size(); ++index) {
                    const auto& edit = region.rangeEdits[index];
                    out[index] = {edit.start, edit.end, edit.gain, edit.fadeIn, edit.fadeOut};
                }
            }
            return region.rangeEdits.size();
        }
        return 0;
    }
    return 0;
}

void* HirariUnifiedEngine::regionHostRetainAudio(void* host, uint32_t trackId,
                                                 uint32_t regionId) {    if (!host) return nullptr;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            if (!region.audio) return nullptr;
            // The shared_ptr keeps the decoded buffer alive for the job even
            // after the region itself moves on; the worker only reads it.
            return new std::shared_ptr<AudioBuffer>(region.audio);
        }
        return nullptr;
    }
    return nullptr;
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
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        if (generation) *generation = track->regionGeneration();
        if (snapshot) *snapshot = reinterpret_cast<size_t>(track->getRegionSnapshot());
        return true;
    }
    return false;
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
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        size_t count = 0;
        for (const auto& region : track->getRegions()) {
            if (out && count < capacity) out[count] = region.id;
            ++count;
        }
        return count;
    }
    return 0;
}

void* HirariUnifiedEngine::regionHostTempoState(void* host) {
    return host ? static_cast<HirariUnifiedEngine*>(host)->m_tempoState : nullptr;
}

void* HirariUnifiedEngine::regionHostUndoState(void* host) {
    if (!host) return nullptr;
    return static_cast<HirariUnifiedEngine*>(host)->m_undoManager.rustState();
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
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            if (out && capacity >= region.path.size()) {
                std::copy(region.path.begin(), region.path.end(), out);
            }
            return region.path.size();
        }
        return 0;
    }
    return 0;
}

bool HirariUnifiedEngine::regionHostDecodeAudio(void* host, const uint8_t* path,
                                                size_t length, void** outHandle,
                                                double* outRate) {
    if (!host || (!path && length != 0) || !outHandle) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    double rate = 0.0;
    auto audio = engine->m_audioDecoderManager.importFile(
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
    double timelineSampleRate, void* audioHandle) {
    if (!host || !audioHandle || (!path && pathLength != 0)) return 0;
    // The handle is consumed on every path, success or failure.
    auto* ownedAudio = static_cast<std::shared_ptr<AudioBuffer>*>(audioHandle);
    auto audio = std::move(*ownedAudio);
    delete ownedAudio;
    if (!audio || audio->getNumSamples() == 0 || audio->getNumChannels() == 0 ||
        !std::isfinite(sourceSampleRate) || !std::isfinite(timelineSampleRate) ||
        sourceSampleRate <= 0.0 || timelineSampleRate <= 0.0) {
        return 0;
    }
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
        const long double timelineLength = std::ceil(
            static_cast<long double>(audio->getNumSamples()) * timelineSampleRate /
            sourceSampleRate);
        if (!std::isfinite(static_cast<double>(timelineLength)) || timelineLength < 1.0L ||
            timelineLength > static_cast<long double>(UINT64_MAX)) {
            return 0;
        }
        region.len = static_cast<uint64_t>(timelineLength);
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
    for (const auto& track : engine->m_tracks) {
        if (track && track->getId() == trackId) {
            return reinterpret_cast<size_t>(track.get());
        }
    }
    return 0;
}

void* HirariUnifiedEngine::regionHostRetainTrack(void* host, uint32_t trackId) {
    if (!host) return nullptr;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (track && track->getId() == trackId) {
            return new std::shared_ptr<Track>(track);
        }
    }
    return nullptr;
}

void HirariUnifiedEngine::regionHostReleaseTrack(void* handle) {
    delete static_cast<std::shared_ptr<Track>*>(handle);
}

bool HirariUnifiedEngine::regionHostPitchBegin(void* trackHandle) {
    if (!trackHandle) return false;
    const auto track = *static_cast<const std::shared_ptr<Track>*>(trackHandle);
    return track && track->beginAudioPitchAnalysis();
}

bool HirariUnifiedEngine::regionHostPitchAnalyze(void* trackHandle, uint32_t regionId,
                                                 double sampleRate) {
    if (!trackHandle) return false;
    const auto track = *static_cast<const std::shared_ptr<Track>*>(trackHandle);
    if (!track) return false;
    try {
        return track->analyzeAudioNoteSegments(regionId, sampleRate);
    } catch (...) {
        return false;
    }
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
    for (auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            Region copy = region;
            if (state) copy.applyEditableState(*state);
            copy.id = engine->allocateRegionIdLocked();
            if (copy.id == 0) return 0;
            track->addRegion(copy);
            return copy.id;
        }
        return 0;
    }
    return 0;
}

size_t HirariUnifiedEngine::regionHostCompRanges(void* host, uint32_t trackId,
                                                 uint32_t regionId,
                                                 HirariRegionCompRange* out, size_t capacity,
                                                 bool* managed) {
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != regionId) continue;
            if (managed) *managed = region.compManaged;
            if (out && capacity >= region.compRanges.size()) {
                for (size_t index = 0; index < region.compRanges.size(); ++index) {
                    const auto& range = region.compRanges[index];
                    out[index] = {range.start, range.end, range.fadeIn, range.fadeOut};
                }
            }
            return region.compRanges.size();
        }
        return 0;
    }
    return 0;
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
    for (auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        return track->editRegionNoteStore(regionId, context, mutate);
    }
    return false;
}

bool HirariUnifiedEngine::regionHostRecordUndo(void* host, const char* name,
                                               const HirariRegionEditPayload* undo,
                                               size_t undoCount,
                                               const HirariRegionEditPayload* redo,
                                               size_t redoCount, bool coalesce, bool perform) {
    if (!host || !name) return false;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    auto capture = [](const HirariRegionEditPayload* source, size_t count) {
        std::vector<OwnedRegionEdit> entries;
        entries.reserve(count);
        for (size_t index = 0; index < count; ++index) {
            OwnedRegionEdit entry;
            entry.payload = source[index];
            if (source[index].op == kHirariRegionEditNoteStore && source[index].blob) {
                auto* handle = reinterpret_cast<HirariRegionVariAudioStore*>(
                    const_cast<uint8_t*>(source[index].blob));
                entry.noteStore = std::shared_ptr<HirariRegionVariAudioStore>(
                    handle, [](HirariRegionVariAudioStore* store) {
                        hirari_region_vari_audio_store_clear(store);
                    });
            }
            if (source[index].op == kHirariRegionEditRestore && source[index].blob) {
                entry.regionSnapshot = std::shared_ptr<Region>(
                    reinterpret_cast<Region*>(const_cast<uint8_t*>(source[index].blob)));
            }
            if ((source[index].op == kHirariRegionEditGeometry ||
                 source[index].op == kHirariRegionEditMarkers) && source[index].blob) {
                const auto count = source[index].blobLen / sizeof(HirariWarpMarker);
                const auto* markers =
                    reinterpret_cast<const HirariWarpMarker*>(source[index].blob);
                entry.warpMarkers = std::make_shared<std::vector<HirariWarpMarker>>(
                    markers, markers + count);
                entry.payload.blobLen = count * sizeof(HirariWarpMarker);
            }
            if (source[index].op == kHirariRegionEditRangeEdits && source[index].blob) {
                const auto count = source[index].blobLen / sizeof(HirariRegionRangeEdit);
                const auto* edits =
                    reinterpret_cast<const HirariRegionRangeEdit*>(source[index].blob);
                entry.rangeEdits = std::make_shared<std::vector<HirariRegionRangeEdit>>(
                    edits, edits + count);
                entry.payload.blobLen = count * sizeof(HirariRegionRangeEdit);
            }
            entries.push_back(std::move(entry));
        }
        return entries;
    };
    const auto undoCopy = capture(undo, undoCount);
    const auto redoCopy = capture(redo, redoCount);
    auto applyAll = [host](const std::vector<OwnedRegionEdit>& entries) {
        for (const auto& entry : entries) {
            HirariRegionEditPayload payload = entry.payload;
            if (entry.regionSnapshot) {
                payload.blob = reinterpret_cast<const uint8_t*>(entry.regionSnapshot.get());
                payload.blobLen = 0;
            }
            if (entry.noteStore) {
                payload.blob = reinterpret_cast<const uint8_t*>(entry.noteStore.get());
                payload.blobLen = 0;
            }
            if (entry.warpMarkers) {
                payload.blob =
                    reinterpret_cast<const uint8_t*>(entry.warpMarkers->data());
            }
            if (entry.rangeEdits) {
                payload.blob =
                    reinterpret_cast<const uint8_t*>(entry.rangeEdits->data());
            }
            (void)HirariUnifiedEngine::regionHostApply(host, &payload);
        }
    };
    if (perform) {
        // The legacy perform path applied the edit through the redo callback,
        // so replaying it here is the same contract, not an extra mutation.
        engine->m_undoManager.performAction(
            name, [applyAll, undoCopy] { applyAll(undoCopy); },
            [applyAll, redoCopy] { applyAll(redoCopy); });
        return true;
    }
    // Rust already applied the edit, so record the entry without replaying the
    // redo a second time.
    return engine->m_undoManager.recordAppliedAction(
        name,
        [applyAll, undoCopy] { applyAll(undoCopy); },
        [applyAll, redoCopy] { applyAll(redoCopy); },
        coalesce);
}

namespace {

const HirariRegionHostOps kRegionEditHostOps = {
    &HirariUnifiedEngine::regionHostFind,
    &HirariUnifiedEngine::regionHostApply,
    &HirariUnifiedEngine::regionHostListSyncGroup,
    &HirariUnifiedEngine::regionHostRecordUndo,
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
    &HirariUnifiedEngine::regionHostPitchAnalyze,
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
