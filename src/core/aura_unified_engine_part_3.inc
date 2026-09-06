bool AuraUnifiedEngine::replace_region_audio(uint32_t tid, uint32_t rid, std::string_view path) {
    if (path.empty()) return false;
    auto audio = m_audioDecoderManager.importFile(std::string(path));
    if (!audio || audio->getNumSamples() == 0 || audio->getNumChannels() == 0) return false;
    std::shared_ptr<Track> target;
    Region before;
    Region after;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (!track || track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id == rid) {
                    target = track;
                    before = region;
                    break;
                }
            }
            break;
        }
        if (!target) return false;
        if (!target->replaceRegionAudio(rid, std::string(path), std::move(audio))) return false;
        for (const auto& region : target->getRegions()) {
            if (region.id == rid) {
                after = region;
                break;
            }
        }
        if (!after.audio) return false;
        m_waveformGeneration.fetch_add(1, std::memory_order_acq_rel);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    }

    const auto publish = [this, target, after, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(after);
        m_waveformGeneration.fetch_add(1, std::memory_order_acq_rel);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    const auto restore = [this, target, before, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(before);
        m_waveformGeneration.fetch_add(1, std::memory_order_acq_rel);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    m_undoManager.recordAppliedAction("Replace Region Audio", restore, publish);
    return true;
}
bool AuraUnifiedEngine::add_plugin(uint32_t tid, uint32_t pluginType) {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& t : m_tracks) {
            if (t->getId() == tid) {
                if (!t->addPlugin(pluginType)) break;
                target = t;
                if (m_context) {
                    // Plugin insertion changes both intrinsic latency and the
                    // published PDC graph. Re-prepare at the control boundary
                    // before recalculating compensation.
                    t->prepareToPlay(m_context->sampleRate, m_context->blockSize);
                }
                break;
            }
        }
    }
    if (!target) return false;

    const size_t pluginIndex = target->getPluginCount() - 1;
    const auto state = target->getPluginStates()[pluginIndex];
    const bool bypassed = target->getPluginBypass(static_cast<uint32_t>(pluginIndex));
    const auto undo = [target, pluginIndex]() {
        if (target->getPluginCount() > pluginIndex)
            (void)target->removePlugin(static_cast<uint32_t>(pluginIndex));
    };
    const auto redo = [target, pluginType, pluginIndex, state, bypassed]() {
        if (!target->addPlugin(pluginType)) return;
        const auto newIndex = static_cast<uint32_t>(target->getPluginCount() - 1);
        if (!state.empty()) (void)target->setPluginState(newIndex, state);
        (void)target->setPluginBypass(newIndex, bypassed);
        (void)pluginIndex; // The current Track API appends plugins by contract.
    };
    m_undoManager.recordAppliedAction("Add Plugin", undo, redo);
    syncStructuralChanges();
    return true;
}

std::vector<uint8_t> AuraUnifiedEngine::get_plugin_state(
    uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            const auto states = track->getPluginStates();
            return pluginIndex < states.size() ? states[pluginIndex] : std::vector<uint8_t>{};
        }
    }
    return {};
}

bool AuraUnifiedEngine::set_plugin_state(
    uint32_t tid, uint32_t pluginIndex, const std::vector<uint8_t>& state) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            return track->setPluginState(pluginIndex, state);
        }
    }
    return false;
}

bool AuraUnifiedEngine::remove_plugin(uint32_t tid, uint32_t pluginIndex) {
    std::shared_ptr<Track> target;
    Track::PluginSnapshot snapshot{};
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                if (track->removePlugin(pluginIndex, &snapshot)) target = track;
                break;
            }
        }
    }
    if (!target) return false;
    const auto undo = [target, pluginIndex, snapshot]() {
        (void)target->restorePlugin(pluginIndex, snapshot);
    };
    const auto redo = [target, pluginIndex]() {
        (void)target->removePlugin(pluginIndex);
    };
    m_undoManager.recordAppliedAction("Remove Plugin", undo, redo);
    syncStructuralChanges();
    return true;
}
bool AuraUnifiedEngine::add_sandboxed_plugin(uint32_t tid, std::string_view pluginPath) {
    if (pluginPath.empty() || pluginPath.size() > 4096 ||
        pluginPath.find('\0') != std::string_view::npos) {
        return false;
    }
    std::string normalizedPath(pluginPath);
    std::transform(normalizedPath.begin(), normalizedPath.end(), normalizedPath.begin(),
        [](unsigned char value) { return static_cast<char>(std::tolower(value)); });
    const auto hasSuffix = [&normalizedPath](std::string_view suffix) {
        return normalizedPath.size() >= suffix.size() &&
            normalizedPath.compare(normalizedPath.size() - suffix.size(), suffix.size(), suffix) == 0;
    };
    const bool supportedBundle = hasSuffix(".vst3") || hasSuffix(".component") ||
        hasSuffix(".clap") || normalizedPath.starts_with("builtin://");
    if (!supportedBundle) return false;
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& t : m_tracks) {
            if (t->getId() != tid) continue;
            if (!t->addSandboxedPlugin(std::string(pluginPath))) return false;
            target = t;
            break;
        }
    }
    if (!target) return false;

    const uint32_t pluginIndex = static_cast<uint32_t>(target->getPluginCount() - 1);
    const std::string path(pluginPath);
    const auto state = target->getPluginStates()[pluginIndex];
    const bool bypassed = target->getPluginBypass(pluginIndex);
    const auto undo = [target, pluginIndex]() {
        if (target->getPluginCount() > pluginIndex)
            (void)target->removePlugin(pluginIndex);
    };
    const auto redo = [target, path, pluginIndex, state, bypassed]() {
        if (!target->addSandboxedPlugin(path)) return;
        const auto newIndex = static_cast<uint32_t>(target->getPluginCount() - 1);
        if (!state.empty()) (void)target->setPluginState(newIndex, state);
        (void)target->setPluginBypass(newIndex, bypassed);
        (void)pluginIndex; // Sandbox insertion appends by the Track contract.
    };
    m_undoManager.recordAppliedAction("Add Sandboxed Plugin", undo, redo);
    syncStructuralChanges();
    return true;
}

