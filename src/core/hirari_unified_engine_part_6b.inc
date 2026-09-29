}

bool HirariUnifiedEngine::set_time_signature_event(double beat, uint8_t numerator,
                                                  uint8_t denominator) {
    if (!std::isfinite(beat) || beat < 0.0 || numerator == 0 || numerator > 32 ||
        !(denominator == 1 || denominator == 2 || denominator == 4 ||
          denominator == 8 || denominator == 16 || denominator == 32)) return false;
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
    const auto before = m_tempoMap.getTimeSignatureAt(beat);
    m_tempoMap.addTimeSignature(beat, numerator, denominator, sampleRate);
    auto& tempoMap = m_tempoMap;
    const auto apply = [beat, sampleRate, &tempoMap](
        const std::optional<Hirari::Core::Engine::TempoMap::TimeSignatureEvent>& event) {
        if (event) {
            tempoMap.addTimeSignature(beat, event->numerator, event->denominator, sampleRate);
        } else if (beat > 0.0) {
            (void)tempoMap.removeTimeSignature(beat, sampleRate);
        }
    };
    m_undoManager.recordAppliedAction(
        "Time Signature", [apply, before] { apply(before); },
        [apply, beat, numerator, denominator] {
            apply(Hirari::Core::Engine::TempoMap::TimeSignatureEvent{
                0, numerator, denominator, beat});
        });
    return true;
}

bool HirariUnifiedEngine::remove_time_signature_event(double beat) {
    if (!std::isfinite(beat) || beat <= 0.0) return false;
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
    const auto before = m_tempoMap.getTimeSignatureAt(beat);
    if (!before) return false;
    auto& tempoMap = m_tempoMap;
    const auto apply = [beat, sampleRate, &tempoMap](
        const std::optional<Hirari::Core::Engine::TempoMap::TimeSignatureEvent>& event) {
        if (event) {
            tempoMap.addTimeSignature(beat, event->numerator, event->denominator, sampleRate);
        } else {
            (void)tempoMap.removeTimeSignature(beat, sampleRate);
        }
    };
    m_undoManager.performAction("Remove Time Signature", [apply, before] { apply(before); },
                                [apply] { apply(std::nullopt); });
    return true;
}

void HirariUnifiedEngine::clear_tempo_events(double initialBpm) {
    if (!std::isfinite(initialBpm) || initialBpm < 20.0 || initialBpm > 300.0) return;
    m_tempoMap.clear(m_sampleRate.load(std::memory_order_relaxed), initialBpm);
    m_tempoMap.setCurrentBpm(static_cast<float>(initialBpm));
}

