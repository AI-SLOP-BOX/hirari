float HirariUnifiedEngine::get_tempo() const {
    return hirari_tempo_map_get_current_bpm(m_tempoState);
}

bool HirariUnifiedEngine::set_tempo(float bpm) {
    // Thin entry point onto the Rust implementation, kept only because
    // tests/timeline_bpm_ghost_contract.cpp drives this method straight off
    // the engine singleton. Every other tempo edit reaches Rust without
    // entering C++ at all.
    return hirari_tempo_edit_set_tempo(
        m_tempoState, m_undoManager.rustState(),
        ::Hirari::Core::Diagnostics::LogBuffer::state(),
        m_playhead.load(std::memory_order_relaxed),
        m_sampleRate.load(std::memory_order_relaxed), bpm);
}

bool HirariUnifiedEngine::set_master_gain(float value) {
    // Storage only: the bridge owns the range check and the undo entry. The
    // audio callback reads this atomic once per block.
    m_masterGain.store(value, std::memory_order_release);
    return true;
}

float HirariUnifiedEngine::get_master_gain() const noexcept {
    return m_masterGain.load(std::memory_order_acquire);
}

size_t HirariUnifiedEngine::automationHostRead(void* host, uint32_t trackId, int32_t kind,
                                               uint32_t pluginIndex, uint32_t parameterId,
                                               Hirari::Core::ProjectAutomationPoint* out,
                                               size_t capacity, bool* targetFound) {
    if (targetFound) *targetFound = false;
    if (!host) return 0;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != trackId) continue;
        std::vector<Hirari::Core::ProjectAutomationPoint> points;
        if (kind == kHirariAutomationPlugin) {
            if (pluginIndex >= track->getPluginTypes().size()) return 0;
            for (const auto& lane : track->getPluginAutomation()) {
                if (lane.pluginIndex == pluginIndex && lane.parameterId == parameterId) {
                    points = lane.points;
                    break;
                }
            }
        } else if (kind == kHirariAutomationTrackDelay) {
            points = track->getTrackDelayAutomationPoints();
        } else if (kind == kHirariAutomationPan) {
            points = track->getPanAutomationPoints();
        } else {
            points = track->getVolumeAutomationPoints();
        }
        if (targetFound) *targetFound = true;
        if (out && capacity >= points.size()) {
            std::copy(points.begin(), points.end(), out);
        }
        return points.size();
    }
    return 0;
}

bool HirariUnifiedEngine::automationHostApply(void* host,
                                              const HirariAutomationEditPayload* payload) {
    if (!host || !payload) return false;
    if (payload->pointCount != 0 && payload->points == nullptr) return false;
    const std::vector<Hirari::Core::ProjectAutomationPoint> points(
        payload->points, payload->points + payload->pointCount);
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    std::lock_guard<std::mutex> lock(engine->m_tracksMutex);
    for (const auto& track : engine->m_tracks) {
        if (!track || track->getId() != payload->trackId) continue;
        switch (payload->kind) {
            case kHirariAutomationVolume:
                track->setVolumeAutomation(points);
                return true;
            case kHirariAutomationPan:
                track->setPanAutomation(points);
                return true;
            case kHirariAutomationTrackDelay:
                track->setTrackDelayAutomation(points);
                return true;
            case kHirariAutomationPlugin: {
                auto lanes = track->getPluginAutomation();
                lanes.erase(std::remove_if(lanes.begin(), lanes.end(),
                                           [payload](const auto& lane) {
                                               return lane.pluginIndex == payload->pluginIndex &&
                                                      lane.parameterId == payload->parameterId;
                                           }),
                            lanes.end());
                if (!points.empty()) {
                    lanes.push_back({payload->pluginIndex, payload->parameterId, points});
                }
                track->setPluginAutomation(lanes);
                return true;
            }
            default:
                return false;
        }
    }
    return false;
}