bool AuraUnifiedEngine::process_sandboxed_plugin_block(uint32_t tid, uint32_t sandboxIndex,
                                                       float* left, float* right, uint32_t frames) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->processSandboxedPluginBlock(sandboxIndex, left, right, frames);
    }
    return false;
}

std::vector<uint8_t> AuraUnifiedEngine::process_sandboxed_plugin_midi_block(
    uint32_t tid, uint32_t sandboxIndex, uint32_t frames,
    const uint8_t* midiData, uint32_t midiSize) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->processSandboxedPluginMidiBlock(sandboxIndex, frames, midiData, midiSize);
    }
    return {};
}

uint32_t AuraUnifiedEngine::maintain_sandboxed_plugins(bool autoRestart) {
    std::lock_guard<std::mutex> maintenanceLock(m_sandboxMaintenanceMutex);
    const auto tracks = get_tracks_snapshot();
    const auto publishStatusEvent = [this](uint32_t type, uint32_t trackId,
                                            uint32_t pluginIndex, float value,
                                            const char* label) {
        ::Aura::Core::EngineEvent event{};
        event.timestamp = m_playhead.load(std::memory_order_relaxed);
        event.type = type;
        event.trackId = trackId;
        event.value = value;
        std::snprintf(event.label, sizeof(event.label), "%s #%u", label, pluginIndex);
        push_event(event);
    };
    uint32_t recovered = 0;
    for (const auto& track : tracks) {
        if (!track) continue;
        const uint32_t trackId = track->getId();
        const auto before = track->getSandboxStatuses();
        for (const auto& status : before) {
            const uint64_t key = (static_cast<uint64_t>(trackId) << 32) | status.pluginIndex;
            auto& observed = m_sandboxObserved[key];
            if ((!observed.alive && status.alive) ||
                (observed.failure != 0 && status.failure == 0 && status.alive)) {
                publishStatusEvent(0x61, trackId, status.pluginIndex, 1.0f,
                                   "PLUGIN_SANDBOX_RECOVERED");
            } else if (observed.alive && !status.alive) {
                publishStatusEvent(0x60, trackId, status.pluginIndex,
                                   static_cast<float>(status.failure),
                                   "PLUGIN_SANDBOX_FAILED");
            }
            if (observed.failure != status.failure && status.failure != 0) {
                publishStatusEvent(0x63, trackId, status.pluginIndex,
                                   static_cast<float>(status.failure),
                                   "PLUGIN_SANDBOX_QUARANTINED");
            }
            if (status.droppedOutputMidi > 0) {
                publishStatusEvent(0x62, trackId, status.pluginIndex,
                                   static_cast<float>(status.droppedOutputMidi),
                                   "PLUGIN_SANDBOX_MIDI_DROPPED");
            }
            observed.alive = status.alive;
            observed.failure = status.failure;
            observed.droppedOutputMidi = status.droppedOutputMidi;
        }
        recovered += track->maintainSandboxedPlugins(autoRestart);
        const auto after = track->getSandboxStatuses();
        for (const auto& status : after) {
            const uint64_t key = (static_cast<uint64_t>(trackId) << 32) | status.pluginIndex;
            auto& observed = m_sandboxObserved[key];
            if (!observed.alive && status.alive) {
                publishStatusEvent(0x61, trackId, status.pluginIndex, 1.0f,
                                   "PLUGIN_SANDBOX_RECOVERED");
            }
            observed.alive = status.alive;
            observed.failure = status.failure;
            observed.droppedOutputMidi = status.droppedOutputMidi;
        }
    }
    return recovered;
}

