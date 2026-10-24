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


std::vector<::Hirari::Core::Composition::ArrangementSection> HirariUnifiedEngine::run_structural_analysis() {
    const auto& s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];

    std::vector<uint64_t> regionStarts;
    uint64_t maxLen = 0;

    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        Track* t = s.activeTracks[i];
        for (const auto& r : t->getRegions()) {
            regionStarts.push_back(r.start);
            maxLen = std::max(maxLen, r.start + r.len);
        }
    }
    if (regionStarts.empty()) {
        regionStarts.push_back(0);
        maxLen = static_cast<uint64_t>(m_sampleRate.load()) * 30; // default 30s
    }

    const auto harmony = Composition::HarmonicContextTracker::getInstance().getState();
    auto rustSections = ::Hirari::Core::Bridge::analysis_structure_sections(
        rust::Slice<const uint64_t>(regionStarts.data(), regionStarts.size()),
        maxLen, hirari_tempo_map_get_current_bpm(m_tempoState), m_sampleRate.load(), harmony.tension, harmony.valence);
    std::vector<::Hirari::Core::Composition::ArrangementSection> results;
    results.reserve(rustSections.size());
    for (const auto& section : rustSections) {
        results.push_back({section.start_sample, section.end_sample,
                           section.section_type, section.energy_level,
                           section.motivic_id, section.narrative_flow_score});
    }
    return results;
}





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
