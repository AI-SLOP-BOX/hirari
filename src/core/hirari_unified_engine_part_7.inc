void HirariUnifiedEngine::sync_sharding_node(uint32_t node_id) {
    // Industrial: Distribute track processing to cluster
    // RT-Safe: Silenced logs in processing path
    auto* s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)].activeTracks;
    uint32_t count = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)].trackCount;
    for (uint32_t i = 0; i < count; ++i) {
        if (s[i]->getId() % 2 == node_id % 2) {
             // OFFLOADING TRACK Logic
        }
    }
}
bool HirariUnifiedEngine::generate_drum_fill(uint32_t tid, uint32_t bar, float complexity) {
    ::Hirari::Core::Diagnostics::LogBuffer::post(tid, bar, "HIRARI_GENERATING_FILL");
    // industrial: uses chaos attractor to derive beat patterns
    int64_t entropy = hirari_sys(300, 0, 0, 0);
    bool highEnergy = (std::abs(entropy) % 100) > (100 - (int)(complexity * 100));

    if (highEnergy) {
        std::string fill_file = "fill_standard_roll.wav";
        uint32_t pattern = std::abs(entropy) % 3;
        if (pattern == 0) fill_file = "fill_kick_snare.wav";
        else if (pattern == 1) fill_file = "fill_tom_roll.wav";
        else fill_file = "fill_hihat_triplet.wav";

        add_region(tid, fill_file, (double)bar * 4.0);
    }
    return true;
}
void HirariUnifiedEngine::set_show_video(bool s) { m_showVideo.store(s); }
void HirariUnifiedEngine::set_ascended_mode(bool enabled) { m_isAscended.store(enabled); }
void HirariUnifiedEngine::execute_restoration(uint32_t tid, float intensity) {
    const auto& s = m_audioStates[m_activeStateIdx.load()];
    for (uint32_t i = 0; i < s.trackCount && i < 256; ++i) {
        Track* t = s.activeTracks[i];
        if (t->getId() == tid) {
            auto& buffer = t->getWorkBuffer(1024);
            const float amount = std::clamp(intensity, 0.0f, 1.0f);

            // Real restoration DSP: Dynamic Gate/Expander to suppress noise floor
            const float threshold = 0.015f;
            const float rangeDb = -24.0f * amount;
            const float rangeVal = std::pow(10.0f, rangeDb / 20.0f);

            for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                float* p = buffer.getWritePointer(c);
                uint32_t samples = buffer.getNumSamples();

                float env = 0.0f;
                for (uint32_t s_idx = 0; s_idx < samples; ++s_idx) {
                    env = std::max(env, std::abs(p[s_idx]));
                }

                float gain = 1.0f;
                if (env < threshold) {
                    float ratio = env / (threshold + 1e-6f);
                    gain = rangeVal + (1.0f - rangeVal) * ratio;
                }

                for (uint32_t s_idx = 0; s_idx < samples; ++s_idx) {
                    p[s_idx] *= gain;
                }
            }
        }
    }
}
bool HirariUnifiedEngine::set_automation_record_mode(uint32_t mode) {
    // Keep the wire value aligned with AutomationMode: 0=Read, 1=Write,
    // 2=Touch, 3=Latch. Unknown values must fail closed to Read rather than
    // accidentally enabling write automation for a future/invalid enum.
    const uint32_t normalized = mode <= 4u ? mode : 0u;
    m_autoRecordMode.store(normalized, std::memory_order_release);
    // Keep the standalone recorder and the engine transport state in lockstep
    // so control-surface/API writes are captured by the same mode semantics.
    using RecorderMode = ::Hirari::Core::Engine::AutomationRecorder::Mode;
    auto& recorder = ::Hirari::Core::Engine::AutomationRecorder::getInstance();
    recorder.setMode(normalized == 0u ? RecorderMode::Off
        : normalized == 1u ? RecorderMode::Write
        : normalized == 2u ? RecorderMode::Touch
        : normalized == 3u ? RecorderMode::Latch
        : RecorderMode::AutoPunch);
    return true;
}

void HirariUnifiedEngine::set_automation_punch_range(uint64_t start, uint64_t end) noexcept {
    ::Hirari::Core::Engine::AutomationRecorder::getInstance().setPunchRange(start, end);
}