bool AuraUnifiedEngine::retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->retrySandboxedPlugin(sandboxIndex);
    }
    return false;
}

bool AuraUnifiedEngine::restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->restartSandboxedPlugin(sandboxIndex);
    }
    return false;
}

bool AuraUnifiedEngine::reset_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->resetSandboxedPlugin(sandboxIndex);
    }
    return false;
}

std::vector<AuraUnifiedEngine::SandboxStatus> AuraUnifiedEngine::get_sandbox_statuses() {
    const auto tracks = get_tracks_snapshot();
    std::vector<SandboxStatus> result;
    for (const auto& track : tracks) {
        if (!track) continue;
        for (const auto& status : track->getSandboxStatuses()) {
            result.push_back({track->getId(), status.pluginIndex, status.alive,
                              status.canRetry, status.failure, status.droppedOutputMidi,
                              status.mailboxOverruns, status.inputMidiTruncations,
                              status.recoveryMode});
        }
    }
    return result;
}

uint32_t AuraUnifiedEngine::take_watchdog_trips() {
    uint32_t trips = 0;
    for (const auto& track : get_tracks_snapshot()) {
        if (track) trips += track->takeWatchdogTrips();
    }
    return trips;
}

uint64_t AuraUnifiedEngine::non_finite_plugin_samples() const {
    uint64_t count = 0;
    for (const auto& track : get_tracks_snapshot()) {
        if (track) count += track->nonFiniteSampleCount();
    }
    return count;
}

uint8_t AuraUnifiedEngine::get_last_sandbox_failure(uint32_t trackId) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId) return track->lastSandboxFailure();
    }
    return 0;
}

std::string AuraUnifiedEngine::get_last_sandbox_failure_text(uint32_t trackId) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId) return track->lastSandboxFailureText();
    }
    return "track-not-found";
}

std::vector<std::string> AuraUnifiedEngine::get_sandbox_plugin_paths() const {
    const auto tracks = get_tracks_snapshot();
    std::vector<std::string> result;
    for (const auto& track : tracks) {
        if (!track) continue;
        const auto& paths = track->getSandboxedPluginPaths();
        result.insert(result.end(), paths.begin(), paths.end());
    }
    return result;
}

std::vector<uint8_t> AuraUnifiedEngine::get_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId)
            return track->getSandboxedPluginState(sandboxIndex);
    }
    return {};
}

bool AuraUnifiedEngine::set_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex, const std::vector<uint8_t>& state) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->setSandboxedPluginState(sandboxIndex, state);
    }
    return false;
}

uint8_t AuraUnifiedEngine::get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks)
        if (track && track->getId() == trackId) return track->sandboxedPluginStateError(sandboxIndex);
    return Aura::Core::Plugins::SandboxProtocol::kStateErrorUnavailable;
}

std::string AuraUnifiedEngine::get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks)
        if (track && track->getId() == trackId) return track->sandboxedPluginStateErrorText(sandboxIndex);
    return "state-unavailable";
}
bool AuraUnifiedEngine::set_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                             uint32_t parameterId, float value) {
    if (!std::isfinite(value)) return false;
    std::shared_ptr<Track> target;
    float before = 0.0f;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& t : m_tracks) {
        if (t->getId() == tid) {
            before = t->getPluginParameter(pluginIndex, parameterId);
            if (!t->setPluginParameter(pluginIndex, parameterId, value)) return false;
            target = t;
            break;
        }
    }
    }
    if (!target) return false;
    const auto undo = [target, pluginIndex, parameterId, before]() {
        (void)target->setPluginParameter(pluginIndex, parameterId, before);
    };
    const auto redo = [target, pluginIndex, parameterId, value]() {
        (void)target->setPluginParameter(pluginIndex, parameterId, value);
    };
    m_undoManager.recordAppliedAction("Set Plugin Parameter", undo, redo);
    return true;
}

