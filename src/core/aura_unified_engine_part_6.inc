bool AuraUnifiedEngine::set_track_eq(uint32_t trackId, float lowBoostDb, float lowCutDb,
                                     float highBoostDb, float highCutDb) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track->getId() == trackId) {
            track->setEq(lowBoostDb, lowCutDb, highBoostDb, highCutDb);
            return true;
        }
    }
    return false;
}

void AuraUnifiedEngine::undo() {
    m_undoManager.undo();
}

void AuraUnifiedEngine::redo() {
    m_undoManager.redo();
}

void AuraUnifiedEngine::begin_undo_transaction(std::string_view name) {
    m_undoManager.beginTransaction(std::string(name));
}

bool AuraUnifiedEngine::end_undo_transaction() {
    return m_undoManager.endTransaction();
}

bool AuraUnifiedEngine::abort_undo_transaction() {
    return m_undoManager.abortTransaction();
}

void AuraUnifiedEngine::clear_undo_history() {
    m_undoManager.clear();
}

uint32_t AuraUnifiedEngine::get_undo_count() const {
    return static_cast<uint32_t>(std::min<std::size_t>(
        m_undoManager.getUndoCount(), UINT32_MAX));
}

uint32_t AuraUnifiedEngine::get_redo_count() const {
    return static_cast<uint32_t>(std::min<std::size_t>(
        m_undoManager.getRedoCount(), UINT32_MAX));
}

