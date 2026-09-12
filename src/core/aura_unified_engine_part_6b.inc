}

bool AuraUnifiedEngine::set_time_signature_event(double beat, uint8_t numerator,
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
        const std::optional<Aura::Core::Engine::TempoMap::TimeSignatureEvent>& event) {
        if (event) {
            tempoMap.addTimeSignature(beat, event->numerator, event->denominator, sampleRate);
        } else if (beat > 0.0) {
            (void)tempoMap.removeTimeSignature(beat, sampleRate);
        }
    };
    m_undoManager.recordAppliedAction(
        "Time Signature", [apply, before] { apply(before); },
        [apply, beat, numerator, denominator] {
            apply(Aura::Core::Engine::TempoMap::TimeSignatureEvent{
                0, numerator, denominator, beat});
        });
    return true;
}

void AuraUnifiedEngine::clear_tempo_events(double initialBpm) {
    if (!std::isfinite(initialBpm) || initialBpm < 20.0 || initialBpm > 300.0) return;
    m_tempoMap.clear(m_sampleRate.load(std::memory_order_relaxed), initialBpm);
    m_tempo.store(static_cast<float>(initialBpm), std::memory_order_relaxed);
}

bool AuraUnifiedEngine::set_tempo_event(double beat, double bpm, bool ramp) {
    if (!std::isfinite(beat) || beat < 0.0 || !std::isfinite(bpm) || bpm < 20.0 || bpm > 300.0) {
        return false;
    }
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const uint64_t sample = m_tempoMap.beatsToSamples(beat, sampleRate);
    auto& tempoMap = m_tempoMap;
    const auto before = tempoMap.getEventAt(sample);
    const Aura::Core::Engine::TempoMap::Event after{sample, bpm, ramp, 0.0};
    auto apply = [sample, sampleRate, &tempoMap, &tempo = m_tempo](
                     const std::optional<Aura::Core::Engine::TempoMap::Event>& event) {
        if (event) {
            tempoMap.addTempo(sample, event->bpm, sampleRate, event->ramp);
            if (sample == 0) tempo.store(static_cast<float>(event->bpm), std::memory_order_relaxed);
        } else {
            tempoMap.removeTempo(sample, sampleRate);
        }
    };
    m_undoManager.performAction(
        "Tempo Event", [apply, before] { apply(before); }, [apply, after] { apply(after); });
    return true;
}

bool AuraUnifiedEngine::remove_tempo_event(double beat) {
    if (!std::isfinite(beat) || beat <= 0.0) return false;
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const uint64_t sample = m_tempoMap.beatsToSamples(beat, sampleRate);
    auto& tempoMap = m_tempoMap;
    const auto before = tempoMap.getEventAt(sample);
    if (!before) return false;
    auto apply = [sample, sampleRate, &tempoMap, &tempo = m_tempo](
                     const std::optional<Aura::Core::Engine::TempoMap::Event>& event) {
        if (event) {
            tempoMap.addTempo(sample, event->bpm, sampleRate, event->ramp);
            if (sample == 0) tempo.store(static_cast<float>(event->bpm), std::memory_order_relaxed);
        } else {
            tempoMap.removeTempo(sample, sampleRate);
        }
    };
    m_undoManager.performAction(
        "Remove Tempo Event", [apply, before] { apply(before); }, [apply] { apply(std::nullopt); });
    return true;
}