bool AuraUnifiedEngine::set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                                          uint32_t parameterId, float value) {
    if (!std::isfinite(value)) return false;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->setPluginParameter(pluginIndex, parameterId, value);
    }
    return false;
}
float AuraUnifiedEngine::get_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                              uint32_t parameterId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->getPluginParameter(pluginIndex, parameterId);
    }
    return 0.0f;
}
uint32_t AuraUnifiedEngine::get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid) return track->getPluginParameterCount(pluginIndex);
    return 0;
}
bool AuraUnifiedEngine::has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid)
            return track->hasPluginNativeEditor(pluginIndex);
    return false;
}
uint64_t AuraUnifiedEngine::open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                      uintptr_t parent) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid)
            return track->openPluginNativeEditor(pluginIndex, parent);
    return 0;
}
bool AuraUnifiedEngine::close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                   uint64_t session) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid)
            return track->closePluginNativeEditor(pluginIndex, session);
    return false;
}
std::string AuraUnifiedEngine::get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                                         uint32_t parameterId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid)
            return track->getPluginParameterName(pluginIndex, parameterId);
    return {};
}
bool AuraUnifiedEngine::save_plugin_preset(uint32_t tid, uint32_t pluginIndex,
                                           std::string_view path) const {
    if (path.empty()) return false;
    for (const auto& track : get_tracks_snapshot()) {
        if (!track || track->getId() != tid) continue;
        const auto states = track->getPluginStates();
        const auto& pluginTypes = track->getPluginTypes();
        if (pluginIndex >= states.size() || pluginIndex >= pluginTypes.size() || states[pluginIndex].empty()) return false;
        const std::filesystem::path target(path);
        static std::atomic<uint64_t> presetSequence{0};
        const auto sequence = presetSequence.fetch_add(1, std::memory_order_relaxed);
        std::string temporaryName = target.string() + ".tmp-";
#if defined(_WIN32)
        temporaryName += "process-";
#else
        temporaryName += std::to_string(static_cast<unsigned long>(::getpid())) + "-";
#endif
        temporaryName += std::to_string(sequence);
        const std::filesystem::path temporary(temporaryName);
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        if (!file) return false;
        const uint32_t magic = 0x41555053u;
        const uint32_t version = 3;
        const uint32_t pluginType = pluginTypes[pluginIndex];
        constexpr size_t kMaxPresetStateBytes = 16u * 1024u * 1024u;
        if (states[pluginIndex].size() > kMaxPresetStateBytes ||
            states[pluginIndex].size() > std::numeric_limits<uint32_t>::max()) return false;
        const uint32_t size = static_cast<uint32_t>(states[pluginIndex].size());
        uint64_t checksum = 1469598103934665603ull;
        for (const uint8_t byte : states[pluginIndex]) {
            checksum ^= byte;
            checksum *= 1099511628211ull;
        }
        file.write(reinterpret_cast<const char*>(&magic), sizeof(magic));
        file.write(reinterpret_cast<const char*>(&version), sizeof(version));
        file.write(reinterpret_cast<const char*>(&pluginType), sizeof(pluginType));
        file.write(reinterpret_cast<const char*>(&size), sizeof(size));
        file.write(reinterpret_cast<const char*>(&checksum), sizeof(checksum));
        file.write(reinterpret_cast<const char*>(states[pluginIndex].data()), size);
        file.flush();
        if (!file) {
            file.close();
            std::error_code removeError;
            std::filesystem::remove(temporary, removeError);
            return false;
        }
        file.close();
#if !defined(_WIN32)
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::error_code removeError;
            std::filesystem::remove(temporary, removeError);
            return false;
        }
        ::close(fd);
#endif
        std::error_code renameError;
        std::filesystem::rename(temporary, target, renameError);
        if (renameError) {
            std::error_code removeError;
            std::filesystem::remove(temporary, removeError);
            return false;
        }
#if !defined(_WIN32)
        const auto parent = target.parent_path().empty() ? std::filesystem::path(".") : target.parent_path();
        const int dirFd = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY);
        if (dirFd >= 0) {
            const int syncResult = ::fsync(dirFd);
            ::close(dirFd);
            if (syncResult != 0) return false;
        }
