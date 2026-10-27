bool HirariUnifiedEngine::replace_region_audio(uint32_t tid, uint32_t rid, std::string_view path) {
    if (path.empty()) return false;
    double sourceSampleRate = 0.0;
    auto audio = m_audioDecoderManager.importFile(std::string(path), &sourceSampleRate);
    if (!audio || audio->getNumSamples() == 0 || audio->getNumChannels() == 0) return false;
    const double timelineSampleRate = m_sampleRate.load(std::memory_order_acquire);
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
        if (!target->replaceRegionAudio(rid, std::string(path), std::move(audio),
                                        sourceSampleRate, timelineSampleRate)) return false;
        for (const auto& region : target->getRegions()) {
            if (region.id == rid) {
                after = region;
                break;
            }
        }
        if (!after.audio) return false;
        hirari_region_waveform_forget(this, rid);
    }

    const auto publish = [this, target, after, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(after);
        hirari_region_waveform_forget(this, rid);
    };
    const auto restore = [this, target, before, rid]() {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        target->removeRegion(rid);
        target->addRegion(before);
        hirari_region_waveform_forget(this, rid);
    };
    m_undoManager.recordAppliedAction("Replace Region Audio", restore, publish);
    return true;
}
bool HirariUnifiedEngine::add_plugin(uint32_t tid, uint32_t pluginType) {
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

std::vector<uint8_t> HirariUnifiedEngine::get_plugin_state(
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

bool HirariUnifiedEngine::set_plugin_state(
    uint32_t tid, uint32_t pluginIndex, const std::vector<uint8_t>& state) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid) {
            return track->setPluginState(pluginIndex, state);
        }
    }
    return false;
}

bool HirariUnifiedEngine::remove_plugin(uint32_t tid, uint32_t pluginIndex) {
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
bool HirariUnifiedEngine::add_sandboxed_plugin(uint32_t tid, std::string_view pluginPath) {
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

bool HirariUnifiedEngine::process_sandboxed_plugin_block(uint32_t tid, uint32_t sandboxIndex,
                                                       float* left, float* right, uint32_t frames) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->processSandboxedPluginBlock(sandboxIndex, left, right, frames);
    }
    return false;
}

std::vector<uint8_t> HirariUnifiedEngine::process_sandboxed_plugin_midi_block(
    uint32_t tid, uint32_t sandboxIndex, uint32_t frames,
    const uint8_t* midiData, uint32_t midiSize) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->processSandboxedPluginMidiBlock(sandboxIndex, frames, midiData, midiSize);
    }
    return {};
}

uint32_t HirariUnifiedEngine::maintain_sandboxed_plugins(bool autoRestart) {
    std::lock_guard<std::mutex> maintenanceLock(m_sandboxMaintenanceMutex);
    const auto tracks = get_tracks_snapshot();
    const auto publishStatusEvent = [this](uint32_t type, uint32_t trackId,
                                            uint32_t pluginIndex, float value,
                                            const char* label) {
        ::Hirari::Core::EngineEvent event{};
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

bool HirariUnifiedEngine::retry_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->retrySandboxedPlugin(sandboxIndex);
    }
    return false;
}

bool HirariUnifiedEngine::restart_sandboxed_plugin(uint32_t trackId, uint32_t sandboxIndex) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->restartSandboxedPlugin(sandboxIndex);
    }
    return false;
}

std::vector<HirariUnifiedEngine::SandboxStatus> HirariUnifiedEngine::get_sandbox_statuses() {
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

uint32_t HirariUnifiedEngine::take_watchdog_trips() {
    uint32_t trips = 0;
    for (const auto& track : get_tracks_snapshot()) {
        if (track) trips += track->takeWatchdogTrips();
    }
    return trips;
}

uint64_t HirariUnifiedEngine::non_finite_plugin_samples() const {
    uint64_t count = 0;
    for (const auto& track : get_tracks_snapshot()) {
        if (track) count += track->nonFiniteSampleCount();
    }
    return count;
}

uint8_t HirariUnifiedEngine::get_last_sandbox_failure(uint32_t trackId) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId) return track->lastSandboxFailure();
    }
    return 0;
}

std::string HirariUnifiedEngine::get_last_sandbox_failure_text(uint32_t trackId) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId) return track->lastSandboxFailureText();
    }
    return "track-not-found";
}

std::vector<std::string> HirariUnifiedEngine::get_sandbox_plugin_paths() const {
    const auto tracks = get_tracks_snapshot();
    std::vector<std::string> result;
    for (const auto& track : tracks) {
        if (!track) continue;
        const auto& paths = track->getSandboxedPluginPaths();
        result.insert(result.end(), paths.begin(), paths.end());
    }
    return result;
}

std::vector<uint8_t> HirariUnifiedEngine::get_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks) {
        if (track && track->getId() == trackId)
            return track->getSandboxedPluginState(sandboxIndex);
    }
    return {};
}

bool HirariUnifiedEngine::set_sandbox_plugin_state(
    uint32_t trackId, uint32_t sandboxIndex, const std::vector<uint8_t>& state) {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == trackId)
            return track->setSandboxedPluginState(sandboxIndex, state);
    }
    return false;
}

