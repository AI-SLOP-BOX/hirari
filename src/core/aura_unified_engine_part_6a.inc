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
            // Distinguish a missing file from a file that exists but was not
            // decoded during project hydration; clients can surface a relink
            // or decoder error instead of silently showing an empty region.
            ss << "\"audio_loaded\":" << (region.audio ? "true" : "false") << ",";
            ss << "\"clip_gain\":" << jsonNumber(region.clipGain) << ",";
            ss << "\"fade_in_samples\":" << region.fadeInSamples << ",";
            ss << "\"fade_out_samples\":" << region.fadeOutSamples << ",";
            ss << "\"reverse\":" << (region.reverse ? "true" : "false");
            ss << ",\"warp_ratio\":" << jsonNumber(region.warpRatio);
            ss << ",\"pitch_semitones\":" << jsonNumber(region.pitchSemitones);
            ss << ",\"loop_count\":" << region.loopCount;
            ss << ",\"range_edits\":[";
            for (size_t ei = 0; ei < region.rangeEdits.size(); ++ei) {
                if (ei > 0) ss << ",";
                const auto& edit = region.rangeEdits[ei];
                ss << "{\"start\":" << edit.start << ",\"end\":" << edit.end
                   << ",\"gain\":" << jsonNumber(edit.gain)
                   << ",\"fade_in\":" << edit.fadeIn
                   << ",\"fade_out\":" << edit.fadeOut << "}";
            }
            ss << "]";
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
    thread_local std::array<float, 256> snapshot{};
    TelemetryData telemetry{};
    if (!copy_telemetry(get_active_telemetry_idx(), telemetry)) {
        return rust::Slice<const float>(snapshot.data(), 0);
    }
    const uint32_t count = std::min<uint32_t>(telemetry.count, snapshot.size());
    std::copy_n(telemetry.peaksL, count, snapshot.data());
    return rust::Slice<const float>(snapshot.data(), count);
}

rust::Slice<const float> AuraUnifiedEngine::get_track_peaks_r() const {
    thread_local std::array<float, 256> snapshot{};
    TelemetryData telemetry{};
    if (!copy_telemetry(get_active_telemetry_idx(), telemetry)) {
        return rust::Slice<const float>(snapshot.data(), 0);
    }
    const uint32_t count = std::min<uint32_t>(telemetry.count, snapshot.size());
    std::copy_n(telemetry.peaksR, count, snapshot.data());
    return rust::Slice<const float>(snapshot.data(), count);
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