rust::String AuraUnifiedEngine::get_project_layout_json() const {
    if (m_ecoModeLogTriggered.exchange(false, std::memory_order_acq_rel)) {
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "SYSTEM | GOVERNOR | ECO_MODE_ACTIVE");
    }
    if (m_videoSyncLogTriggered.exchange(false, std::memory_order_acq_rel)) {
        ::Aura::Core::Diagnostics::LogBuffer::post(0, 0, "SYSTEM | VIDEO | SYNC_LOCKED");
    }

    std::stringstream ss;
    const auto jsonString = [](const std::string& value) {
        std::string escaped;
        escaped.reserve(value.size() + 2);
        for (unsigned char c : value) {
            switch (c) {
                case '"': escaped += "\\\""; break;
                case '\\': escaped += "\\\\"; break;
                case '\n': escaped += "\\n"; break;
                case '\r': escaped += "\\r"; break;
                case '\t': escaped += "\\t"; break;
                default: if (c < 0x20) escaped += "?"; else escaped += static_cast<char>(c);
            }
        }
        return escaped;
    };
    const auto jsonNumber = [](double value) {
        return std::isfinite(value) ? std::to_string(value) : std::string("0");
    };
    const auto jsonHex = [](const std::vector<uint8_t>& bytes) {
        static constexpr char digits[] = "0123456789abcdef";
        std::string encoded;
        encoded.reserve(bytes.size() * 2);
        for (uint8_t byte : bytes) {
            encoded.push_back(digits[byte >> 4]);
            encoded.push_back(digits[byte & 0x0f]);
        }
        return encoded;
    };
    ss << "[";
    std::vector<Aura::Core::Engine::RoutingEngine::FeedbackConnection> feedbackConnections;
    m_routingEngine.copyFeedbackConnections(feedbackConnections);
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track) {
            // A callback may have queued write-automation points since the
            // previous snapshot. Drain them on this control-side boundary so
            // UI, CLI generation checks, history diffs, and project saves all
            // observe the same automation state.
            track->flushRecordedAutomation();
            track->reclaimEffectChainRetiredMemory();
        }
    }
    size_t serializedTrackCount = 0;
    for (size_t i = 0; i < m_tracks.size(); ++i) {
        const auto& track = m_tracks[i];
        const bool isEmptyBootstrap = track->getId() == 0 &&
            track->getName() == "Audio 1" &&
            track->getPluginCount() == 0 && track->getRegions().empty() &&
            m_tracks.size() > 1;
        if (isEmptyBootstrap) continue;
        if (serializedTrackCount++ > 0) ss << ",";
        ss << "{";
        ss << "\"id\":" << track->getId() << ",";
        ss << "\"name\":\"" << jsonString(track->getName()) << "\",";
        
        std::string typeStr = "Audio";
        if (track->getType() == Track::Type::Midi) typeStr = "Midi";
        else if (track->getType() == Track::Type::Instrument) typeStr = "Instrument";
        else if (track->getType() == Track::Type::Bus) typeStr = "Bus";
        else if (track->getType() == Track::Type::Vocal) typeStr = "Vocal";
        
        ss << "\"type\":\"" << typeStr << "\",";
        ss << "\"volume\":" << jsonNumber(track->getVolume()) << ",";
        ss << "\"pan\":" << jsonNumber(track->getPan()) << ",";
        ss << "\"mute\":" << (track->isMuted() ? "true" : "false") << ",";
        ss << "\"solo\":" << (track->isSolo() ? "true" : "false") << ",";
        ss << "\"record_armed\":" << (track->isRecordArmed() ? "true" : "false") << ",";
        ss << "\"phase_invert\":" << (track->isPhaseInverted() ? "true" : "false") << ",";
        ss << "\"track_delay_samples\":" << track->getTrackDelaySamples() << ",";
        ss << "\"frozen\":" << (track->isFrozen() ? "true" : "false") << ",";
        ss << "\"frozen_total_samples\":" << track->frozenTotalSamples() << ",";
        ss << "\"frozen_sample_rate\":" << track->frozenSampleRate() << ",";
        ss << "\"frozen_path\":\"" << jsonString(track->frozenPath()) << "\",";
        const auto writeAutomationJson = [&ss, &jsonNumber](const char* key, const auto& points) {
            ss << "\"" << key << "\":[";
            for (size_t index = 0; index < points.size(); ++index) {
                if (index > 0) ss << ",";
                const auto& point = points[index];
                ss << "{\"time\":" << jsonNumber(point.time)
                   << ",\"value\":" << jsonNumber(point.value)
                   << ",\"curve\":" << jsonNumber(point.curve) << "}";
            }
            ss << "],";
        };
        writeAutomationJson("volume_automation", track->getVolumeAutomationPoints());
        writeAutomationJson("pan_automation", track->getPanAutomationPoints());
        writeAutomationJson("track_delay_automation", track->getTrackDelayAutomationPoints());
        ss << "\"plugin_types\":[";
        const auto& pluginTypes = track->getPluginTypes();
        for (size_t index = 0; index < pluginTypes.size(); ++index) {
            if (index > 0) ss << ",";
            ss << pluginTypes[index];
        }
        ss << "],\"plugin_bypass\":[";
        for (size_t index = 0; index < pluginTypes.size(); ++index) {
            if (index > 0) ss << ",";
            ss << (track->getPluginBypass(static_cast<uint32_t>(index)) ? "true" : "false");
        }
        ss << "],\"plugin_parameter_values\":[";
        for (size_t pluginIndex = 0; pluginIndex < pluginTypes.size(); ++pluginIndex) {
            if (pluginIndex > 0) ss << ",";
            ss << "[";
            const uint32_t parameterCount = track->getPluginParameterCount(static_cast<uint32_t>(pluginIndex));
            for (uint32_t parameterIndex = 0; parameterIndex < parameterCount; ++parameterIndex) {
                if (parameterIndex > 0) ss << ",";
                const float value = track->getPluginParameter(static_cast<uint32_t>(pluginIndex), parameterIndex);
                ss << (std::isfinite(value) ? std::to_string(value) : "0.0");
            }
            ss << "]";
        }
        ss << "],\"plugin_state_sizes\":[";
        auto pluginStates = track->getPluginStates();
        // Keep the serialized arrays structurally aligned even when a legacy
        // processor does not expose a state serializer. The parameter vector
        // is already part of the control-plane snapshot, so this marker is
        // deterministic and is not read or allocated from the audio thread.
        while (pluginStates.size() < pluginTypes.size()) {
            const auto pluginIndex = static_cast<uint32_t>(pluginStates.size());
            std::vector<uint8_t> marker(sizeof(float));
            const float value = track->getPluginParameter(pluginIndex, 0);
            std::memcpy(marker.data(), &value, sizeof(float));
            pluginStates.push_back(std::move(marker));
        }
        for (size_t index = 0; index < pluginStates.size(); ++index) {
            if (index > 0) ss << ",";
            ss << pluginStates[index].size();
        }
        ss << "],\"plugin_state_hashes\":[";
        for (size_t index = 0; index < pluginStates.size(); ++index) {
            if (index > 0) ss << ",";
            uint64_t hash = 1469598103934665603ull;
            for (const uint8_t byte : pluginStates[index]) {
                hash ^= byte;
                hash *= 1099511628211ull;
            }
            ss << hash;
        }
        ss << "],\"plugin_state_hex\":[";
        for (size_t index = 0; index < pluginStates.size(); ++index) {
            if (index > 0) ss << ",";
            ss << "\"" << jsonHex(pluginStates[index]) << "\"";
        }
        ss << "],\"sandbox_plugin_paths\":[";
        const auto& sandboxPaths = track->getSandboxedPluginPaths();
        for (size_t index = 0; index < sandboxPaths.size(); ++index) {
            if (index > 0) ss << ",";
            ss << "\"" << jsonString(sandboxPaths[index]) << "\"";
        }
        ss << "],\"sandbox_plugin_state_hex\":[";
        const auto sandboxStates = track->getSandboxedPluginStates();
        for (size_t index = 0; index < sandboxStates.size(); ++index) {
            if (index > 0) ss << ",";
            ss << "\"" << jsonHex(sandboxStates[index]) << "\"";
        }
        ss << "],";

        ss << "\"sidechain_routes\":[";
        bool firstSidechain = true;
        for (const auto& route : m_sidechainRoutes) {
            if (route.destinationId != track->getId()) continue;
            if (!firstSidechain) ss << ",";
            firstSidechain = false;
            ss << "{\"source_id\":" << route.sourceId
               << ",\"destination_id\":" << route.destinationId
               << ",\"plugin_index\":" << route.pluginIndex
               << ",\"tap_point\":" << route.tapPoint << "}";
        }
        ss << "],";

        ss << "\"feedback_routes\":[";
        bool firstFeedback = true;
        for (const auto& route : feedbackConnections) {
            if (route.sourceId != track->getId()) continue;
            if (!firstFeedback) ss << ",";
            firstFeedback = false;
            ss << "{\"source_id\":" << route.sourceId
               << ",\"destination_id\":" << route.destId
               << ",\"gain\":" << jsonNumber(route.gain) << "}";
        }
        ss << "],";
        
        ss << "\"regions\":[";
        const auto& regions = track->getRegions();
        for (size_t r = 0; r < regions.size(); ++r) {
            const auto& region = regions[r];
            if (r > 0) ss << ",";
            ss << "{";
            ss << "\"id\":" << region.id << ",";
            ss << "\"name\":\"" << jsonString(region.name) << "\",";
            ss << "\"start\":" << region.start << ",";
            ss << "\"len\":" << region.len << ",";
            ss << "\"source_offset\":" << region.sourceOffset << ",";
            ss << "\"base_start\":" << region.baseStart << ",";
            ss << "\"base_source_offset\":" << region.baseSourceOffset << ",";
            ss << "\"base_length\":" << region.baseLength << ",";
            ss << "\"muted\":" << (region.muted ? "true" : "false") << ",";
            std::error_code pathError;
            const bool missing = region.path.empty() ||
                !std::filesystem::exists(std::filesystem::path(region.path), pathError);
            ss << "\"path\":\"" << jsonString(region.path) << "\",";
            ss << "\"missing\":" << (missing ? "true" : "false") << ",";
            ss << "\"clip_gain\":" << jsonNumber(region.clipGain) << ",";
            ss << "\"fade_in_samples\":" << region.fadeInSamples << ",";
            ss << "\"fade_out_samples\":" << region.fadeOutSamples << ",";
            ss << "\"reverse\":" << (region.reverse ? "true" : "false");
            ss << ",\"warp_ratio\":" << jsonNumber(region.warpRatio);
            ss << ",\"pitch_semitones\":" << jsonNumber(region.pitchSemitones);
            ss << ",\"loop_count\":" << region.loopCount;
            ss << ",\"audio_note_segments\":[";
            for (size_t si = 0; si < region.audioNoteSegments.size(); ++si) {
                if (si > 0) ss << ",";
                const auto& segment = region.audioNoteSegments[si];
                ss << "{\"start\":" << jsonNumber(segment.startSeconds)
                   << ",\"end\":" << jsonNumber(segment.endSeconds)
                   << ",\"detected_pitch_cents\":" << jsonNumber(segment.detectedPitchCents)
                   << ",\"pitch_offset_cents\":" << jsonNumber(segment.pitchOffsetCents)
                   << ",\"formant_offset_cents\":" << jsonNumber(segment.formantOffsetCents)
                   << ",\"anchors\":[";
                for (size_t ai = 0; ai < segment.anchors.size(); ++ai) {
                    if (ai > 0) ss << ",";
                    const auto& anchor = segment.anchors[ai];
                    ss << "{\"position\":" << jsonNumber(anchor.positionSeconds)
                       << ",\"pitch_cents\":" << jsonNumber(anchor.pitchCents)
                       << ",\"formant_cents\":" << jsonNumber(anchor.formantCents) << "}";
                }
                ss << "]}";
            }
            ss << "]";
            ss << "}";
        }
        ss << "]";
        ss << "}";
    }
    ss << "]";
    return rust::String(ss.str());
}