#endif
        return true;
    }
    return false;
}
bool AuraUnifiedEngine::load_plugin_preset(uint32_t tid, uint32_t pluginIndex,
                                           std::string_view path) {
    if (path.empty()) return false;
    std::ifstream file(std::string(path), std::ios::binary);
    if (!file) return false;
    uint32_t magic = 0, version = 0, pluginType = 0, size = 0;
    uint64_t expectedChecksum = 0;
    file.read(reinterpret_cast<char*>(&magic), sizeof(magic));
    file.read(reinterpret_cast<char*>(&version), sizeof(version));
    file.read(reinterpret_cast<char*>(&pluginType), sizeof(pluginType));
    file.read(reinterpret_cast<char*>(&size), sizeof(size));
    file.read(reinterpret_cast<char*>(&expectedChecksum), sizeof(expectedChecksum));
    if (!file || magic != 0x41555053u || version != 3 || size == 0 || size > 16u * 1024u * 1024u)
        return false;
    std::vector<uint8_t> state(size);
    file.read(reinterpret_cast<char*>(state.data()), static_cast<std::streamsize>(size));
    if (!file) return false;
    uint64_t actualChecksum = 1469598103934665603ull;
    for (const uint8_t byte : state) {
        actualChecksum ^= byte;
        actualChecksum *= 1099511628211ull;
    }
    if (actualChecksum != expectedChecksum) return false;
    std::shared_ptr<Track> target;
    std::vector<uint8_t> before;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks) {
            if (!track || track->getId() != tid || pluginIndex >= track->getPluginTypes().size() ||
                track->getPluginTypes()[pluginIndex] != pluginType) continue;
            target = track;
            before = track->getPluginStates()[pluginIndex];
            if (!track->setPluginState(pluginIndex, state)) return false;
            break;
        }
    }
    if (!target) return false;
    const auto undo = [target, pluginIndex, before]() {
        (void)target->setPluginState(pluginIndex, before);
    };
    const auto redo = [target, pluginIndex, state]() {
        (void)target->setPluginState(pluginIndex, state);
    };
    m_undoManager.recordAppliedAction("Load Plugin Preset", undo, redo);
    return true;
}
bool AuraUnifiedEngine::set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& t : m_tracks) {
            if (t->getId() == tid) {
                before = t->getPluginBypass(pluginIndex);
                if (t->setPluginBypass(pluginIndex, bypassed)) target = t;
                break;
            }
        }
    }
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state after releasing m_tracksMutex; syncStructuralChanges
    // takes the same lock and must never be called while it is held.
    if (!target) return false;
    const auto undo = [target, pluginIndex, before]() {
        (void)target->setPluginBypass(pluginIndex, before);
    };
    const auto redo = [target, pluginIndex, bypassed]() {
        (void)target->setPluginBypass(pluginIndex, bypassed);
    };
    m_undoManager.recordAppliedAction("Set Plugin Bypass", undo, redo);
    syncStructuralChanges();
    return true;
}
bool AuraUnifiedEngine::get_plugin_bypass(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track->getId() == tid) return track->getPluginBypass(pluginIndex);
    }
    return false;
}
bool AuraUnifiedEngine::move_plugin(uint32_t tid, uint32_t fromIndex, uint32_t toIndex) {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                if (track->movePlugin(fromIndex, toIndex)) target = track;
                break;
            }
        }
    }
    if (!target) return false;
    const auto undo = [target, fromIndex, toIndex]() {
        (void)target->movePlugin(toIndex, fromIndex);
    };
    const auto redo = [target, fromIndex, toIndex]() {
        (void)target->movePlugin(fromIndex, toIndex);
    };
    m_undoManager.recordAppliedAction("Move Plugin", undo, redo);
    syncStructuralChanges();
    return true;
}
bool AuraUnifiedEngine::set_phase_invert(uint32_t tid, bool inverted) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                target = track;
                before = track->isPhaseInverted();
                break;
            }
        }
    }
    // A valid no-op is still a successful state application. Hydration and
    // idempotent command replay use the setter as an acknowledgement that the
    // target existed; rejecting the already-correct value made project restore
    // fail for every track whose phase state was false.
    if (!target) return false;
    if (before == inverted) return true;
    target->setPhaseInverted(inverted);
    const auto apply = [target](bool next) { target->setPhaseInverted(next); };
    m_undoManager.recordAppliedAction(
        "Phase Invert", [apply, before] { apply(before); },
        [apply, inverted] { apply(inverted); });
    return true;
}
bool AuraUnifiedEngine::set_track_delay_samples(uint32_t tid, uint32_t samples) {
    constexpr uint32_t kMaxTrackDelaySamples = 8192;
    if (samples > kMaxTrackDelaySamples) return false;
    std::shared_ptr<Track> target;
    uint32_t before = 0;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            target = track;
            before = track->getTrackDelaySamples();
            break;
        }
    }
    if (!target) return false;
    target->setTrackDelaySamples(samples);
    const auto apply = [target](uint32_t next) { target->setTrackDelaySamples(next); };
    m_undoManager.performAction(
        "Track Delay " + std::to_string(tid),
        [apply, before] { apply(before); },
        [apply, samples] { apply(samples); });
    // performAction executes redo, but keep the live value explicit because
    // coalescing and nested transactions are allowed to invoke callbacks.
    target->setTrackDelaySamples(samples);
    return true;
}
uint32_t AuraUnifiedEngine::get_track_delay_samples(uint32_t tid) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) return track->getTrackDelaySamples();
    }
    return 0;
}
bool AuraUnifiedEngine::set_track_armed(uint32_t tid, bool armed) {
    std::shared_ptr<Track> target;
    bool before = false;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track && track->getId() == tid) {
                target = track;
                before = track->isRecordArmed();
                break;
            }
        }
    }
    // Match the other scalar setters: an already-correct value is a valid
    // idempotent application, not a missing-target failure.
    if (!target) return false;
    if (before == armed) return true;
    target->setRecordArmed(armed);
    const auto apply = [target](bool next) { target->setRecordArmed(next); };
    m_undoManager.recordAppliedAction(
        "Record Arm", [apply, before] { apply(before); },
        [apply, armed] { apply(armed); });
    return true;
}
void AuraUnifiedEngine::set_macro_value(uint32_t macroIndex, float value) {
    if (!std::isfinite(value)) return;
    m_macroControlManager.setMacroValue(macroIndex, value);
}
bool AuraUnifiedEngine::bind_midi_cc_to_macro(uint8_t channel, uint8_t cc,
                                               uint32_t macroIndex, float minimum,
                                               float maximum, float curve,
                                               bool pickup) noexcept {
    if (macroIndex >= MacroControlManager::kMaxMacros || channel > 16 || cc > 127 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    auto* target = m_macroControlManager.targetPointer(macroIndex);
    if (target == nullptr) return false;
    m_midiLearnManager.addMappingAdvanced(
        cc, channel, target, minimum, maximum, curve, pickup);
    return true;
}
bool AuraUnifiedEngine::bind_midi_cc14_to_macro(uint8_t channel, uint16_t controller,
                                                 uint32_t macroIndex, float minimum,
                                                 float maximum, float curve,
                                                 bool pickup) noexcept {
    if (macroIndex >= MacroControlManager::kMaxMacros || channel > 16 || controller > 16383 ||
        !std::isfinite(minimum) || !std::isfinite(maximum) || minimum > maximum ||
        !std::isfinite(curve) || curve < -1.0f || curve > 1.0f) {
        return false;
    }
    auto* target = m_macroControlManager.targetPointer(macroIndex);
    if (target == nullptr) return false;
    m_midiLearnManager.addMapping14Bit(
        controller, channel, target, minimum, maximum, curve, pickup);
    return true;
}
void AuraUnifiedEngine::handle_midi_cc(uint8_t channel, uint8_t cc,
                                       uint8_t value) noexcept {
    m_midiLearnManager.handleMidiCC(channel, cc, value);
}

void AuraUnifiedEngine::handle_midi_cc14(uint8_t channel, uint16_t controller,
                                         uint16_t value) noexcept {
    m_midiLearnManager.handleMidiCC14(channel, controller, value);
}
void AuraUnifiedEngine::set_preview_synth_engine(uint32_t engine) {
    m_previewSynthEngine.store(std::min(engine, 2u), std::memory_order_release);
}
bool AuraUnifiedEngine::set_route(uint32_t sourceId, uint32_t destId, bool enabled) {
    if (sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Aura::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId) return false;
    auto& routing = m_routingEngine;
    const bool before = routing.hasConnection(sourceId, destId);
    if (before == enabled) return false;
    if (enabled) routing.addConnection(sourceId, destId);
    else routing.removeConnection(sourceId, destId);
    if (routing.hasConnection(sourceId, destId) != enabled) return false;
    const auto apply = [&routing, sourceId, destId](bool next) {
        if (next) routing.addConnection(sourceId, destId);
        else routing.removeConnection(sourceId, destId);
        routing.buildGraph();
    };
    m_undoManager.recordAppliedAction(
        "Audio Route", [apply, before] { apply(before); },
        [apply, enabled] { apply(enabled); });
    routing.buildGraph();
    syncStructuralChanges();
    return true;
}
bool AuraUnifiedEngine::set_route_gain(uint32_t sourceId, uint32_t destId, float gain, bool enabled) {
    if (sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Aura::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
        !std::isfinite(gain) || gain < 0.0f || gain > 2.0f || (enabled && gain <= 0.0f)) return false;
    auto& routing = m_routingEngine;
    const bool beforeEnabled = routing.hasConnection(sourceId, destId);
    const float beforeGain = routing.connectionGain(sourceId, destId);
    if (beforeEnabled == enabled && (!enabled || std::abs(beforeGain - gain) < 1.0e-6f)) {
        return false;
    }
    if (enabled) routing.addConnection(sourceId, destId, gain);
    else routing.removeConnection(sourceId, destId);
    if (routing.hasConnection(sourceId, destId) != enabled) return false;
    const auto apply = [&routing, sourceId, destId](float nextGain, bool nextEnabled) {
        if (nextEnabled) routing.addConnection(sourceId, destId, nextGain);
        else routing.removeConnection(sourceId, destId);
        routing.buildGraph();
    };
    m_undoManager.recordAppliedAction(
        "Audio Route Gain",
        [apply, beforeGain, beforeEnabled] { apply(beforeGain, beforeEnabled); },
        [apply, gain, enabled] { apply(gain, enabled); });
    routing.buildGraph();
    syncStructuralChanges();
    return true;
}
bool AuraUnifiedEngine::set_feedback_route(uint32_t sourceId, uint32_t destId,
                                           float gain, bool enabled) {
    if (sourceId >= Aura::Core::Engine::RoutingEngine::kMaxNodes ||
        destId >= Aura::Core::Engine::RoutingEngine::kMaxNodes || sourceId == destId ||
        !std::isfinite(gain)) return false;
    auto& routing = m_routingEngine;
    const float beforeGain = routing.feedbackConnectionGain(sourceId, destId);
    const bool changed = enabled
        ? routing.addFeedbackConnection(sourceId, destId, gain)
        : (beforeGain >= 0.0f ? (routing.removeFeedbackConnection(sourceId, destId), true) : false);
    if (!changed) return false;
    const auto apply = [&routing, sourceId, destId](float nextGain) {
        if (nextGain < 0.0f) routing.removeFeedbackConnection(sourceId, destId);
        else routing.addFeedbackConnection(sourceId, destId, nextGain);
    };
    m_undoManager.recordAppliedAction(
        "Feedback Route", [apply, beforeGain] { apply(beforeGain); },
        [apply, gain, enabled] { apply(enabled ? std::clamp(gain, 0.0f, 2.0f) : -1.0f); });
    return true;
}
bool AuraUnifiedEngine::set_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex, uint32_t tapPoint,
                                           bool enabled) {
    return set_sidechain_link_internal(sourceId, destId, pluginIndex, tapPoint,
                                       enabled, true);
}