bool HirariUnifiedEngine::set_tempo_event(double beat, double bpm, bool ramp) {
    if (!std::isfinite(beat) || beat < 0.0 || !std::isfinite(bpm) || bpm < 20.0 || bpm > 300.0) {
        return false;
    }
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const uint64_t sample = m_tempoMap.beatsToSamples(beat, sampleRate);
    auto& tempoMap = m_tempoMap;
    const auto before = tempoMap.getEventAt(sample);
    const Hirari::Core::Engine::TempoMap::Event after{sample, bpm, ramp, 0.0};
    auto apply = [sample, sampleRate, this, &tempoMap](
                     const std::optional<Hirari::Core::Engine::TempoMap::Event>& event) {
        if (event) {
            tempoMap.addTempo(sample, event->bpm, sampleRate, event->ramp);
            if (sample == 0) m_tempoMap.setCurrentBpm(static_cast<float>(event->bpm));
        } else {
            tempoMap.removeTempo(sample, sampleRate);
        }
    };
    m_undoManager.performAction(
        "Tempo Event " + std::to_string(sample),
        [apply, before] { apply(before); }, [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::set_tempo_transition_event(double beat, double bpm, bool ramp) {
    if (!std::isfinite(beat) || beat < 0.0 || !std::isfinite(bpm) || bpm < 20.0 || bpm > 300.0) {
        return false;
    }
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    auto& tempoMap = m_tempoMap;
    const auto before = tempoMap.getEvents();
    if (before.empty()) return false;

    auto after = before;
    auto target = std::find_if(after.begin(), after.end(), [beat](const auto& event) {
        return std::fabs(event.worldBeats - beat) <= 0.001;
    });
    const double targetBeat = target == after.end() ? beat : target->worldBeats;
    if (target == after.end()) {
        after.push_back({0, bpm, false, targetBeat});
        target = std::prev(after.end());
    } else {
        target->bpm = bpm;
    }

    if (targetBeat <= 0.001) {
        target->worldBeats = 0.0;
        target->bpm = bpm;
        target->ramp = ramp;
    } else {
        auto previous = after.end();
        for (auto event = after.begin(); event != after.end(); ++event) {
            if (event->worldBeats < targetBeat - 0.001 &&
                (previous == after.end() || event->worldBeats > previous->worldBeats)) {
                previous = event;
            }
        }
        if (previous == after.end()) return false;
        previous->ramp = ramp;
    }

    std::sort(after.begin(), after.end(), [](const auto& left, const auto& right) {
        return left.worldBeats < right.worldBeats;
    });
    if (!tempoMap.replaceEventsAtBeats(after, sampleRate)) return false;

    const auto apply = [this, sampleRate](const std::vector<Hirari::Core::Engine::TempoMap::Event>& events) {
        if (!m_tempoMap.replaceEventsAtBeats(events, sampleRate)) return;
        if (!events.empty()) m_tempoMap.setCurrentBpm(static_cast<float>(events.front().bpm));
    };
    m_tempoMap.setCurrentBpm(static_cast<float>(after.front().bpm));
    m_undoManager.recordAppliedAction(
        "Set Tempo Transition",
        [apply, before] { apply(before); },
        [apply, after] { apply(after); });
    return true;
}

bool HirariUnifiedEngine::remove_tempo_event(double beat) {
    if (!std::isfinite(beat) || beat <= 0.0) return false;
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const uint64_t sample = m_tempoMap.beatsToSamples(beat, sampleRate);
    auto& tempoMap = m_tempoMap;
    const auto before = tempoMap.getEventAt(sample);
    if (!before) return false;
    auto apply = [sample, sampleRate, this, &tempoMap](
                     const std::optional<Hirari::Core::Engine::TempoMap::Event>& event) {
        if (event) {
            tempoMap.addTempo(sample, event->bpm, sampleRate, event->ramp);
            if (sample == 0) m_tempoMap.setCurrentBpm(static_cast<float>(event->bpm));
        } else {
            tempoMap.removeTempo(sample, sampleRate);
        }
    };
    m_undoManager.performAction(
        "Remove Tempo Event", [apply, before] { apply(before); }, [apply] { apply(std::nullopt); });
    return true;
}

bool HirariUnifiedEngine::move_tempo_event(double from_beat, double to_beat) {
    if (!std::isfinite(from_beat) || !std::isfinite(to_beat) ||
        from_beat <= 0.0 || to_beat <= 0.0 || std::fabs(from_beat - to_beat) < 1.0e-6) {
        return false;
    }
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    auto& tempoMap = m_tempoMap;
    const uint64_t fromSample = tempoMap.beatsToSamples(from_beat, sampleRate);
    const uint64_t toSample = tempoMap.beatsToSamples(to_beat, sampleRate);
    if (fromSample == 0 || toSample == 0 || fromSample == toSample) return false;
    const auto before = tempoMap.getEventAt(fromSample);
    if (!before || tempoMap.getEventAt(toSample)) return false;
    auto apply = [fromSample, toSample, sampleRate, &tempoMap](bool forward) {
        const uint64_t removeSample = forward ? fromSample : toSample;
        const auto event = tempoMap.getEventAt(forward ? fromSample : toSample);
        if (!event) return;
        tempoMap.removeTempo(removeSample, sampleRate);
        const uint64_t addSample = forward ? toSample : fromSample;
        tempoMap.addTempo(addSample, event->bpm, sampleRate, event->ramp);
    };
    m_undoManager.performAction(
        "Move Tempo Event", [apply] { apply(false); }, [apply] { apply(true); });
    return true;
}

float HirariUnifiedEngine::get_tempo() const {
    return m_tempoMap.currentBpm();
}

bool HirariUnifiedEngine::set_tempo(float bpm) {
    if (!std::isfinite(bpm)) return false;
    const float bounded = std::clamp(bpm, 20.0f, 300.0f);
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
    const uint64_t sample = m_playhead.load(std::memory_order_relaxed);
    auto& tempoMap = m_tempoMap;
    const auto beforeEvent = tempoMap.getEventAt(sample);
    const float beforeTempo = m_tempoMap.currentBpm();

    // The ordinary tempo control writes an event at the current playhead.
    // Keep that event and the transport's displayed BPM in one history entry;
    // otherwise Undo changes the number shown by the UI but leaves the tempo
    // map used by beat/sample conversion behind.
    tempoMap.addTempo(sample, bounded, sampleRate);
    m_tempoMap.setCurrentBpm(bounded);

    const auto restore = [sample, sampleRate, this, &tempoMap, beforeEvent, beforeTempo]() {
        if (beforeEvent) {
            tempoMap.addTempo(sample, beforeEvent->bpm, sampleRate, beforeEvent->ramp);
        } else if (sample != 0) {
            (void)tempoMap.removeTempo(sample, sampleRate);
        }
        m_tempoMap.setCurrentBpm(beforeTempo);
    };
    const auto reapply = [sample, sampleRate, this, &tempoMap, bounded]() {
        tempoMap.addTempo(sample, bounded, sampleRate);
        m_tempoMap.setCurrentBpm(bounded);
    };
    m_undoManager.recordAppliedAction("Set Tempo", restore, reapply);
    return true;
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
    std::vector<Hirari::Core::ProjectSerializer::AutomationPoint> serializedPoints;
    serializedPoints.reserve(points.size());
    for (const auto& p : points) {
        Hirari::Core::ProjectSerializer::AutomationPoint sp;
        sp.time = p.time;
        sp.value = p.value;
        sp.curve = p.curve;
        serializedPoints.push_back(sp);
    }

    // Snapshot the previous curve while holding the model lock, then release
    // it before registering the undo transaction. Undo callbacks reacquire the
    // lock later, so the audio/model lock is never held across a callback.
    std::vector<Hirari::Core::ProjectSerializer::AutomationPoint> previousPoints;
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

    auto apply = [this, tid, param_id](const std::vector<Hirari::Core::ProjectSerializer::AutomationPoint>& curve) {
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
    const std::vector<ProjectSerializer::AutomationPoint>& points) {
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
    std::vector<ProjectSerializer::PluginAutomationLane> before;
    std::vector<ProjectSerializer::PluginAutomationLane> after;
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
        std::vector<Hirari::Core::ProjectSerializer::AutomationPoint> serialized;
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
void HirariUnifiedEngine::set_spatial_mode(uint32_t tid, uint32_t mode) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == tid) {
            track->setSpatialMode(mode);
            return;
        }
    }
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
        projectLen, m_tempoMap.currentBpm(), m_sampleRate.load(), harmony.tension, harmony.valence
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