bool AuraUnifiedEngine::move_tempo_event(double from_beat, double to_beat) {
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

float AuraUnifiedEngine::get_tempo() const {
    return m_tempo.load();
}

bool AuraUnifiedEngine::set_tempo(float bpm) {
    if (!std::isfinite(bpm)) return false;
    const float bounded = std::clamp(bpm, 20.0f, 300.0f);
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    if (!std::isfinite(sampleRate) || sampleRate <= 0.0) return false;
    const uint64_t sample = m_playhead.load(std::memory_order_relaxed);
    auto& tempoMap = m_tempoMap;
    const auto beforeEvent = tempoMap.getEventAt(sample);
    const float beforeTempo = m_tempo.load(std::memory_order_relaxed);

    // The ordinary tempo control writes an event at the current playhead.
    // Keep that event and the transport's displayed BPM in one history entry;
    // otherwise Undo changes the number shown by the UI but leaves the tempo
    // map used by beat/sample conversion behind.
    tempoMap.addTempo(sample, bounded, sampleRate);
    m_tempo.store(bounded, std::memory_order_relaxed);

    const auto restore = [sample, sampleRate, &tempoMap, beforeEvent, beforeTempo,
                          &tempo = m_tempo]() {
        if (beforeEvent) {
            tempoMap.addTempo(sample, beforeEvent->bpm, sampleRate, beforeEvent->ramp);
        } else if (sample != 0) {
            (void)tempoMap.removeTempo(sample, sampleRate);
        }
        tempo.store(beforeTempo, std::memory_order_relaxed);
    };
    const auto reapply = [sample, sampleRate, &tempoMap, bounded,
                          &tempo = m_tempo]() {
        tempoMap.addTempo(sample, bounded, sampleRate);
        tempo.store(bounded, std::memory_order_relaxed);
    };
    m_undoManager.recordAppliedAction("Set Tempo", restore, reapply);
    return true;
}

bool AuraUnifiedEngine::set_master_gain(float value) {
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

float AuraUnifiedEngine::get_master_gain() const noexcept {
    return m_masterGain.load(std::memory_order_acquire);
}

bool AuraUnifiedEngine::set_automation_data(uint32_t tid, uint32_t param_id, const std::vector<AutomationPoint>& points) {
    if (param_id > 1) return false;
    double previousTime = -1.0;
    for (const auto& point : points) {
        if (!std::isfinite(point.time) || point.time < 0.0 || std::trunc(point.time) != point.time
            || !std::isfinite(point.value) || point.value < 0.0f || point.value > 1.0f
            || !std::isfinite(point.curve) || point.curve < -1.0f || point.curve > 1.0f
            || point.time <= previousTime) {
            return false;
        }
        previousTime = point.time;
    }
    std::vector<Aura::Core::ProjectSerializer::AutomationPoint> serializedPoints;
    serializedPoints.reserve(points.size());
    for (const auto& p : points) {
        Aura::Core::ProjectSerializer::AutomationPoint sp;
        sp.time = p.time;
        sp.value = p.value;
        sp.curve = p.curve;
        serializedPoints.push_back(sp);
    }

    // Snapshot the previous curve while holding the model lock, then release
    // it before registering the undo transaction. Undo callbacks reacquire the
    // lock later, so the audio/model lock is never held across a callback.
    std::vector<Aura::Core::ProjectSerializer::AutomationPoint> previousPoints;
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

    auto apply = [this, tid, param_id](const std::vector<Aura::Core::ProjectSerializer::AutomationPoint>& curve) {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            if (param_id == 0) track->setVolumeAutomation(curve);
            else track->setPanAutomation(curve);
            return;
        }
    };
    m_undoManager.performAction(
        param_id == 0 ? "Volume Automation" : "Pan Automation",
        [apply, previousPoints] { apply(previousPoints); },
        [apply, serializedPoints] { apply(serializedPoints); });
    return true;
}

bool AuraUnifiedEngine::set_track_delay_automation(uint32_t tid, const std::vector<AutomationPoint>& points) {
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
        std::vector<Aura::Core::ProjectSerializer::AutomationPoint> serialized;
        serialized.reserve(curve.size());
        for (const auto& point : curve) {
            serialized.push_back({point.time, point.value, point.curve});
        }
        target->setTrackDelayAutomation(serialized);
    };
    m_undoManager.performAction("Track Delay Automation", [apply, before] { apply(before); }, [apply, points] { apply(points); });
    return true;
}
void AuraUnifiedEngine::set_spatial_mode(uint32_t tid, uint32_t mode) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == tid) {
            track->setSpatialMode(mode);
            return;
        }
    }
}
bool AuraUnifiedEngine::set_spatial_position(uint32_t tid, float x, float y, float z) {
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

bool AuraUnifiedEngine::set_hrtf_kernel(uint32_t tid, const std::vector<float>& left,
                                        const std::vector<float>& right) {
    if (left.empty() || left.size() != right.size() ||
        left.size() > ::Aura::DSP::Spatial::HolographicPanner::kMaxHrtfTaps) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->setHrtfKernel(left.data(), right.data(), static_cast<uint32_t>(left.size()));
    }
    return false;
}

bool AuraUnifiedEngine::clear_hrtf_kernel(uint32_t tid) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            track->clearHrtfKernel();
            return true;
        }
    }
    return false;
}
bool AuraUnifiedEngine::execute_auto_mixing() {
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
bool AuraUnifiedEngine::execute_auto_arrangement() {
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
    auto sections = Composition::NeuralArrangementKernel::AnalyzeStructure(
        regionStarts, projectLen, m_tempo.load(), m_sampleRate.load()
    );

    // 3. GENERATE MARKERS & NOTIFY UI
    for (const auto& sec : sections) {
        ::Aura::Core::EngineEvent ev{};
        ev.type = 0x50; // EVENT_TYPE_STRUCTURE_MARKER
        ev.timestamp = sec.startSample;
        ev.value = static_cast<float>(sec.endSample - sec.startSample);
        push_event(ev);
        
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "AI | ARRANGEMENT | SECTION_DETECTED");
    }
    return true;
}
