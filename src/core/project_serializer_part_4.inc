    static ProjectState load(const std::string& path) {
        ProjectState state{};
        std::ifstream input(path, std::ios::binary | std::ios::ate);
        if (!input.is_open()) return state;
        const auto fileSize = input.tellg();
        if (fileSize < static_cast<std::streamoff>(sizeof(uint32_t) * 2u)) return {};
        if (static_cast<uint64_t>(fileSize) > kMaxProjectBytes) return {};
        input.seekg(0, std::ios::beg);
        std::vector<uint8_t> bytes(static_cast<size_t>(fileSize));
        input.read(reinterpret_cast<char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
        if (!input) return {};
        if (bytes.size() < sizeof(uint32_t) * 2u) return {};
        uint32_t magic = 0;
        uint32_t version = 0;
        std::memcpy(&magic, bytes.data(), sizeof(magic));
        std::memcpy(&version, bytes.data() + sizeof(magic), sizeof(version));
        if (magic != 0x41555241u || version == 0 || version > 36) return {};
        size_t payloadSize = bytes.size();
        if (version >= 27) {
            if (bytes.size() < sizeof(uint32_t) * 3u) return {};
            payloadSize -= sizeof(uint32_t);
            uint32_t storedChecksum = 0;
            std::memcpy(&storedChecksum, bytes.data() + payloadSize, sizeof(storedChecksum));
            if (calculateCRC32(bytes.data(), payloadSize) != storedChecksum) return {};
        }
        std::string payload(reinterpret_cast<const char*>(bytes.data()), payloadSize);
        std::istringstream file(payload, std::ios::binary | std::ios::in);
        // Consume the header that was already inspected above; the existing
        // parser expects the stream cursor immediately after magic/version.
        (void)readLE<uint32_t>(file);
        (void)readLE<uint32_t>(file);
        state.version = version;
        state.sampleRate = readLE<uint32_t>(file); state.bpm = readLE<double>(file);
        if (version >= 14) {
            state.rootNote = readLE<int32_t>(file);
            state.scaleType = readLE<int32_t>(file);
        }
        if (!file || state.sampleRate == 0 || !std::isfinite(state.bpm) ||
            state.bpm < 20.0 || state.bpm > 300.0 || state.rootNote < 0 ||
            state.rootNote > 11 || state.scaleType < 0 || state.scaleType > 32) return {};
        size_t totalStringBytes = 0;
        size_t totalPluginDataBytes = 0;
        size_t totalPluginStateBytes = 0;
        uint64_t totalAutomationPoints = 0;
        uint64_t totalPluginStateEntries = 0;
        const auto addBudget = [](size_t& total, size_t amount, size_t limit) -> bool {
            if (amount > limit || total > limit - amount) return false;
            total += amount;
            return true;
        };
        const auto addCount = [](uint64_t& total, uint64_t amount, uint64_t limit) -> bool {
            if (amount > limit || total > limit - amount) return false;
            total += amount;
            return true;
        };
        auto readString = [&file, &totalStringBytes, &addBudget](std::string& out) -> bool {
            uint32_t len = readLE<uint32_t>(file);
            if (!file || len > 16u * 1024u * 1024u) return false;
            if (!addBudget(totalStringBytes, len, kMaxStringBytes)) return false;
            out.resize(len); file.read(out.data(), len); return static_cast<bool>(file);
        };
        uint32_t trackCount = readLE<uint32_t>(file);
        if (!file || trackCount > 100000) return {};
        state.tracks.resize(trackCount);
        std::unordered_set<uint32_t> trackIds;
        for (auto& t : state.tracks) {
            t.id = readLE<uint32_t>(file);
            // Track zero is the native engine's first valid track ID. Older
            // loads rejected it even though the template/bootstrap path
            // legitimately serializes the first track as ID 0, making a
            // freshly saved template impossible to reopen. Uniqueness and
            // the reserved uint32 max sentinel are enforced by the hydrate
            // layer; zero itself is valid.
            if (!file || !trackIds.insert(t.id).second) return {};
            if (version >= 12) t.type = readLE<uint32_t>(file);
            t.volume = readLE<float>(file); t.pan = readLE<float>(file);
            t.pan3d_x = readLE<float>(file); t.pan3d_y = readLE<float>(file); t.pan3d_z = readLE<float>(file);
            if (version >= 12) {
                t.muted = readLE<uint8_t>(file) != 0;
                t.solo = readLE<uint8_t>(file) != 0;
                if (version >= 15) t.phaseInvert = readLE<uint8_t>(file) != 0;
                if (version >= 23) t.recordArmed = readLE<uint8_t>(file) != 0;
                if (version >= 29) t.trackDelaySamples = readLE<uint32_t>(file);
            }
            if (!file || !std::isfinite(t.volume) || t.volume < 0.0f || t.volume > 2.0f ||
                !std::isfinite(t.pan) || t.pan < -1.0f || t.pan > 1.0f ||
                !std::isfinite(t.pan3d_x) || !std::isfinite(t.pan3d_y) || !std::isfinite(t.pan3d_z) ||
                t.trackDelaySamples > 8192 ||
                (version >= 12 && t.type > 4)) return {};
            if (!readString(t.pluginName)) return {};
            uint32_t dataLen = readLE<uint32_t>(file);
            if (!file || dataLen > 64u * 1024u * 1024u) return {};
            if (!addBudget(totalPluginDataBytes, dataLen, kMaxPluginDataBytes)) return {};
            t.pluginData.resize(dataLen);
            if (dataLen) file.read(reinterpret_cast<char*>(t.pluginData.data()), dataLen);
            if (!file) return {};
            if (version >= 24) {
                const uint32_t pluginStateCount = readLE<uint32_t>(file);
                if (!file || pluginStateCount > 4096) return {};
                if (!addCount(totalPluginStateEntries, pluginStateCount, kMaxPluginStateEntries)) return {};
                t.pluginStates.resize(pluginStateCount);
                for (auto& blob : t.pluginStates) {
                    const uint32_t blobLen = readLE<uint32_t>(file);
                    if (!file || blobLen > 16u * 1024u * 1024u) return {};
                    if (!addBudget(totalPluginStateBytes, blobLen, kMaxPluginStateBytes)) return {};
                    blob.resize(blobLen);
                    if (blobLen) file.read(reinterpret_cast<char*>(blob.data()), blobLen);
                    if (!file) return {};
                }
                if (version >= 25) {
                    const uint32_t bypassCount = readLE<uint32_t>(file);
                    if (!file || bypassCount > 4096) return {};
                    t.pluginBypass.resize(bypassCount);
                    for (auto& bypassed : t.pluginBypass) {
                        bypassed = readLE<uint8_t>(file);
                        if (!file || bypassed > 1) return {};
                    }
                }
                if (version >= 32) {
                    const uint32_t guiCount = readLE<uint32_t>(file);
                    if (!file || guiCount > 4096 || guiCount != pluginStateCount) return {};
                    t.pluginGuiStates.resize(guiCount);
                    for (auto& blob : t.pluginGuiStates) { const uint32_t len = readLE<uint32_t>(file); if (!file || len > 1024u * 1024u) return {}; blob.resize(len); if (len) file.read(reinterpret_cast<char*>(blob.data()), len); if (!file) return {}; }
                }
            }
            if (version >= 16) {
                const uint32_t sandboxCount = readLE<uint32_t>(file);
                if (!file || sandboxCount > 4096) return {};
                t.sandboxedPluginPaths.resize(sandboxCount);
                for (auto& path : t.sandboxedPluginPaths) {
                    if (!readString(path)) return {};
                    if (path.empty() || path.size() > 16u * 1024u * 1024u) return {};
                }
                if (version >= 17) {
                    const uint32_t stateCount = readLE<uint32_t>(file);
                    if (!file || stateCount > 4096 || stateCount != sandboxCount) return {};
                    if (!addCount(totalPluginStateEntries, stateCount, kMaxPluginStateEntries)) return {};
                    t.sandboxedPluginStates.resize(stateCount);
                    for (auto& blob : t.sandboxedPluginStates) {
                        const uint32_t blobLen = readLE<uint32_t>(file);
                        if (!file || blobLen > 16u * 1024u * 1024u) return {};
                        if (!addBudget(totalPluginStateBytes, blobLen, kMaxPluginStateBytes)) return {};
                        blob.resize(blobLen);
                        if (blobLen) file.read(reinterpret_cast<char*>(blob.data()), blobLen);
                        if (!file) return {};
                    }
                }
            }
            if (version >= 13) {
                const auto readAutomation = [&file, &totalAutomationPoints, &addCount, version](auto& points) -> bool {
                    const uint32_t count = readLE<uint32_t>(file);
                    if (!file || count > 100000) return false;
                    if (!addCount(totalAutomationPoints, count, kMaxAutomationPoints)) return false;
                    points.resize(count);
                    for (auto& point : points) {
                        point.time = readLE<double>(file);
                        point.value = readLE<float>(file);
                        point.curve = readLE<float>(file);
                        if (!file || !std::isfinite(point.time) || point.time < 0.0 ||
                            (version >= 30 && std::trunc(point.time) != point.time) ||
                            !std::isfinite(point.value) || !std::isfinite(point.curve)) return false;
                    }
                    return true;
                };
                if (!readAutomation(t.volumeAutomation) || !readAutomation(t.panAutomation)) return {};
                if (version >= 30 && !readAutomation(t.trackDelayAutomation)) return {};
            }
        }
        uint32_t regionCount = readLE<uint32_t>(file);
        if (!file || regionCount > 1000000) return {};
        state.regions.resize(regionCount);
        std::unordered_set<uint32_t> regionIds;
        for (auto& r : state.regions) {
            if (version >= 28) {
                r.id = readLE<uint32_t>(file);
                if (!file || r.id == 0 || !regionIds.insert(r.id).second) return {};
            }
            r.trackId = readLE<uint32_t>(file); r.samplePosition = readLE<uint64_t>(file); r.sampleLength = readLE<uint64_t>(file);
            if (!file || !trackIds.contains(r.trackId)) return {};
            if (version >= 19) {
                r.sourceOffset = readLE<uint64_t>(file);
                r.baseStart = readLE<uint64_t>(file);
                r.baseSourceOffset = readLE<uint64_t>(file);
                r.baseLength = readLE<uint64_t>(file);
                if (!file || r.sourceOffset > std::numeric_limits<uint64_t>::max() - r.sampleLength ||
                    r.baseSourceOffset > std::numeric_limits<uint64_t>::max() - r.baseLength) return {};
            }
            r.isMuted = readLE<uint8_t>(file) != 0;
            if (!readString(r.filePath) || !readString(r.name)) return {};
            if (r.filePath.empty() || r.sampleLength == 0 ||
                r.samplePosition > std::numeric_limits<uint64_t>::max() - r.sampleLength) return {};
            if (version >= 11) {
                r.clipGain = readLE<float>(file);
                r.fadeInSamples = readLE<uint64_t>(file);
                r.fadeOutSamples = readLE<uint64_t>(file);
                if (!file || !std::isfinite(r.clipGain) || r.clipGain < 0.0f || r.clipGain > 2.0f ||
                    r.fadeInSamples > r.sampleLength || r.fadeOutSamples > r.sampleLength) return {};
                if (version >= 18) {
                    r.reverse = readLE<uint8_t>(file) != 0;
                    if (!file) return {};
                }
                if (version >= 20) {
                    r.warpRatio = readLE<double>(file);
                    if (!file || !std::isfinite(r.warpRatio) || r.warpRatio < 0.5 || r.warpRatio > 2.0) return {};
                }
                if (version >= 21) {
                    r.pitchSemitones = readLE<float>(file);
                    if (!file || !std::isfinite(r.pitchSemitones) || r.pitchSemitones < -24.0f || r.pitchSemitones > 24.0f) return {};
                }
                if (version >= 22) {
                    r.loopCount = readLE<uint32_t>(file);
                    if (!file || r.loopCount == 0 || r.loopCount > 1024 ||
                        r.sampleLength > std::numeric_limits<uint64_t>::max() / r.loopCount ||
                        r.samplePosition > std::numeric_limits<uint64_t>::max() - r.sampleLength * r.loopCount) return {};
                }
                if (version >= 33) {
                    const uint32_t segmentCount = readLE<uint32_t>(file);
                    if (!file || segmentCount > 100000) return {};
                    r.audioNoteSegments.resize(segmentCount);
                    for (auto& segment : r.audioNoteSegments) {
                        segment.startSeconds = readLE<double>(file); segment.endSeconds = readLE<double>(file);
                        segment.detectedPitchCents = readLE<double>(file); segment.pitchOffsetCents = readLE<double>(file);
                        segment.formantOffsetCents = readLE<double>(file);
                        const uint32_t anchorCount = readLE<uint32_t>(file);
                        if (!file || anchorCount > 10000 || !segment.valid()) return {};
                        segment.anchors.resize(anchorCount);
                        for (auto& anchor : segment.anchors) {
                            anchor.positionSeconds = readLE<double>(file); anchor.pitchCents = readLE<double>(file); anchor.formantCents = readLE<double>(file);
                            if (!file || !std::isfinite(anchor.positionSeconds) || !std::isfinite(anchor.pitchCents) || !std::isfinite(anchor.formantCents) ||
                                anchor.positionSeconds < segment.startSeconds || anchor.positionSeconds > segment.endSeconds) return {};
                        }
                    }
                }
                if (version >= 35) { r.locked = readLE<uint8_t>(file) != 0; r.syncGroup = readLE<uint32_t>(file); if (!file) return {}; }
                if (version >= 35) {
                    const uint32_t count = readLE<uint32_t>(file); if (!file || count > 4096) return {};
                    r.processingHistory.resize(count);
                    for (auto& step : r.processingHistory) { step.id = readLE<uint32_t>(file); step.parameter = readLE<float>(file); step.enabled = readLE<uint8_t>(file) != 0; const uint32_t len = readLE<uint32_t>(file); if (!file || len > 256) return {}; step.operation.resize(len); if (len) file.read(step.operation.data(), len); if (!file || step.operation.empty()) return {}; }
                }
                if (version >= 36) {
                    const uint32_t count = readLE<uint32_t>(file); if (!file || count > 4096) return {};
                    r.rangeEdits.resize(count);
                    for (auto& edit : r.rangeEdits) { edit.start = readLE<uint64_t>(file); edit.end = readLE<uint64_t>(file); edit.gain = readLE<float>(file); edit.fadeIn = readLE<uint64_t>(file); edit.fadeOut = readLE<uint64_t>(file); if (!file || edit.start >= edit.end || !std::isfinite(edit.gain) || edit.gain < 0.0f || edit.gain > 16.0f || edit.fadeIn > edit.end - edit.start || edit.fadeOut > edit.end - edit.start) return {}; }
                }
            }
        }
        if (version >= 26) {
            const uint32_t sidechainCount = readLE<uint32_t>(file);
            if (!file || sidechainCount > 100000) return {};
            state.sidechains.resize(sidechainCount);
            std::unordered_set<uint64_t> sidechainDestinations;
            for (auto& sidechain : state.sidechains) {
                sidechain.sourceId = readLE<uint32_t>(file);
                sidechain.destinationId = readLE<uint32_t>(file);
                sidechain.pluginIndex = readLE<uint32_t>(file);
                sidechain.tapPoint = readLE<uint32_t>(file);
                const uint64_t destinationKey =
                    (static_cast<uint64_t>(sidechain.destinationId) << 32) |
                    static_cast<uint64_t>(sidechain.pluginIndex);
                if (!file || sidechain.sourceId == sidechain.destinationId ||
                    sidechain.tapPoint > 2 ||
                    !sidechainDestinations.insert(destinationKey).second) return {};
            }
        }
        if (version >= 31) {
            const uint32_t routeCount = readLE<uint32_t>(file);
            if (!file || routeCount > 100000) return {};
            state.routes.resize(routeCount);
            std::unordered_set<uint64_t> routeKeys;
            for (auto& route : state.routes) {
                route.sourceId = readLE<uint32_t>(file);
                route.destinationId = readLE<uint32_t>(file);
                route.gain = readLE<float>(file);
                const uint64_t key = (static_cast<uint64_t>(route.sourceId) << 32) |
                                     static_cast<uint64_t>(route.destinationId);
                if (!file || route.sourceId == route.destinationId ||
                    !std::isfinite(route.gain) || route.gain <= 0.0f || route.gain > 2.0f ||
                    !routeKeys.insert(key).second) return {};
            }
        }
        if (version >= 34) {
            const uint32_t markerCount = readLE<uint32_t>(file);
            if (!file || markerCount > 100000) return {};
            state.markers.resize(markerCount);
            std::unordered_set<uint64_t> markerPositions;
            for (auto& marker : state.markers) {
                marker.sample = readLE<uint64_t>(file); marker.color = readLE<uint32_t>(file);
                const uint32_t len = readLE<uint32_t>(file);
                if (!file || len > 1024 || !markerPositions.insert(marker.sample).second) return {};
                marker.name.resize(len); if (len) file.read(marker.name.data(), len);
                if (!file || marker.name.empty()) return {};
            }
            const uint32_t partCount = readLE<uint32_t>(file);
            if (!file || partCount > 100000) return {};
            state.arrangerParts.resize(partCount);
            for (auto& part : state.arrangerParts) {
                part.start = readLE<uint64_t>(file); part.length = readLE<uint64_t>(file); part.repeats = readLE<uint32_t>(file);
                const uint32_t len = readLE<uint32_t>(file);
                if (!file || part.length == 0 || part.repeats == 0 || len == 0 || len > 1024) return {};
                part.name.resize(len); file.read(part.name.data(), len);
                if (!file) return {};
            }
            const uint32_t tempoCount = readLE<uint32_t>(file);
            if (!file || tempoCount > 100000) return {};
            state.tempoEvents.resize(tempoCount);
            uint64_t previousSample = 0;
            for (auto& event : state.tempoEvents) {
                event.sample = readLE<uint64_t>(file); event.bpm = readLE<double>(file); event.ramp = readLE<uint8_t>(file);
                if (!file || !std::isfinite(event.bpm) || event.bpm < 20.0 || event.bpm > 300.0 || event.ramp > 1 || (event.sample < previousSample)) return {};
                previousSample = event.sample;
            }
        }
        if (!file || file.peek() != std::char_traits<char>::eof()) return {};
        state.valid = true;
        return state;
    }