bool AuraUnifiedEngine::set_sidechain_link_internal(uint32_t sourceId, uint32_t destId,
                                                    uint32_t pluginIndex, uint32_t tapPoint,
                                                    bool enabled, bool recordUndo) {
    if (sourceId == destId || tapPoint > 2) return false;
    auto& manager = m_sidechainManager;
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    const auto previous = std::find_if(
        m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
        [destId, pluginIndex](const SidechainRoute& route) {
            return route.destinationId == destId && route.pluginIndex == pluginIndex;
        });
    const bool hadPrevious = previous != m_sidechainRoutes.end();
    const uint32_t previousSource = hadPrevious ? previous->sourceId : 0;
    const uint32_t previousTapPoint = hadPrevious ? previous->tapPoint : 2;
    if (!enabled) {
        const bool removed = manager.removeLink(destId, pluginIndex);
        m_sidechainRoutes.erase(
            std::remove_if(m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
                           [destId, pluginIndex](const SidechainRoute& route) {
                               return route.destinationId == destId &&
                                      route.pluginIndex == pluginIndex;
                           }),
            m_sidechainRoutes.end());
        if (!removed) return false;
        if (recordUndo) {
            m_undoManager.recordAppliedAction(
                "Sidechain Disconnect",
                [this, previousSource, destId, pluginIndex, previousTapPoint]() {
                    set_sidechain_link_internal(previousSource, destId, pluginIndex,
                                                previousTapPoint, true, false);
                },
                [this, previousSource, destId, pluginIndex, previousTapPoint]() {
                    set_sidechain_link_internal(previousSource, destId, pluginIndex,
                                                previousTapPoint, false, false);
                });
        }
        return true;
    }

    // Sidechain dependencies are directed source -> destination. Adding a
    // link is rejected when the destination can already reach the source;
    // this prevents A->B->A cycles before the native graph is published.
    auto reaches = [this](uint32_t from, uint32_t target) {
        std::vector<uint32_t> pending{from};
        std::vector<uint32_t> visited;
        while (!pending.empty()) {
            const uint32_t node = pending.back();
            pending.pop_back();
            if (node == target) return true;
            if (std::find(visited.begin(), visited.end(), node) != visited.end()) continue;
            visited.push_back(node);
            for (const auto& route : m_sidechainRoutes) {
                if (route.sourceId == node) pending.push_back(route.destinationId);
            }
        }
        return false;
    };
    if (reaches(destId, sourceId)) return false;

    Track* source = nullptr;
    Track* destination = nullptr;
    for (const auto& track : m_tracks) {
        if (track->getId() == sourceId) source = track.get();
        if (track->getId() == destId) destination = track.get();
    }
    const uint32_t blockSize = m_context && m_context->blockSize > 0
        ? std::min(m_context->blockSize, Aura::Core::Engine::SidechainManager::kMaxBlockSize)
        : 512u;
    if (!source || !destination) return false;
    const bool sourcePrepared = source->canProcess(blockSize);
    const uint32_t sampleRate = static_cast<uint32_t>(std::max(
        1.0, m_sampleRate.load(std::memory_order_acquire)));
    const uint64_t sourceGeneration = std::max<uint64_t>(
        1, m_audioConfigGeneration.load(std::memory_order_acquire));
    if (!sourcePrepared) {
        const bool published = manager.registerSilentLink(
            destId, pluginIndex, sourceId, blockSize, sampleRate,
            sourceGeneration, 1.0f,
            static_cast<Aura::Core::Engine::SidechainTapPoint>(tapPoint));
        if (!published) return false;
    } else {
        auto& sourceBuffer = source->getWorkBuffer(blockSize);
    Aura::Core::Engine::SidechainLink link;
    link.sourceBufferL = sourceBuffer.getReadPointer(0);
    link.sourceBufferR = sourceBuffer.getNumChannels() > 1
        ? sourceBuffer.getReadPointer(1) : link.sourceBufferL;
    link.sourceTrackId = sourceId;
    link.sourceSampleRate = sampleRate;
    link.sourceGeneration = sourceGeneration;
    link.level = 1.0f;
    link.tapPoint = static_cast<Aura::Core::Engine::SidechainTapPoint>(tapPoint);
    // Registration copies one prepared block into SidechainManager-owned
    // storage.  Supplying the frame count is part of that ownership contract;
    // without it the manager correctly rejects the link as an unbounded/raw
    // pointer publication.
    link.sourceFrames = blockSize;
        if (!manager.registerLink(destId, pluginIndex, link)) return false;
    }
    auto existing = std::find_if(
        m_sidechainRoutes.begin(), m_sidechainRoutes.end(),
        [destId, pluginIndex](const SidechainRoute& route) {
            return route.destinationId == destId && route.pluginIndex == pluginIndex;
        });
    if (existing == m_sidechainRoutes.end()) {
        m_sidechainRoutes.push_back({sourceId, destId, pluginIndex, tapPoint});
    } else {
        existing->sourceId = sourceId;
        existing->tapPoint = tapPoint;
    }
    if (recordUndo) {
        m_undoManager.recordAppliedAction(
            "Sidechain Connect",
            [this, hadPrevious, previousSource, destId, pluginIndex, previousTapPoint]() {
                if (hadPrevious) {
                    set_sidechain_link_internal(previousSource, destId, pluginIndex,
                                                previousTapPoint, true, false);
                } else {
                    set_sidechain_link_internal(0, destId, pluginIndex, 0, false, false);
                }
            },
            [this, sourceId, destId, pluginIndex, tapPoint]() {
                set_sidechain_link_internal(sourceId, destId, pluginIndex, tapPoint,
                                            true, false);
            });
    }
    return true;
}