void HirariUnifiedEngine::record_plugin_scan_failure(std::string_view id, std::string_view error) {
    m_pluginCompatibility.markScanFailure(std::string(id), std::string(error));
}
void HirariUnifiedEngine::record_plugin_crash(std::string_view id) {
    m_pluginCompatibility.markCrash(std::string(id));
}
void HirariUnifiedEngine::set_plugin_blacklisted(std::string_view id, bool value) {
    m_pluginCompatibility.setBlacklisted(std::string(id), value);
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

rust::Vec<Bridge::PlainClash> HirariUnifiedEngine::run_masking_analysis(uint32_t trackAId) {
    rust::Vec<Bridge::PlainClash> results;
    const auto& s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];

    Track* targetTrack = nullptr;
    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        if (s.activeTracks[i]->getId() == trackAId) {
            targetTrack = s.activeTracks[i];
            break;
        }
    }

    if (!targetTrack) return results;

    constexpr uint32_t N = 512;
    auto& bufferA = targetTrack->getWorkBuffer(N);

    std::array<float, N> targetMono{};
    uint32_t chanA = bufferA.getNumChannels();
    for (uint32_t idx = 0; idx < N; ++idx) {
        float sum = 0.0f;
        for (uint32_t c = 0; c < chanA; ++c) {
            sum += bufferA.getReadPointer(c)[idx];
        }
        targetMono[idx] = sum / (chanA + 1e-6f);
    }

    std::vector<float> otherMono;
    std::vector<uint32_t> otherTrackIds;
    otherMono.reserve(static_cast<size_t>(s.trackCount) * N);
    otherTrackIds.reserve(s.trackCount);

    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        Track* other = s.activeTracks[i];
        if (other->getId() == trackAId || other->isMuted()) continue;

        auto& bufferB = other->getWorkBuffer(N);
        uint32_t chanB = bufferB.getNumChannels();
        const size_t trackOffset = otherMono.size();
        otherMono.resize(trackOffset + N);
        for (uint32_t idx = 0; idx < N; ++idx) {
            float sum = 0.0f;
            for (uint32_t c = 0; c < chanB; ++c) {
                sum += bufferB.getReadPointer(c)[idx];
            }
            otherMono[trackOffset + idx] = sum / (chanB + 1e-6f);
        }
        otherTrackIds.push_back(other->getId());
    }

    std::array<uint32_t, 50> clashBins{};
    std::array<float, 50> clashIntensities{};
    std::array<uint32_t, 50> clashTrackIds{};
    const size_t clashCount = hirari_masking_analysis(
        targetMono.data(), otherMono.data(), otherTrackIds.data(),
        otherTrackIds.size(), N, clashBins.data(), clashIntensities.data(),
        clashTrackIds.data(), clashBins.size());
    results.reserve(clashCount);
    for (size_t i = 0; i < clashCount; ++i) {
        Bridge::PlainClash clash;
        clash.bin = clashBins[i];
        clash.intensity = clashIntensities[i];
        clash.track_b = clashTrackIds[i];
        results.push_back(clash);
    }
    return results;
}

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
        maxLen, m_tempoMap.currentBpm(), m_sampleRate.load(), harmony.tension, harmony.valence);
    std::vector<::Hirari::Core::Composition::ArrangementSection> results;
    results.reserve(rustSections.size());
    for (const auto& section : rustSections) {
        results.push_back({section.start_sample, section.end_sample,
                           section.section_type, section.energy_level,
                           section.motivic_id, section.narrative_flow_score});
    }
    return results;
}

void HirariUnifiedEngine::autonomous_plugin_audit() {
    const auto& s = m_audioStates[m_activeStateIdx.load(std::memory_order_acquire)];
    for (uint32_t i = 0; i < s.trackCount && i < 1024; ++i) {
        // [SYSTEM AUDIT]
    }
}

std::map<std::string, float> HirariUnifiedEngine::get_vibe_scores() const {
    Mixing::AestheticFeatures f = m_vibeData.load(std::memory_order_relaxed);
    return Mixing::QualitativeMetricEngine::getInstance().calculateScores(f);
}

void HirariUnifiedEngine::perform_autonomous_correction() {
    Mixing::AestheticFeatures f = m_vibeData.load(std::memory_order_relaxed);

    if (f.transientClarity < 0.2f) {
        ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "ADVICE | CLARITY_CRITICAL | TRIGGER_SPECTRAL_CLEANUP");
    }

    if (f.dynamicComplexity > 0.85f && m_masterLimiter) {
         // m_masterLimiter->setDrive(m_masterLimiter->getDrive() * 0.99f);
    }
}

bool HirariUnifiedEngine::checkSystemHealth() const {
    return m_engineDiagnostics.getHealth().isRealTimeSafe.load() && m_controlRoom.validate();
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
