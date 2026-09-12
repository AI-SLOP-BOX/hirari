    static void writeToStream(std::ostream& stream, const ProjectState& state) {
        uint32_t magic = 0x41555241; 
        writeLE(stream, magic);
        writeLE(stream, state.version);
        writeLE(stream, state.sampleRate);
        writeLE(stream, state.bpm);
        if (state.version >= 14) {
            writeLE(stream, state.rootNote);
            writeLE(stream, state.scaleType);
        }
        
        uint32_t trackCount = static_cast<uint32_t>(state.tracks.size());
        writeLE(stream, trackCount);
        for (const auto& t : state.tracks) {
            writeLE(stream, t.id);
            if (state.version >= 12) writeLE(stream, t.type);
            writeLE(stream, t.volume);
            writeLE(stream, t.pan);
            writeLE(stream, t.pan3d_x); writeLE(stream, t.pan3d_y); writeLE(stream, t.pan3d_z);
            if (state.version >= 12) {
                writeLE(stream, static_cast<uint8_t>(t.muted ? 1 : 0));
                writeLE(stream, static_cast<uint8_t>(t.solo ? 1 : 0));
                if (state.version >= 15) writeLE(stream, static_cast<uint8_t>(t.phaseInvert ? 1 : 0));
                if (state.version >= 23) writeLE(stream, static_cast<uint8_t>(t.recordArmed ? 1 : 0));
                if (state.version >= 29) writeLE(stream, t.trackDelaySamples);
            }
            uint32_t nameLen = static_cast<uint32_t>(t.pluginName.size());
            writeLE(stream, nameLen); stream.write(t.pluginName.data(), nameLen);
            uint32_t dataLen = static_cast<uint32_t>(t.pluginData.size());
            writeLE(stream, dataLen);
            if (dataLen) stream.write(reinterpret_cast<const char*>(t.pluginData.data()), dataLen);
            if (state.version >= 24) {
                const uint32_t pluginStateCount = static_cast<uint32_t>(t.pluginStates.size());
                writeLE(stream, pluginStateCount);
                for (const auto& blob : t.pluginStates) {
                    const uint32_t blobLen = static_cast<uint32_t>(blob.size());
                    writeLE(stream, blobLen);
                    if (blobLen) stream.write(reinterpret_cast<const char*>(blob.data()), blobLen);
                }
                if (state.version >= 25) {
                    const uint32_t bypassCount = static_cast<uint32_t>(t.pluginBypass.size());
                    writeLE(stream, bypassCount);
                    for (const uint8_t bypassed : t.pluginBypass) writeLE(stream, bypassed);
                }
                if (state.version >= 32) {
                    const uint32_t guiCount = static_cast<uint32_t>(t.pluginGuiStates.size());
                    writeLE(stream, guiCount);
                    for (const auto& blob : t.pluginGuiStates) { const uint32_t len = static_cast<uint32_t>(blob.size()); writeLE(stream, len); if (len) stream.write(reinterpret_cast<const char*>(blob.data()), len); }
                }
            }
            if (state.version >= 16) {
                const uint32_t sandboxCount = static_cast<uint32_t>(t.sandboxedPluginPaths.size());
                writeLE(stream, sandboxCount);
                for (const auto& path : t.sandboxedPluginPaths) {
                    const uint32_t pathLen = static_cast<uint32_t>(path.size());
                    writeLE(stream, pathLen);
                    stream.write(path.data(), pathLen);
                }
                if (state.version >= 17) {
                    const uint32_t stateCount = static_cast<uint32_t>(t.sandboxedPluginStates.size());
                    writeLE(stream, stateCount);
                    for (const auto& blob : t.sandboxedPluginStates) {
                        const uint32_t blobLen = static_cast<uint32_t>(blob.size());
                        writeLE(stream, blobLen);
                        if (blobLen) stream.write(reinterpret_cast<const char*>(blob.data()), blobLen);
                    }
                }
            }
            if (state.version >= 13) {
                const auto writeAutomation = [&stream](const auto& points) {
                    const uint32_t count = static_cast<uint32_t>(points.size());
                    writeLE(stream, count);
                    for (const auto& point : points) {
                        writeLE(stream, point.time);
                        writeLE(stream, point.value);
                        writeLE(stream, point.curve);
                    }
                };
                writeAutomation(t.volumeAutomation);
                writeAutomation(t.panAutomation);
                if (state.version >= 30) writeAutomation(t.trackDelayAutomation);
            }
        }
        uint32_t regionCount = static_cast<uint32_t>(state.regions.size());
        writeLE(stream, regionCount);
        for (const auto& r : state.regions) {
            if (state.version >= 28) writeLE(stream, r.id);
            writeLE(stream, r.trackId); writeLE(stream, r.samplePosition); writeLE(stream, r.sampleLength);
            if (state.version >= 19) {
                writeLE(stream, r.sourceOffset);
                writeLE(stream, r.baseStart);
                writeLE(stream, r.baseSourceOffset);
                writeLE(stream, r.baseLength);
            }
            uint8_t muted = r.isMuted ? 1 : 0; writeLE(stream, muted);
            uint32_t pathLen = static_cast<uint32_t>(r.filePath.size());
            writeLE(stream, pathLen); stream.write(r.filePath.data(), pathLen);
            uint32_t regionNameLen = static_cast<uint32_t>(r.name.size());
            writeLE(stream, regionNameLen); stream.write(r.name.data(), regionNameLen);
            writeLE(stream, r.clipGain);
            writeLE(stream, r.fadeInSamples);
            writeLE(stream, r.fadeOutSamples);
            if (state.version >= 18) {
                writeLE(stream, static_cast<uint8_t>(r.reverse ? 1 : 0));
            }
            if (state.version >= 20) writeLE(stream, r.warpRatio);
            if (state.version >= 21) writeLE(stream, r.pitchSemitones);
            if (state.version >= 22) writeLE(stream, r.loopCount);
            if (state.version >= 33) {
                uint32_t count = static_cast<uint32_t>(std::min<size_t>(r.audioNoteSegments.size(), 100000));
                writeLE(stream, count);
                for (const auto& segment : r.audioNoteSegments) {
                    writeLE(stream, segment.startSeconds); writeLE(stream, segment.endSeconds);
                    writeLE(stream, segment.detectedPitchCents); writeLE(stream, segment.pitchOffsetCents);
                    writeLE(stream, segment.formantOffsetCents);
                    uint32_t anchors = static_cast<uint32_t>(std::min<size_t>(segment.anchors.size(), 10000));
                    writeLE(stream, anchors);
                    for (const auto& anchor : segment.anchors) {
                        writeLE(stream, anchor.positionSeconds); writeLE(stream, anchor.pitchCents); writeLE(stream, anchor.formantCents);
                    }
                }
            }
            if (state.version >= 35) { writeLE(stream, static_cast<uint8_t>(r.locked ? 1 : 0)); writeLE(stream, r.syncGroup); }
            if (state.version >= 35) {
                const uint32_t count = static_cast<uint32_t>(std::min<size_t>(r.processingHistory.size(), 4096)); writeLE(stream, count);
                for (const auto& step : r.processingHistory) { writeLE(stream, step.id); writeLE(stream, step.parameter); writeLE(stream, static_cast<uint8_t>(step.enabled ? 1 : 0)); const uint32_t len = static_cast<uint32_t>(std::min<size_t>(step.operation.size(), 256)); writeLE(stream, len); if (len) stream.write(step.operation.data(), len); }
            }
            if (state.version >= 36) {
                uint32_t count = 0;
                for (const auto& edit : r.rangeEdits) {
                    if (edit.start < edit.end && std::isfinite(edit.gain) && edit.gain >= 0.0f &&
                        edit.gain <= 16.0f && edit.fadeIn <= edit.end - edit.start &&
                        edit.fadeOut <= edit.end - edit.start && count < 4096) count++;
                }
                writeLE(stream, count);
                for (const auto& edit : r.rangeEdits) {
                    if (edit.start >= edit.end || !std::isfinite(edit.gain) || edit.gain < 0.0f ||
                        edit.gain > 16.0f || edit.fadeIn > edit.end - edit.start ||
                        edit.fadeOut > edit.end - edit.start || count == 0) continue;
                    writeLE(stream, edit.start); writeLE(stream, edit.end); writeLE(stream, edit.gain);
                    writeLE(stream, edit.fadeIn); writeLE(stream, edit.fadeOut);
                    count--;
                }
            }
        }
        if (state.version >= 26) {
            const uint32_t sidechainCount = static_cast<uint32_t>(state.sidechains.size());
            writeLE(stream, sidechainCount);
            for (const auto& sidechain : state.sidechains) {
                writeLE(stream, sidechain.sourceId);
                writeLE(stream, sidechain.destinationId);
                writeLE(stream, sidechain.pluginIndex);
                writeLE(stream, sidechain.tapPoint);
            }
        }
        if (state.version >= 31) {
            const uint32_t routeCount = static_cast<uint32_t>(state.routes.size());
            writeLE(stream, routeCount);
            for (const auto& route : state.routes) {
                writeLE(stream, route.sourceId);
                writeLE(stream, route.destinationId);
                writeLE(stream, route.gain);
            }
        }
        if (state.version >= 34) {
            const uint32_t markerCount = static_cast<uint32_t>(std::min<size_t>(state.markers.size(), 100000));
            writeLE(stream, markerCount);
            for (const auto& marker : state.markers) {
                writeLE(stream, marker.sample); writeLE(stream, marker.color);
                const uint32_t len = static_cast<uint32_t>(std::min<size_t>(marker.name.size(), 1024));
                writeLE(stream, len); if (len) stream.write(marker.name.data(), len);
            }
            const uint32_t partCount = static_cast<uint32_t>(std::min<size_t>(state.arrangerParts.size(), 100000));
            writeLE(stream, partCount);
            for (const auto& part : state.arrangerParts) {
                writeLE(stream, part.start); writeLE(stream, part.length); writeLE(stream, part.repeats);
                const uint32_t len = static_cast<uint32_t>(std::min<size_t>(part.name.size(), 1024));
                writeLE(stream, len); if (len) stream.write(part.name.data(), len);
            }
            const uint32_t tempoCount = static_cast<uint32_t>(std::min<size_t>(state.tempoEvents.size(), 100000));
            writeLE(stream, tempoCount);
            for (const auto& event : state.tempoEvents) { writeLE(stream, event.sample); writeLE(stream, event.bpm); writeLE(stream, event.ramp); }
        }
    }