void HirariUnifiedEngine::automationHostRecordUndo(void* host, const char* name,
                                                   const HirariAutomationEditPayload* undo,
                                                   const HirariAutomationEditPayload* redo) {
    if (!host || !name || !undo || !redo) return;
    auto* engine = static_cast<HirariUnifiedEngine*>(host);
    struct OwnedAutomationEdit {
        HirariAutomationEditPayload payload;
        std::shared_ptr<std::vector<Hirari::Core::ProjectAutomationPoint>> points;
    };
    auto capture = [](const HirariAutomationEditPayload& source) {
        OwnedAutomationEdit entry;
        entry.payload = source;
        entry.points =
            std::make_shared<std::vector<Hirari::Core::ProjectAutomationPoint>>();
        if (source.pointCount != 0 && source.points != nullptr) {
            entry.points->assign(source.points, source.points + source.pointCount);
        }
        entry.payload.pointCount = entry.points->size();
        return entry;
    };
    const auto undoCopy = capture(*undo);
    const auto redoCopy = capture(*redo);
    auto applyOne = [host](const OwnedAutomationEdit& entry) {
        HirariAutomationEditPayload payload = entry.payload;
        payload.points = entry.points->data();
        (void)HirariUnifiedEngine::automationHostApply(host, &payload);
    };
    // Rust already applied the edit, so record the entry without replaying the
    // redo a second time.
    engine->m_undoManager.recordAppliedAction(name, [applyOne, undoCopy] { applyOne(undoCopy); },
                                              [applyOne, redoCopy] { applyOne(redoCopy); },
                                              false);
}

namespace {

const HirariAutomationHostOps kAutomationEditHostOps = {
    &HirariUnifiedEngine::automationHostRead,
    &HirariUnifiedEngine::automationHostApply,
    &HirariUnifiedEngine::automationHostRecordUndo,
};

[[maybe_unused]] const bool kAutomationEditHostInstalled = [] {
    hirari_automation_host_install(&kAutomationEditHostOps);
    return true;
}();

}  // namespace
bool HirariUnifiedEngine::set_spatial_position(uint32_t tid, float x, float y, float z) {
    if (!std::isfinite(x) || !std::isfinite(y) || !std::isfinite(z)) return false;
    // C-4 fix: use m_tracks under mutex instead of raw audioState pointers
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& t : m_tracks) {
        if (t->getId() == tid) {
            t->setSpatialPosition(x, y, z);
            return true;
        }
    }
    return false;
}

bool HirariUnifiedEngine::set_hrtf_kernel(uint32_t tid, const std::vector<float>& left,
                                        const std::vector<float>& right) {
    if (left.empty() || left.size() != right.size() ||
        left.size() > ::Hirari::DSP::Spatial::HolographicPanner::kMaxHrtfTaps) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->setHrtfKernel(left.data(), right.data(), static_cast<uint32_t>(left.size()));
    }
    return false;
}

bool HirariUnifiedEngine::clear_hrtf_kernel(uint32_t tid) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            track->clearHrtfKernel();
            return true;
        }
    }
    return false;
}
bool HirariUnifiedEngine::execute_auto_mixing() {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    if (m_tracks.empty()) return false;
    float targetVolume = 1.0f / std::sqrt(static_cast<float>(m_tracks.size()));
    for (auto& track : m_tracks) {
        if (track->getType() != Track::Type::Bus) {
            float currentVol = track->getVolume();
            float newVol = currentVol * 0.8f + targetVolume * 0.2f;
            track->setVolume(std::clamp(newVol, 0.0f, 1.0f));
        }
    }
    return true;
}
bool HirariUnifiedEngine::execute_auto_arrangement() {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    if (m_tracks.empty()) return false;
    
    // 1. GATHER TIMELINE DATA
    std::vector<uint64_t> regionStarts;
    uint64_t projectLen = 0;
    
    for (const auto& t : m_tracks) {
        for (const auto& r : t->getRegions()) {
            regionStarts.push_back(r.start);
            projectLen = std::max(projectLen, r.start + r.len);
        }
    }

    // 2. INDUSTRIAL: AI ANALYSIS (Neural Arrangement Kernel)
    // For this phase, we use the density analyzer to identify sections.
    const auto harmony = Composition::HarmonicContextTracker::getInstance().getState();
    auto sections = ::Hirari::Core::Bridge::analysis_structure_sections(
        rust::Slice<const uint64_t>(regionStarts.data(), regionStarts.size()),
        projectLen, hirari_tempo_map_get_current_bpm(m_tempoState), m_sampleRate.load(), harmony.tension, harmony.valence
    );

    // 3. GENERATE MARKERS & NOTIFY UI
    for (const auto& sec : sections) {
        ::Hirari::Core::EngineEvent ev{};
        ev.type = 0x50; // EVENT_TYPE_STRUCTURE_MARKER
        ev.timestamp = sec.start_sample;
        ev.value = static_cast<float>(sec.end_sample - sec.start_sample);
        push_event(ev);
        
        ::Hirari::Core::Diagnostics::LogBuffer::post(0, 0, "AI | ARRANGEMENT | SECTION_DETECTED");
    }
    return true;
}