uint8_t HirariUnifiedEngine::get_sandbox_plugin_state_error(uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks)
        if (track && track->getId() == trackId) return track->sandboxedPluginStateError(sandboxIndex);
    return Hirari::Core::Plugins::SandboxProtocol::kStateErrorUnavailable;
}

std::string HirariUnifiedEngine::get_sandbox_plugin_state_error_text(uint32_t trackId, uint32_t sandboxIndex) const {
    const auto tracks = get_tracks_snapshot();
    for (const auto& track : tracks)
        if (track && track->getId() == trackId) return track->sandboxedPluginStateErrorText(sandboxIndex);
    return "state-unavailable";
}
bool HirariUnifiedEngine::set_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                             uint32_t parameterId, float value) {
    return hirari_plugin_edit_set_parameter(this, tid, pluginIndex, parameterId, value);
}

bool HirariUnifiedEngine::set_plugin_parameter_without_undo(uint32_t tid, uint32_t pluginIndex,
                                                          uint32_t parameterId, float value) {
    return hirari_plugin_edit_set_parameter_without_undo(
        this, tid, pluginIndex, parameterId, value);
}
float HirariUnifiedEngine::get_plugin_parameter(uint32_t tid, uint32_t pluginIndex,
                                              uint32_t parameterId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks) {
        if (track && track->getId() == tid)
            return track->getPluginParameter(pluginIndex, parameterId);
    }
    return 0.0f;
}
uint32_t HirariUnifiedEngine::get_plugin_parameter_count(uint32_t tid, uint32_t pluginIndex) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid) return track->getPluginParameterCount(pluginIndex);
    return 0;
}
bool HirariUnifiedEngine::has_plugin_native_editor(uint32_t tid, uint32_t pluginIndex) const {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks)
            if (track && track->getId() == tid) {
                target = track;
                break;
            }
    }
    // Plugin/editor calls can enter vendor UI code. Keep the Track alive but
    // release the project topology lock before crossing that boundary.
    return target && target->hasPluginNativeEditor(pluginIndex);
}
uint64_t HirariUnifiedEngine::open_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                      uintptr_t parent) const {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks)
            if (track && track->getId() == tid) {
                target = track;
                break;
            }
    }
    return target ? target->openPluginNativeEditor(pluginIndex, parent) : 0;
}
bool HirariUnifiedEngine::close_plugin_native_editor(uint32_t tid, uint32_t pluginIndex,
                                                   uint64_t session) const {
    std::shared_ptr<Track> target;
    {
        std::lock_guard<std::mutex> lock(m_tracksMutex);
        for (const auto& track : m_tracks)
            if (track && track->getId() == tid) {
                target = track;
                break;
            }
    }
    return target && target->closePluginNativeEditor(pluginIndex, session);
}
std::string HirariUnifiedEngine::get_plugin_parameter_name(uint32_t tid, uint32_t pluginIndex,
                                                         uint32_t parameterId) const {
    std::lock_guard<std::mutex> lock(m_tracksMutex);
    for (const auto& track : m_tracks)
        if (track && track->getId() == tid)
            return track->getPluginParameterName(pluginIndex, parameterId);
    return {};
}
bool HirariUnifiedEngine::save_plugin_preset(uint32_t tid, uint32_t pluginIndex,
                                           std::string_view path) const {
    if (path.empty()) return false;
    for (const auto& track : get_tracks_snapshot()) {
        if (!track || track->getId() != tid) continue;
        const auto states = track->getPluginStates();
        const auto& pluginTypes = track->getPluginTypes();
        if (pluginIndex >= states.size() || pluginIndex >= pluginTypes.size() || states[pluginIndex].empty()) return false;
        const uint32_t pluginType = pluginTypes[pluginIndex];
        return hirari_plugin_preset_file_save(
            reinterpret_cast<const uint8_t*>(path.data()), path.size(), pluginType,
            states[pluginIndex].data(), states[pluginIndex].size());
    }
    return false;
}
bool HirariUnifiedEngine::load_plugin_preset(uint32_t tid, uint32_t pluginIndex,
                                           std::string_view path) {
    void* rawPreset = hirari_plugin_preset_file_load(
        reinterpret_cast<const uint8_t*>(path.data()), path.size());
    if (!rawPreset) return false;
    std::unique_ptr<void, void (*)(void*)> preset(rawPreset, hirari_plugin_preset_file_free);
    const uint32_t pluginType = hirari_plugin_preset_file_type(preset.get());
    const uint8_t* stateData = hirari_plugin_preset_file_data(preset.get());
    const size_t stateSize = hirari_plugin_preset_file_size(preset.get());
    if (!stateData || stateSize == 0) return false;
    std::vector<uint8_t> state(stateData, stateData + stateSize);
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
bool HirariUnifiedEngine::set_plugin_bypass(uint32_t tid, uint32_t pluginIndex, bool bypassed) {
    if (!hirari_plugin_edit_set_bypass(this, tid, pluginIndex, bypassed)) return false;
    // Bypass changes alter the active latency path. Rebuild the published
    // routing/PDC state now that the edit host has released m_tracksMutex.
    syncStructuralChanges();
    return true;
}
