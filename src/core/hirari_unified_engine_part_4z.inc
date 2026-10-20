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
        const bool applied = payload->op == kHirariRegionEditState
            ? track->applyRegionState(payload->regionId, payload->state)
            : track->applyRegionScalarFields(payload->regionId, payload->state);
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

void HirariUnifiedEngine::regionHostRecordUndo(void* host, const char* name,
                                               const HirariRegionEditPayload* undo,
                                               size_t undoCount,
                                               const HirariRegionEditPayload* redo,
                                               size_t redoCount) {
    if (!host || !name) return;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::vector<HirariRegionEditPayload> undoCopy;
    undoCopy.reserve(undoCount);
    for (size_t index = 0; index < undoCount; ++index) undoCopy.push_back(undo[index]);
    std::vector<HirariRegionEditPayload> redoCopy;
    redoCopy.reserve(redoCount);
    for (size_t index = 0; index < redoCount; ++index) redoCopy.push_back(redo[index]);
    auto applyAll = [host](const std::vector<HirariRegionEditPayload>& payloads) {
        for (const auto& payload : payloads) {
            (void)HirariUnifiedEngine::regionHostApply(host, &payload);
        }
    };
    // Rust already applied the edit, so record the entry without replaying the
    // redo a second time.
    engine->m_undoManager.recordAppliedAction(
        name,
        [applyAll, undoCopy] { applyAll(undoCopy); },
        [applyAll, redoCopy] { applyAll(redoCopy); },
        /*coalesce=*/true);
}

namespace {

const HirariRegionHostOps kRegionEditHostOps = {
    &HirariUnifiedEngine::regionHostFind,
    &HirariUnifiedEngine::regionHostApply,
    &HirariUnifiedEngine::regionHostListSyncGroup,
    &HirariUnifiedEngine::regionHostRecordUndo,
};

[[maybe_unused]] const bool kRegionEditHostInstalled = [] {
    hirari_region_host_install(&kRegionEditHostOps);
    return true;
}();

}  // namespace