rust::String AuraUnifiedEngine::get_routing_snapshot_json() const {
    std::vector<Aura::Core::Engine::RoutingEngine::Connection> connections;
    m_routingEngine.copyConnections(connections);
    std::ostringstream stream;
    stream << "[";
    for (size_t index = 0; index < connections.size(); ++index) {
        if (index > 0) stream << ",";
        const auto& connection = connections[index];
        stream << "{\"source_id\":" << connection.sourceId
               << ",\"destination_id\":" << connection.destId
               << ",\"gain\":" << (std::isfinite(connection.gain) ? connection.gain : 0.0f)
               << "}";
    }
    stream << "]";
    return rust::String(stream.str());
}

uint64_t AuraUnifiedEngine::get_project_generation() const {
    const auto layout = get_project_layout_json();
    uint64_t generation = 0xcbf29ce484222325ull;
    for (const auto byte : layout) {
        generation ^= static_cast<uint8_t>(byte);
        generation *= 0x100000001b3ull;
    }
    return generation;
}

bool AuraUnifiedEngine::push_command(CommandType type, uint32_t tid, float val, uint64_t ts,
                                     uint64_t expectedProjectGeneration,
                                     uint64_t expectedAudioGeneration) {
    const auto setError = [this](CommandPushError error) {
        m_lastCommandError.store(static_cast<uint8_t>(error), std::memory_order_release);
    };
    // A bounded integer is not a valid target by itself.  Reject commands for
    // tracks that are no longer present instead of letting the legacy
    // get_track() fallback silently redirect an old UI command to the first
    // current track after project reload.
    const auto tracks = get_tracks_snapshot();
    const bool targetExists = std::any_of(
        tracks.begin(), tracks.end(), [tid](const auto& track) {
            return track != nullptr && track->getId() == tid;
        });
    if (!targetExists) {
        setError(CommandPushError::InvalidTarget);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        return false;
    }
    const uint64_t projectGeneration = get_project_generation();
    if (projectGeneration != expectedProjectGeneration) {
        setError(CommandPushError::StaleProjectGeneration);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        return false;
    }
    if (get_audio_config_generation() != expectedAudioGeneration) {
        setError(CommandPushError::StaleAudioGeneration);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        return false;
    }
    // LOCK-FREE Command Injection. State mutation is deferred to the audio
    // block consumer so the UI thread never races Track parameters.
    Command cmd{type, tid, val, ts, expectedAudioGeneration};
    // A full queue is a real command failure, not a best-effort enqueue. Do
    // not report success to the UI/CLI when the audio thread cannot observe
    // the command; otherwise the caller's undo/generation state diverges
    // from the engine.
    if (!m_commandQueue.push(cmd)) {
        setError(CommandPushError::QueueFull);
        m_forensicJournaler.log(
            static_cast<uint32_t>(type), tid, val, ts);
        ::Aura::Core::Diagnostics::LogBuffer::post(
            1, tid, "COMMAND_QUEUE_FULL");
        return false;
    }
    setError(CommandPushError::None);

    // INDUSTRIAL: Real-time Journaling
    m_forensicJournaler.log(static_cast<uint32_t>(type), tid, val, ts);
    return true;
}