bool AuraUnifiedEngine::has_sidechain_link(uint32_t sourceId, uint32_t destId,
                                           uint32_t pluginIndex) const {
    return m_sidechainManager.hasLink(
        destId, pluginIndex, sourceId);
}
bool AuraUnifiedEngine::split_region(uint32_t tid, uint32_t rid, double splitBeat) {
    // The bridge passes an absolute sample position as a double for ABI
    // compatibility. Reject fractional positions instead of truncating them
    // and silently moving an edit by part of a sample.
    if (!std::isfinite(splitBeat) || splitBeat < 0.0 || std::floor(splitBeat) != splitBeat) return false;
    std::shared_ptr<Track> target;
    Region before;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
            if (track->getId() != tid) continue;
            for (const auto& region : track->getRegions()) {
                if (region.id != rid) continue;
                target = track;
                before = region;
                break;
            }
            break;
        }
    }
    if (!target || before.len == 0 || before.start > UINT64_MAX - before.len) return false;
    const double end = static_cast<double>(before.start) + static_cast<double>(before.len);
    if (splitBeat <= static_cast<double>(before.start) || splitBeat >= end) return false;
    const uint64_t splitOffset = static_cast<uint64_t>(splitBeat - static_cast<double>(before.start));
    if (splitOffset == 0 || splitOffset >= before.len || before.sourceOffset > UINT64_MAX - splitOffset)
        return false;

    Region left = before;
    Region right = before;
    left.len = splitOffset;
    right.start = before.start + splitOffset;
    right.len = before.len - splitOffset;
    right.sourceOffset += splitOffset;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        right.id = allocateRegionIdLocked();
    }
    if (right.id == 0) return false;

    const auto publish = [this, target, rid, left, right]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(left);
        target->addRegion(right);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
        m_peakCache.erase(right.id);
    };
    const uint32_t rightId = right.id;
    const auto restore = [this, target, rid, rightId, before]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->removeRegion(rightId);
        target->addRegion(before);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(rid);
    };
    m_undoManager.performAction("Split Region", restore, publish);
    return true;
}

