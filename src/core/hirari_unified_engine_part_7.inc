bool HirariUnifiedEngine::set_automation_record_mode(uint32_t mode) {
    // Keep the wire value aligned with AutomationMode: 0=Read, 1=Write,
    // 2=Touch, 3=Latch. Unknown values must fail closed to Read rather than
    // accidentally enabling write automation for a future/invalid enum.
    const uint32_t normalized = mode <= 4u ? mode : 0u;
    m_autoRecordMode.store(normalized, std::memory_order_release);
    // Keep the standalone recorder and the engine transport state in lockstep
    // so control-surface/API writes are captured by the same mode semantics.
    enum class RecorderMode : uint32_t { Off = 0, Touch = 1, Latch = 2, Write = 3, AutoPunch = 4 };
    hirari_automation_recorder_set_mode(hirari_automation_recorder_instance(),
        static_cast<uint32_t>(normalized == 0u ? RecorderMode::Off
        : normalized == 1u ? RecorderMode::Write
        : normalized == 2u ? RecorderMode::Touch
        : normalized == 3u ? RecorderMode::Latch
        : RecorderMode::AutoPunch));
    return true;
}

std::string HirariUnifiedEngine::plugin_compatibility_snapshot_json() const {
    return m_pluginCompatibility.snapshotJson();
}

bool HirariUnifiedEngine::execute_vocal_remover(uint32_t tid) {
    const auto& s = m_audioStates[m_activeStateIdx.load()];
    for (uint32_t i = 0; i < s.trackCount && i < 256; ++i) {
        Track* t = s.activeTracks[i];
        if (t->getId() == tid) {
            auto& buffer = t->getWorkBuffer(1024);
            if (buffer.getNumChannels() >= 2) {
                return hirari_vocal_remove_stereo(
                    buffer.getWritePointer(0), buffer.getWritePointer(1), buffer.getNumSamples());
            }
            return false;
        }
    }
    return false;
}

float HirariUnifiedEngine::get_track_correlation(uint32_t tid) const {
    const auto& s = m_audioStates[m_activeStateIdx.load()];
    for (uint32_t i = 0; i < s.trackCount && i < 256; ++i) {
        Track* t = s.activeTracks[i];
        if (t->getId() == tid) {
            auto& buffer = t->getWorkBuffer(1024);
            if (buffer.getNumChannels() < 2) return 1.0f;

            const float* pL = buffer.getReadPointer(0);
            const float* pR = buffer.getReadPointer(1);
            return hirari_stereo_correlation(pL, pR, buffer.getNumSamples());
        }
    }
    return 1.0f;
}

void HirariUnifiedEngine::push_event(const ::Hirari::Core::EngineEvent& e) { m_eventQueue.push(e); }
bool HirariUnifiedEngine::pop_event(::Hirari::Core::EngineEvent& e) const { return m_eventQueue.pop(e); }








HirariUnifiedEngine::WaveformCache::~WaveformCache() {
    cleanup();
}
void HirariUnifiedEngine::WaveformCache::cleanup() {
    if (rustMapping != nullptr) {
        hirari_waveform_cache_map_destroy(rustMapping);
        rustMapping = nullptr;
    }
    valid.store(false, std::memory_order_release);
}