Track& AuraUnifiedEngine::get_track(uint32_t id) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& t : m_tracks) {
        if (t->getId() == id) return *t;
    }
    // Never redirect an unknown/stale identifier to the first live track.
    // Callers that need mutation must validate the ID before reaching this
    // compatibility reference; returning a real track here would silently
    // edit the wrong project object after reload.
    static Track fallbackTrack{0, "Fallback Track", Track::Audio};
    return fallbackTrack;
}

uint32_t AuraUnifiedEngine::get_track_count() const { 
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    return static_cast<uint32_t>(m_tracks.size());
}

std::vector<std::shared_ptr<Track>> AuraUnifiedEngine::get_tracks_snapshot() const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    return m_tracks;
}

bool AuraUnifiedEngine::freeze_track(uint32_t tid, uint64_t totalSamples, uint32_t sampleRate) {
    if (totalSamples == 0 || sampleRate == 0 ||
        totalSamples > TrackFreezeManager::kMaxFreezeSamples) return false;
    // A freeze render mutates processor state while producing its immutable
    // snapshot, so it must never overlap the live callback or another graph
    // hydration.  The manager itself remains allocation/control-thread only.
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    return TrackFreezeManager::freezeTrack(std::move(target), totalSamples, sampleRate);
}

bool AuraUnifiedEngine::freeze_track_to_project_end(uint32_t tid, uint32_t sampleRate) {
    if (sampleRate == 0) return false;
    std::shared_ptr<Track> target;
    uint64_t projectEnd = 0;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
        for (const auto& region : target->getRegions()) {
            if (region.len == 0 || region.start > UINT64_MAX - region.len) return false;
            projectEnd = std::max(projectEnd, region.start + region.len);
        }
    }
    const uint64_t fallback = std::max<uint32_t>(1u, get_block_size()) * 2ull;
    const uint64_t totalSamples = std::max(projectEnd, fallback);
    if (totalSamples > TrackFreezeManager::kMaxFreezeSamples) return false;
    return freeze_track(tid, totalSamples, sampleRate);
}

