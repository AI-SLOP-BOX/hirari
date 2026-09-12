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
