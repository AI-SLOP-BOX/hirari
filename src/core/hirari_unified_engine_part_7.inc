bool HirariUnifiedEngine::set_automation_record_mode(uint32_t mode) {
    // Rust owns wire-mode normalization, the fail-closed unknown-value rule
    // and the recorder-mode translation; the atomic keeps the audio
    // callback's copy of the normalized wire value.
    m_autoRecordMode.store(hirari_automation_record_mode_apply(mode),
                           std::memory_order_release);
    return true;
}

bool HirariUnifiedEngine::execute_vocal_remover(uint32_t tid) {
    Track* track = findPublishedTrack(tid);
    if (!track) return false;
    void* buffer = track->workBufferStorage();
    if (hirari_audio_buffer_storage_channels(buffer) < 2) return false;
    return hirari_vocal_remove_stereo(
        hirari_audio_buffer_storage_write_pointer(buffer, 0),
        hirari_audio_buffer_storage_write_pointer(buffer, 1),
        hirari_audio_buffer_storage_samples(buffer));
}

float HirariUnifiedEngine::get_track_correlation(uint32_t tid) const {
    Track* track = findPublishedTrack(tid);
    if (!track) return 1.0f;
    void* buffer = track->workBufferStorage();
    if (hirari_audio_buffer_storage_channels(buffer) < 2) return 1.0f;
    return hirari_stereo_correlation(
        hirari_audio_buffer_storage_read_pointer(buffer, 0, 0),
        hirari_audio_buffer_storage_read_pointer(buffer, 1, 0),
        hirari_audio_buffer_storage_samples(buffer));
}

bool HirariUnifiedEngine::pop_event(::Hirari::Core::EngineEvent& e) const {
    return hirari_engine_event_ring_pop(m_engineRings, &e.timestamp, &e.type, &e.trackId, &e.value,
                                        reinterpret_cast<uint8_t*>(e.label), sizeof(e.label));
}