bool AuraUnifiedEngine::freeze_track_to_file(uint32_t tid, std::string_view path,
                                             uint64_t totalSamples, uint32_t sampleRate) {
    if (path.empty() || totalSamples == 0 || sampleRate == 0 ||
        totalSamples > TrackFreezeManager::kMaxFreezeSamples) return false;
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    std::error_code pathError;
    const auto absolutePath = std::filesystem::absolute(std::filesystem::path(path), pathError);
    if (pathError || absolutePath.empty()) return false;
    return TrackFreezeManager::freezeTrackToFile(
        std::move(target), absolutePath, totalSamples, sampleRate);
}

bool AuraUnifiedEngine::set_track_freeze_cache_path(uint32_t tid, std::string_view path) {
    if (path.empty()) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
        [tid](const auto& track) { return track && track->getId() == tid; });
    return it != m_tracks.end() && (*it)->setFrozenCachePath(path);
}

bool AuraUnifiedEngine::restore_track_freeze_from_file(uint32_t tid, std::string_view path,
                                                       uint64_t totalSamples, uint32_t sampleRate) {
    if (path.empty() || totalSamples == 0 || sampleRate == 0) return false;
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    return TrackFreezeManager::restoreTrackFromFile(
        std::move(target), std::filesystem::path(path), totalSamples, sampleRate);
}

