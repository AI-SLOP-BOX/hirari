    static std::vector<uint8_t> serialize(const ProjectState& state) {
        struct TrackEncodeStorage {
            std::vector<HirariProjectBlobView> pluginStates;
            std::vector<HirariProjectBlobView> pluginGuiStates;
            std::vector<HirariProjectBlobView> sandboxPaths;
            std::vector<HirariProjectBlobView> sandboxStates;
            std::vector<HirariProjectAutomationPoint> volumeAutomation;
            std::vector<HirariProjectAutomationPoint> panAutomation;
            std::vector<HirariProjectAutomationPoint> delayAutomation;
            std::vector<std::vector<HirariProjectAutomationPoint>> pluginLanePoints;
            std::vector<HirariProjectPluginAutomationLaneView> pluginLanes;
        };
        std::vector<TrackEncodeStorage> trackStorage;
        trackStorage.reserve(state.tracks.size());
        const auto makeBlobs = [](const auto& blobs) {
            std::vector<HirariProjectBlobView> result;
            result.reserve(blobs.size());
            for (const auto& blob : blobs) result.push_back({blob.data(), blob.size()});
            return result;
        };
        const auto makeAutomation = [](const auto& points) {
            std::vector<HirariProjectAutomationPoint> result;
            result.reserve(points.size());
            for (const auto& point : points) {
                result.push_back({point.time, point.value, point.curve});
            }
            return result;
        };
        for (const auto& track : state.tracks) {
            trackStorage.emplace_back();
            auto& storage = trackStorage.back();
            storage.pluginStates = makeBlobs(track.pluginStates);
            storage.pluginGuiStates = makeBlobs(track.pluginGuiStates);
            storage.sandboxPaths.reserve(track.sandboxedPluginPaths.size());
            for (const auto& path : track.sandboxedPluginPaths) {
                storage.sandboxPaths.push_back({
                    reinterpret_cast<const uint8_t*>(path.data()), path.size()});
            }
            storage.sandboxStates = makeBlobs(track.sandboxedPluginStates);
            storage.volumeAutomation = makeAutomation(track.volumeAutomation);
            storage.panAutomation = makeAutomation(track.panAutomation);
            storage.delayAutomation = makeAutomation(track.trackDelayAutomation);
            storage.pluginLanePoints.reserve(track.pluginAutomation.size());
            for (const auto& lane : track.pluginAutomation) {
                storage.pluginLanePoints.push_back(makeAutomation(lane.points));
            }
            storage.pluginLanes.reserve(track.pluginAutomation.size());
            for (size_t i = 0; i < track.pluginAutomation.size(); ++i) {
                const auto& lane = track.pluginAutomation[i];
                const auto& points = storage.pluginLanePoints[i];
                storage.pluginLanes.push_back({lane.pluginIndex, lane.parameterId,
                                               points.data(), points.size()});
            }
        }
        std::vector<HirariProjectTrackView> trackViews;
        trackViews.reserve(state.tracks.size());
        for (size_t i = 0; i < state.tracks.size(); ++i) {
            const auto& track = state.tracks[i];
            const auto& storage = trackStorage[i];
            HirariProjectTrackView view{};
            view.id = track.id;
            view.track_type = track.type;
            view.volume = track.volume;
            view.pan = track.pan;
            view.pan3d_x = track.pan3d_x;
            view.pan3d_y = track.pan3d_y;
            view.pan3d_z = track.pan3d_z;
            view.muted = static_cast<uint8_t>(track.muted);
            view.solo = static_cast<uint8_t>(track.solo);
            view.phase_invert = static_cast<uint8_t>(track.phaseInvert);
            view.record_armed = static_cast<uint8_t>(track.recordArmed);
            view.track_delay_samples = track.trackDelaySamples;
            view.plugin_name = reinterpret_cast<const uint8_t*>(track.pluginName.data());
            view.plugin_name_size = track.pluginName.size();
            view.plugin_data = track.pluginData.data();
            view.plugin_data_size = track.pluginData.size();
            view.plugin_states = storage.pluginStates.data();
            view.plugin_state_count = storage.pluginStates.size();
            view.plugin_gui_states = storage.pluginGuiStates.data();
            view.plugin_gui_state_count = storage.pluginGuiStates.size();
            view.plugin_bypass = track.pluginBypass.data();
            view.plugin_bypass_count = track.pluginBypass.size();
            view.sandboxed_plugin_paths = storage.sandboxPaths.data();
            view.sandboxed_plugin_path_count = storage.sandboxPaths.size();
            view.sandboxed_plugin_states = storage.sandboxStates.data();
            view.sandboxed_plugin_state_count = storage.sandboxStates.size();
            view.volume_automation = storage.volumeAutomation.data();
            view.volume_automation_count = storage.volumeAutomation.size();
            view.pan_automation = storage.panAutomation.data();
            view.pan_automation_count = storage.panAutomation.size();
            view.track_delay_automation = storage.delayAutomation.data();
            view.track_delay_automation_count = storage.delayAutomation.size();
            view.plugin_automation = storage.pluginLanes.data();
            view.plugin_automation_count = storage.pluginLanes.size();
            trackViews.push_back(view);
        }
        size_t encodedTracksSize = 0;
        uint8_t* encodedTracks = hirari_project_encode_tracks(
            state.version, trackViews.data(), trackViews.size(), &encodedTracksSize);
        if (!encodedTracks) {
            return {};
        }
        struct EncodedTracksGuard {
            uint8_t* data;
            size_t size;
            ~EncodedTracksGuard() { hirari_project_free_encoded_tracks(data, size); }
        } encodedTracksGuard{encodedTracks, encodedTracksSize};
        struct RegionEncodeStorage {
            std::vector<HirariProjectRangeEditView> rangeEdits;
            std::vector<HirariProjectEventStepView> processingHistory;
            std::vector<std::vector<HirariProjectAudioNoteAnchorView>> audioAnchors;
            std::vector<HirariProjectAudioNoteSegmentView> audioSegments;
            std::vector<HirariProjectWarpMarkerView> warpMarkers;
        };
        std::vector<RegionEncodeStorage> regionStorage;
        regionStorage.reserve(state.regions.size());
        for (const auto& region : state.regions) {
            regionStorage.emplace_back();
            auto& storage = regionStorage.back();
            storage.rangeEdits.reserve(region.rangeEdits.size());
            for (const auto& edit : region.rangeEdits) {
                storage.rangeEdits.push_back({edit.start, edit.end, edit.gain,
                                              edit.fadeIn, edit.fadeOut});
            }
            storage.processingHistory.reserve(region.processingHistory.size());
            for (const auto& step : region.processingHistory) {
                storage.processingHistory.push_back({
                    step.id, reinterpret_cast<const uint8_t*>(step.operation.data()),
                    step.operation.size(), step.parameter,
                    static_cast<uint8_t>(step.enabled)});
            }
            storage.audioAnchors.reserve(region.audioNoteSegments.size());
            for (const auto& segment : region.audioNoteSegments) {
                storage.audioAnchors.emplace_back();
                auto& anchors = storage.audioAnchors.back();
                anchors.reserve(segment.anchors.size());
                for (const auto& anchor : segment.anchors) {
                    anchors.push_back({anchor.positionSeconds, anchor.pitchCents,
                                       anchor.formantCents});
                }
            }
            storage.audioSegments.reserve(region.audioNoteSegments.size());
            for (size_t i = 0; i < region.audioNoteSegments.size(); ++i) {
                const auto& segment = region.audioNoteSegments[i];
                const auto& anchors = storage.audioAnchors[i];
                storage.audioSegments.push_back({
                    segment.startSeconds, segment.endSeconds, segment.detectedPitchCents,
                    segment.pitchOffsetCents, segment.formantOffsetCents,
                    anchors.data(), anchors.size()});
            }
            storage.warpMarkers.reserve(region.warpMarkers.size());
            for (const auto& marker : region.warpMarkers) {
                storage.warpMarkers.push_back({marker.sourceSample, marker.timelineSample,
                                               static_cast<uint8_t>(marker.transient)});
            }
        }
        std::vector<HirariProjectRegionView> regionViews;
        regionViews.reserve(state.regions.size());
        for (size_t i = 0; i < state.regions.size(); ++i) {
            const auto& region = state.regions[i];
            const auto& storage = regionStorage[i];
            HirariProjectRegionView view{};
            view.id = region.id;
            view.track_id = region.trackId;
            view.sample_position = region.samplePosition;
            view.sample_length = region.sampleLength;
            view.source_length = region.sourceLength;
            view.source_sample_rate = region.sourceSampleRate;
            view.source_offset = region.sourceOffset;
            view.base_start = region.baseStart;
            view.base_source_offset = region.baseSourceOffset;
            view.base_length = region.baseLength;
            view.muted = static_cast<uint8_t>(region.isMuted);
            view.file_path = reinterpret_cast<const uint8_t*>(region.filePath.data());
            view.file_path_size = region.filePath.size();
            view.name = reinterpret_cast<const uint8_t*>(region.name.data());
            view.name_size = region.name.size();
            view.clip_gain = region.clipGain;
            view.fade_in_samples = region.fadeInSamples;
            view.fade_out_samples = region.fadeOutSamples;
            view.reverse = static_cast<uint8_t>(region.reverse);
            view.warp_ratio = region.warpRatio;
            view.pitch_preserve_warp = static_cast<uint8_t>(region.pitchPreserveWarp);
            view.pitch_semitones = region.pitchSemitones;
            view.loop_count = region.loopCount;
            view.locked = static_cast<uint8_t>(region.locked);
            view.sync_group = region.syncGroup;
            view.range_edits = storage.rangeEdits.data();
            view.range_edit_count = storage.rangeEdits.size();
            view.processing_history = storage.processingHistory.data();
            view.processing_history_count = storage.processingHistory.size();
            view.audio_note_segments = storage.audioSegments.data();
            view.audio_note_segment_count = storage.audioSegments.size();
            view.warp_markers = storage.warpMarkers.data();
            view.warp_marker_count = storage.warpMarkers.size();
            regionViews.push_back(view);
        }
        size_t encodedRegionsSize = 0;
        uint8_t* encodedRegions = hirari_project_encode_regions(
            state.version, state.sampleRate, regionViews.data(), regionViews.size(),
            &encodedRegionsSize);
        if (!encodedRegions) {
            return {};
        }
        struct EncodedRegionsGuard {
            uint8_t* data;
            size_t size;
            ~EncodedRegionsGuard() { hirari_project_free_encoded_regions(data, size); }
        } encodedRegionsGuard{encodedRegions, encodedRegionsSize};
        std::vector<HirariProjectSidechainView> sidechains;
        sidechains.reserve(state.sidechains.size());
        for (const auto& item : state.sidechains) {
            sidechains.push_back({item.sourceId, item.destinationId, item.pluginIndex,
                                  item.tapPoint});
        }
        std::vector<HirariProjectRouteView> routes;
        routes.reserve(state.routes.size());
        for (const auto& item : state.routes) {
            routes.push_back({item.sourceId, item.destinationId, item.gain,
                              static_cast<uint8_t>(item.send),
                              static_cast<uint8_t>(item.preFader)});
        }
        std::vector<HirariProjectMarkerView> markers;
        markers.reserve(state.markers.size());
        for (const auto& item : state.markers) {
            markers.push_back({item.sample,
                               reinterpret_cast<const uint8_t*>(item.name.data()),
                               item.name.size(), item.color});
        }
        std::vector<HirariProjectArrangerPartView> arrangerParts;
        arrangerParts.reserve(state.arrangerParts.size());
        for (const auto& item : state.arrangerParts) {
            arrangerParts.push_back({item.start, item.length, item.repeats,
                                     reinterpret_cast<const uint8_t*>(item.name.data()),
                                     item.name.size()});
        }
        std::vector<HirariProjectTempoEventView> tempoEvents;
        tempoEvents.reserve(state.tempoEvents.size());
        for (const auto& item : state.tempoEvents) {
            tempoEvents.push_back({item.sample, item.bpm, item.ramp});
        }
        const HirariProjectTailView tail{
            sidechains.data(), sidechains.size(), routes.data(), routes.size(),
            markers.data(), markers.size(), arrangerParts.data(), arrangerParts.size(),
            tempoEvents.data(), tempoEvents.size()};
        size_t tailSize = 0;
        uint8_t* encodedTail = hirari_project_encode_tail(state.version, &tail, &tailSize);
        if (!encodedTail) {
            return {};
        }
        struct EncodedTailGuard {
            uint8_t* data;
            size_t size;
            ~EncodedTailGuard() { hirari_project_free_encoded_tail(data, size); }
        } encodedTailGuard{encodedTail, tailSize};
        size_t payloadSize = 0;
        uint8_t* payload = hirari_project_assemble_native_payload(
            state.version, state.sampleRate, state.bpm, state.rootNote, state.scaleType,
            encodedTracks, encodedTracksSize, encodedRegions, encodedRegionsSize,
            encodedTail, tailSize, &payloadSize);
        if (!payload) {
            return {};
        }
        struct AssembledPayloadGuard {
            uint8_t* data;
            size_t size;
            ~AssembledPayloadGuard() { hirari_project_free_assembled_payload(data, size); }
        } payloadGuard{payload, payloadSize};
        return std::vector<uint8_t>(payload, payload + payloadSize);
    }
