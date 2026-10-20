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
    if (!std::isfinite(value) || value < 0.0f || value > 2.0f) return false;
    const float before = m_masterGain.load(std::memory_order_acquire);
    m_masterGain.store(value, std::memory_order_release);
    if (before != value) {
        const auto apply = [this](float next) {
            m_masterGain.store(next, std::memory_order_release);
        };
        m_undoManager.recordAppliedAction(
            "Master Gain", [apply, before] { apply(before); }, [apply, value] { apply(value); });
    }
    return true;
}

float HirariUnifiedEngine::get_master_gain() const noexcept {
    return m_masterGain.load(std::memory_order_acquire);
}

bool HirariUnifiedEngine::set_automation_data(uint32_t tid, uint32_t param_id, const std::vector<AutomationPoint>& points) {
    if (param_id > 1) return false;
    const float minValue = param_id == 0 ? 0.0f : -1.0f;
    const float maxValue = param_id == 0 ? 2.0f : 1.0f;
    double previousTime = -1.0;
    for (const auto& point : points) {
        if (!std::isfinite(point.time) || point.time < 0.0 || std::trunc(point.time) != point.time
            || !std::isfinite(point.value) || point.value < minValue || point.value > maxValue
            || !std::isfinite(point.curve) || point.curve < -1.0f || point.curve > 1.0f
            || point.time <= previousTime) {
            return false;
        }
        previousTime = point.time;
    }
    std::vector<Hirari::Core::ProjectAutomationPoint> serializedPoints;
    serializedPoints.reserve(points.size());
    for (const auto& p : points) {
        Hirari::Core::ProjectAutomationPoint sp;
        sp.time = p.time;
        sp.value = p.value;
        sp.curve = p.curve;
        serializedPoints.push_back(sp);
    }

    // Snapshot the previous curve while holding the model lock, then release
    // it before registering the undo transaction. Undo callbacks reacquire the
    // lock later, so the audio/model lock is never held across a callback.
    std::vector<Hirari::Core::ProjectAutomationPoint> previousPoints;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& t : m_tracks) {
            if (t->getId() != tid) continue;
            previousPoints = param_id == 0
                ? t->getVolumeAutomationPoints()
                : t->getPanAutomationPoints();
            break;
        }
    }
    if (previousPoints.empty() && serializedPoints.empty()) {
        // An empty curve is still a valid clear operation only when the track
        // exists; distinguish it from an unknown track below.
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        if (std::none_of(m_tracks.begin(), m_tracks.end(),
                         [tid](const auto& track) { return track->getId() == tid; })) {
            return false;
        }
    } else {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        if (std::none_of(m_tracks.begin(), m_tracks.end(),
                         [tid](const auto& track) { return track->getId() == tid; })) {
            return false;
        }
    }

    auto apply = [this, tid, param_id](const std::vector<Hirari::Core::ProjectAutomationPoint>& curve) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            if (param_id == 0) track->setVolumeAutomation(curve);
            else track->setPanAutomation(curve);
            return;
        }
    };
    const std::string actionName =
        std::string(param_id == 0 ? "Volume Automation Track " : "Pan Automation Track ") +
        std::to_string(tid);
    m_undoManager.performAction(
        actionName,
        [apply, previousPoints] { apply(previousPoints); },
        [apply, serializedPoints] { apply(serializedPoints); });
    return true;
}

bool HirariUnifiedEngine::set_plugin_automation(
    uint32_t tid, uint32_t pluginIndex, uint32_t parameterId,
    const std::vector<ProjectAutomationPoint>& points) {
    if (points.size() > 100'000) return false;
    double previousTime = -1.0;
    for (const auto& point : points) {
        if (!std::isfinite(point.time) || point.time < 0.0 || point.time > 9007199254740992.0 ||
            std::trunc(point.time) != point.time || point.time <= previousTime ||
            !std::isfinite(point.value) || point.value < 0.0f || point.value > 1.0f ||
            !std::isfinite(point.curve) || point.curve < -1.0f || point.curve > 1.0f) {
            return false;
        }
        previousTime = point.time;
    }

    std::shared_ptr<Track> target;
    std::vector<ProjectPluginAutomationLane> before;
    std::vector<ProjectPluginAutomationLane> after;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            if (pluginIndex >= track->getPluginTypes().size()) return false;
            target = track;
            before = track->getPluginAutomation();
            after = before;
            after.erase(std::remove_if(after.begin(), after.end(),
                [pluginIndex, parameterId](const auto& lane) {
                    return lane.pluginIndex == pluginIndex && lane.parameterId == parameterId;
                }), after.end());
            if (!points.empty()) {
                after.push_back({pluginIndex, parameterId, points});
            }
            break;
        }
    }
    if (!target) return false;

    auto apply = [target](const auto& lanes) { target->setPluginAutomation(lanes); };
    m_undoManager.performAction(
        "Plugin Parameter Automation Track " + std::to_string(tid),
        [apply, before] { apply(before); }, [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::set_track_delay_automation(uint32_t tid, const std::vector<AutomationPoint>& points) {
    if (points.size() > 8192) return false;
    double previousTime = -1.0;
    for (const auto& point : points) {
        if (!std::isfinite(point.time) || point.time < 0.0 || std::trunc(point.time) != point.time || point.time <= previousTime ||
            !std::isfinite(point.value) || point.value < 0.0f || point.value > 1.0f ||
            !std::isfinite(point.curve) || point.curve < -1.0f || point.curve > 1.0f) return false;
        previousTime = point.time;
    }
    std::vector<AutomationPoint> before;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                target = track;
                for (const auto& point : track->getTrackDelayAutomationPoints()) {
                    before.push_back({point.time, point.value, point.curve});
                }
                break;
            }
        }
    }
    if (!target) return false;
    auto apply = [target](const std::vector<AutomationPoint>& curve) {
        std::vector<Hirari::Core::ProjectAutomationPoint> serialized;
        serialized.reserve(curve.size());
        for (const auto& point : curve) {
            serialized.push_back({point.time, point.value, point.curve});
        }
        target->setTrackDelayAutomation(serialized);
    };
    m_undoManager.performAction(
        "Track Delay Automation " + std::to_string(tid),
        [apply, before] { apply(before); }, [apply, points] { apply(points); });
    return true;
}
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
