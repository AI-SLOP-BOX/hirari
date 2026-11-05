bool HirariUnifiedEngine::set_automation_record_mode(uint32_t mode) {
    // Rust owns wire-mode normalization, the fail-closed unknown-value rule
    // and the recorder-mode translation; the block store keeps the audio
    // callback's copy of the normalized wire value.
    hirari_engine_block_set_auto_record_mode(handle(kHirariHandleEngineBlock),
                                             hirari_automation_record_mode_apply(mode));
    return true;
}

bool HirariUnifiedEngine::execute_vocal_remover(uint32_t tid) {
    void* track = findPublishedTrack(tid);
    if (!track) return false;
    void* buffer = hirari_track_storage(track, kHirariTrackStoreWorkBuffer);
    if (hirari_audio_buffer_storage_channels(buffer) < 2) return false;
    return hirari_vocal_remove_stereo(
        hirari_audio_buffer_storage_write_pointer(buffer, 0),
        hirari_audio_buffer_storage_write_pointer(buffer, 1),
        hirari_audio_buffer_storage_samples(buffer));
}

float HirariUnifiedEngine::get_track_correlation(uint32_t tid) const {
    void* track = findPublishedTrack(tid);
    if (!track) return 1.0f;
    void* buffer = hirari_track_storage(track, kHirariTrackStoreWorkBuffer);
    if (hirari_audio_buffer_storage_channels(buffer) < 2) return 1.0f;
    return hirari_stereo_correlation(
        hirari_audio_buffer_storage_read_pointer(buffer, 0, 0),
        hirari_audio_buffer_storage_read_pointer(buffer, 1, 0),
        hirari_audio_buffer_storage_samples(buffer));
}

bool HirariUnifiedEngine::pop_event(::Hirari::Core::EngineEvent& e) const {
    return hirari_engine_event_ring_pop(handle(kHirariHandleEngineRings), &e.timestamp, &e.type, &e.trackId, &e.value,
                                        reinterpret_cast<uint8_t*>(e.label), sizeof(e.label));
}

