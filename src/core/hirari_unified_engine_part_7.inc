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
    auto& buffer = track->getWorkBuffer(1024);
    if (buffer.getNumChannels() < 2) return false;
    return hirari_vocal_remove_stereo(
        buffer.getWritePointer(0), buffer.getWritePointer(1), buffer.getNumSamples());
}

float HirariUnifiedEngine::get_track_correlation(uint32_t tid) const {
    Track* track = findPublishedTrack(tid);
    if (!track) return 1.0f;
    auto& buffer = track->getWorkBuffer(1024);
    if (buffer.getNumChannels() < 2) return 1.0f;
    return hirari_stereo_correlation(
        buffer.getReadPointer(0), buffer.getReadPointer(1), buffer.getNumSamples());
}

void HirariUnifiedEngine::push_event(const ::Hirari::Core::EngineEvent& e) { m_eventQueue.push(e); }
bool HirariUnifiedEngine::pop_event(::Hirari::Core::EngineEvent& e) const { return m_eventQueue.pop(e); }

