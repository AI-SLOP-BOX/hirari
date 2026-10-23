// Region edit host bridge.
//
// Rust owns every region edit decision and the undo capture; this file is the
// only place where that policy reaches the engine's region storage. Each
// callback runs on the control thread, under the same locks the previous
// per-method C++ code took.

namespace {

void fillRegionHostState(const Region& region, HirariRegionState& out) {
    out.start = region.start;
    out.len = region.len;
    out.baseStart = region.baseStart;
    out.baseLength = region.baseLength;
    out.sourceOffset = region.sourceOffset;
    out.baseSourceOffset = region.baseSourceOffset;
    out.sourceLength = region.sourceLength;
    out.fadeInSamples = region.fadeInSamples;
    out.fadeOutSamples = region.fadeOutSamples;
    out.warpRatio = region.warpRatio;
    out.sourceSampleRate = region.sourceSampleRate;
    out.timelineSampleRate = region.timelineSampleRate;
    out.clipGain = region.clipGain;
    out.pitchSemitones = region.pitchSemitones;
    out.loopCount = region.loopCount;
    out.syncGroup = region.syncGroup;
    out.muted = region.muted;
    out.reverse = region.reverse;
    out.locked = region.locked;
    out.pitchPreserveWarp = region.pitchPreserveWarp;
}

// The undo registry keeps its callbacks alive for the life of the process, so
// a note-table snapshot handed over by Rust is owned here and freed with the
// undo entry.
struct OwnedRegionEdit {
    HirariRegionEditPayload payload;
    std::shared_ptr<HirariRegionVariAudioStore> noteStore;
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
        case kHirariRegionEditNoteStore:
            applied = track->editRegionNoteStore(
                payload->regionId, const_cast<uint8_t*>(payload->blob),
                &hirari_region_note_store_restore);
            break;
        default:
            applied = track->applyRegionScalarFields(payload->regionId, payload->state);
            break;
        }
        if (applied) {
            std::lock_guard<std::mutex> cacheLock(engine->m_cacheMutex);
            engine->m_peakCache.erase(payload->regionId);
        }
        return applied;
    }
    return false;
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

void HirariUnifiedEngine::regionHostRecordUndo(void* host, const char* name,
                                               const HirariRegionEditPayload* undo,
                                               size_t undoCount,
                                               const HirariRegionEditPayload* redo,
                                               size_t redoCount, bool coalesce) {
    if (!host || !name) return;
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
            entries.push_back(std::move(entry));
        }
        return entries;
    };
    const auto undoCopy = capture(undo, undoCount);
    const auto redoCopy = capture(redo, redoCount);
    auto applyAll = [host](const std::vector<OwnedRegionEdit>& entries) {
        for (const auto& entry : entries) {
            HirariRegionEditPayload payload = entry.payload;
            if (entry.noteStore) {
                payload.blob = reinterpret_cast<const uint8_t*>(entry.noteStore.get());
                payload.blobLen = 0;
            }
            (void)HirariUnifiedEngine::regionHostApply(host, &payload);
        }
    };
    // Rust already applied the edit, so record the entry without replaying the
    // redo a second time.
    engine->m_undoManager.recordAppliedAction(
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
};

[[maybe_unused]] const bool kRegionEditHostInstalled = [] {
    hirari_region_host_install(&kRegionEditHostOps);
    return true;
}();

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