bool AuraUnifiedEngine::unfreeze_track(uint32_t tid) {
    waitForAudioCallbacks();
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
            [tid](const auto& track) { return track && track->getId() == tid; });
        if (it == m_tracks.end()) return false;
        target = *it;
    }
    TrackFreezeManager::unfreezeTrack(std::move(target));
    return true;
}

bool AuraUnifiedEngine::is_track_frozen(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const auto it = std::find_if(m_tracks.begin(), m_tracks.end(),
        [tid](const auto& track) { return track && track->getId() == tid; });
    return it != m_tracks.end() && (*it)->isFrozen();
}

rust::Slice<const float> AuraUnifiedEngine::get_track_peaks_l() const {
    const auto& td = m_telemetryBuffers[m_activeTelemetryIdx.load(std::memory_order_acquire)];
    return rust::Slice<const float>(td.peaksL, td.count);
}

rust::Slice<const float> AuraUnifiedEngine::get_track_peaks_r() const {
    const auto& td = m_telemetryBuffers[m_activeTelemetryIdx.load(std::memory_order_acquire)];
    return rust::Slice<const float>(td.peaksR, td.count);
}

AuraUnifiedEngine::MeterData AuraUnifiedEngine::getMasterMeterData() const {
    if (m_analyzer) {
        auto d = m_analyzer->getLatestData();
        MeterData out{};
        out.peakL = d.peakL;
        out.peakR = d.peakR;
        out.truePeakL = d.truePeakL;
        out.truePeakR = d.truePeakR;
        out.rmsL = d.rmsL;
        out.rmsR = d.rmsR;
        out.lufsShortTerm = d.lufsShortTerm;
        out.lufsIntegrated = d.lufsIntegrated;
        out.correlation = d.correlation;
        out.balance = d.balance;
        return out;
    }
    return {};
}

double AuraUnifiedEngine::get_playhead_beats() const {
    return samples_to_beats(m_playhead.load(std::memory_order_relaxed));
}

double AuraUnifiedEngine::samples_to_beats(uint64_t samples) const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return m_tempoMap.samplesToBeats(samples, sampleRate);
}

uint64_t AuraUnifiedEngine::beats_to_samples(double beats) const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    return m_tempoMap.beatsToSamples(beats, sampleRate);
}

std::vector<double> AuraUnifiedEngine::get_tempo_events() const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const auto events = m_tempoMap.getEvents();
    std::vector<double> packed;
    packed.reserve(events.size() * 3);
    for (const auto& event : events) {
        packed.push_back(m_tempoMap.samplesToBeats(
            event.samplePos, sampleRate));
        packed.push_back(event.bpm);
        packed.push_back(event.ramp ? 1.0 : 0.0);
    }
    return packed;
}

std::vector<double> AuraUnifiedEngine::get_time_signature_events() const {
    const double sampleRate = m_sampleRate.load(std::memory_order_relaxed);
    const auto events = m_tempoMap.getTimeSignatures();
    std::vector<double> packed;
    packed.reserve(events.size() * 3);
    for (const auto& event : events) {
        packed.push_back(m_tempoMap.samplesToBeats(event.samplePos, sampleRate));
        packed.push_back(static_cast<double>(event.numerator));
        packed.push_back(static_cast<double>(event.denominator));
    }
    return packed;
}

void AuraUnifiedEngine::clear_time_signature_events() {
    m_tempoMap.clearTimeSignatures();
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