uint32_t AuraUnifiedEngine::duplicate_region(uint32_t tid, uint32_t rid, uint64_t startSample) {
    std::shared_ptr<Track> target;
    Region duplicate;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (auto& track : m_tracks) {
        if (track->getId() != tid) continue;
        for (const auto& region : track->getRegions()) {
            if (region.id != rid || region.len == 0 || startSample > UINT64_MAX - region.len)
                continue;
            target = track;
            duplicate = region;
            duplicate.id = allocateRegionIdLocked();
            if (duplicate.id == 0) return 0;
            duplicate.start = startSample;
            break;
        }
        if (target) break;
        }
    }
    if (!target) return 0;

    // A duplicated clip is a new timeline instance. Preserve the source
    // offset, trim, fades, warp, pitch, loop and shared audio asset, but keep
    // one stable identity across undo/redo instead of allocating a new ID on
    // every redo.
    const uint32_t duplicateId = duplicate.id;
    const auto apply = [this, target, duplicate]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->addRegion(duplicate);
    };
    const auto remove = [this, target, duplicateId]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(duplicateId);
        std::lock_guard<std::mutex> cacheLock(m_cacheMutex);
        m_peakCache.erase(duplicateId);
    };
    m_undoManager.performAction(
        "Duplicate Region", remove, apply);
    return duplicateId;
}
