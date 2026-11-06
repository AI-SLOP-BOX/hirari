// Pitch analysis is a Rust job (region_jobs.rs): rate validation, the worker,
// its registry limits, the staleness check and the undo pair all live there.
// The engine only exposes the retained-track storage callbacks (part_4z).

// Alignment and sync-group quantize are Rust jobs: validation, the job
// registry, analysis threads, staleness checks and the marker commit all live
// in hirari-core-bridge/src/region_time_jobs.rs. The engine only provides the
// host callbacks (part_4z) that expose storage.

bool HirariUnifiedEngine::append_region_processing(uint32_t tid, uint32_t rid, uint32_t stepId, std::string_view operation, float parameter, bool enabled) {
    const HirariEventStep step{stepId, reinterpret_cast<const uint8_t*>(operation.data()),
                               operation.size(), parameter, enabled};
    void* track = findPublishedTrack(tid);
    return track != nullptr && hirari_region_store_append_processing(track, rid, step);
}
bool HirariUnifiedEngine::clear_region_processing(uint32_t tid, uint32_t rid) {
    void* track = findPublishedTrack(tid);
    return track != nullptr && hirari_region_store_clear_processing(track, rid);
}
