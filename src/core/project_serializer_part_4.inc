    static ProjectState load(const std::string& path) {
        ProjectState state{};
        size_t byteCount = 0;
        HirariNativeProjectHeader header{};
        uint8_t* loadedBytes = hirari_load_project_payload(
            path.data(), path.size(), &byteCount, &header);
        if (!loadedBytes) return state;
        struct LoadedBytesGuard {
            uint8_t* data;
            size_t size;
            ~LoadedBytesGuard() { hirari_free_loaded_project_bytes(data, size); }
        } loadedBytesGuard{loadedBytes, byteCount};
        if (byteCount > kMaxProjectBytes || header.version == 0 || header.version > 42 ||
            header.header_size > byteCount) return {};
        state.version = header.version;
        state.sampleRate = header.sample_rate;
        state.bpm = header.bpm;
        state.rootNote = header.root_note;
        state.scaleType = header.scale_type;
        const uint32_t version = header.version;
        size_t totalStringBytes = 0;
        const auto addBudget = [](size_t& total, size_t amount, size_t limit) -> bool {
            if (amount > limit || total > limit - amount) return false;
            total += amount;
            return true;
        };
        std::unordered_set<uint32_t> trackIds;
        state.tracks.reserve(header.track_count);
        size_t trackSectionOffset = 0;
        const auto consumeTrack = [](void* context, const HirariProjectTrackView* view) -> bool {
            if (!context || !view) return false;
            try {
            auto& tracks = *static_cast<std::vector<TrackState>*>(context);
            TrackState track{};
            track.id = view->id;
            track.type = view->track_type;
            track.volume = view->volume;
            track.pan = view->pan;
            track.pan3d_x = view->pan3d_x;
            track.pan3d_y = view->pan3d_y;
            track.pan3d_z = view->pan3d_z;
            track.muted = view->muted != 0;
            track.solo = view->solo != 0;
            track.phaseInvert = view->phase_invert != 0;
            track.recordArmed = view->record_armed != 0;
            track.trackDelaySamples = view->track_delay_samples;
            if (view->plugin_name_size != 0) {
                if (!view->plugin_name) return false;
                track.pluginName.assign(reinterpret_cast<const char*>(view->plugin_name),
                                         view->plugin_name_size);
            }
            const auto copyBytes = [](std::vector<uint8_t>& destination,
                                      const uint8_t* source, size_t count) -> bool {
                if (count == 0) { destination.clear(); return true; }
                if (!source) return false;
                destination.assign(source, source + count);
                return true;
            };
            if (!copyBytes(track.pluginData, view->plugin_data, view->plugin_data_size) ||
                !copyBytes(track.pluginBypass, view->plugin_bypass, view->plugin_bypass_count)) {
                return false;
            }
            const auto copyBlobs = [&copyBytes](auto& destination,
                                                const HirariProjectBlobView* source,
                                                size_t count) -> bool {
                if (count != 0 && !source) return false;
                destination.reserve(count);
                for (size_t index = 0; index < count; ++index) {
                    std::vector<uint8_t> blob;
                    if (!copyBytes(blob, source[index].data, source[index].size)) return false;
                    destination.push_back(std::move(blob));
                }
                return true;
            };
            if (!copyBlobs(track.pluginStates, view->plugin_states, view->plugin_state_count) ||
                !copyBlobs(track.pluginGuiStates, view->plugin_gui_states,
                           view->plugin_gui_state_count) ||
                !copyBlobs(track.sandboxedPluginStates, view->sandboxed_plugin_states,
                           view->sandboxed_plugin_state_count)) return false;
            if (view->sandboxed_plugin_path_count != 0 && !view->sandboxed_plugin_paths) {
                return false;
            }
            track.sandboxedPluginPaths.reserve(view->sandboxed_plugin_path_count);
            for (size_t index = 0; index < view->sandboxed_plugin_path_count; ++index) {
                const auto& path = view->sandboxed_plugin_paths[index];
                if (path.size != 0 && !path.data) return false;
                track.sandboxedPluginPaths.emplace_back(
                    reinterpret_cast<const char*>(path.data), path.size);
            }
            const auto copyAutomation = [](auto& destination,
                                           const HirariProjectAutomationPoint* source,
                                           size_t count) -> bool {
                if (count != 0 && !source) return false;
                destination.reserve(count);
                for (size_t index = 0; index < count; ++index) {
                    destination.push_back({source[index].time, source[index].value,
                                           source[index].curve});
                }
                return true;
            };
            if (!copyAutomation(track.volumeAutomation, view->volume_automation,
                                view->volume_automation_count) ||
                !copyAutomation(track.panAutomation, view->pan_automation,
                                view->pan_automation_count) ||
                !copyAutomation(track.trackDelayAutomation, view->track_delay_automation,
                                view->track_delay_automation_count)) return false;
            if (view->plugin_automation_count != 0 && !view->plugin_automation) return false;
            track.pluginAutomation.reserve(view->plugin_automation_count);
            for (size_t index = 0; index < view->plugin_automation_count; ++index) {
                const auto& source = view->plugin_automation[index];
                if (source.point_count != 0 && !source.points) return false;
                PluginAutomationLane lane{};
                lane.pluginIndex = source.plugin_index;
                lane.parameterId = source.parameter_id;
                if (!copyAutomation(lane.points, source.points, source.point_count)) return false;
                track.pluginAutomation.push_back(std::move(lane));
            }
            tracks.push_back(std::move(track));
            return true;
            } catch (...) {
                return false;
            }
        };
        if (!hirari_project_decode_tracks(
                loadedBytes, byteCount, header.header_size, header.version,
                header.track_count, &state.tracks, consumeTrack,
                &trackSectionOffset, &totalStringBytes) ||
            trackSectionOffset > byteCount) return {};
        for (const auto& track : state.tracks) {
            if (!trackIds.insert(track.id).second) return {};
        }
        std::vector<uint32_t> trackIdList;
        trackIdList.reserve(state.tracks.size());
        for (const auto& track : state.tracks) trackIdList.push_back(track.id);
        size_t regionSectionOffset = 0;
        state.regions.reserve(1024);
        const auto consumeRegion = [](void* context, const HirariProjectRegionView* view) -> bool {
            if (!context || !view) return false;
            try {
                auto& regions = *static_cast<std::vector<RegionState>*>(context);
                RegionState region{};
                region.id = view->id;
                region.trackId = view->track_id;
                region.samplePosition = view->sample_position;
                region.sampleLength = view->sample_length;
                region.sourceLength = view->source_length;
                region.sourceSampleRate = view->source_sample_rate;
                region.sourceOffset = view->source_offset;
                region.baseStart = view->base_start;
                region.baseSourceOffset = view->base_source_offset;
                region.baseLength = view->base_length;
                region.isMuted = view->muted != 0;
                if (view->file_path_size != 0) {
                    if (!view->file_path) return false;
                    region.filePath.assign(reinterpret_cast<const char*>(view->file_path),
                                           view->file_path_size);
                }
                if (view->name_size != 0) {
                    if (!view->name) return false;
                    region.name.assign(reinterpret_cast<const char*>(view->name), view->name_size);
                }
                region.clipGain = view->clip_gain;
                region.fadeInSamples = view->fade_in_samples;
                region.fadeOutSamples = view->fade_out_samples;
                region.reverse = view->reverse != 0;
                region.warpRatio = view->warp_ratio;
                region.pitchPreserveWarp = view->pitch_preserve_warp != 0;
                region.pitchSemitones = view->pitch_semitones;
                region.loopCount = view->loop_count;
                region.locked = view->locked != 0;
                region.syncGroup = view->sync_group;
                if (view->range_edit_count != 0 && !view->range_edits) return false;
                region.rangeEdits.reserve(view->range_edit_count);
                for (size_t index = 0; index < view->range_edit_count; ++index) {
                    const auto& source = view->range_edits[index];
                    region.rangeEdits.push_back({source.start, source.end, source.gain,
                                                 source.fade_in, source.fade_out});
                }
                if (view->processing_history_count != 0 && !view->processing_history) return false;
                region.processingHistory.reserve(view->processing_history_count);
                for (size_t index = 0; index < view->processing_history_count; ++index) {
                    const auto& source = view->processing_history[index];
                    if (source.operation_size != 0 && !source.operation) return false;
                    ::hirari::editing::EventProcessingStep step{};
                    step.id = source.id;
                    step.parameter = source.parameter;
                    step.enabled = source.enabled != 0;
                    step.operation.assign(reinterpret_cast<const char*>(source.operation),
                                          source.operation_size);
                    region.processingHistory.push_back(std::move(step));
                }
                if (view->audio_note_segment_count != 0 && !view->audio_note_segments) {
                    return false;
                }
                region.audioNoteSegments.reserve(view->audio_note_segment_count);
                for (size_t index = 0; index < view->audio_note_segment_count; ++index) {
                    const auto& source = view->audio_note_segments[index];
                    if (source.anchor_count != 0 && !source.anchors) return false;
                    ::hirari::editing::AudioNoteSegment segment{};
                    segment.startSeconds = source.start_seconds;
                    segment.endSeconds = source.end_seconds;
                    segment.detectedPitchCents = source.detected_pitch_cents;
                    segment.pitchOffsetCents = source.pitch_offset_cents;
                    segment.formantOffsetCents = source.formant_offset_cents;
                    segment.anchors.reserve(source.anchor_count);
                    for (size_t anchorIndex = 0; anchorIndex < source.anchor_count; ++anchorIndex) {
                        const auto& anchor = source.anchors[anchorIndex];
                        segment.anchors.push_back({anchor.position_seconds, anchor.pitch_cents,
                                                   anchor.formant_cents});
                    }
                    region.audioNoteSegments.push_back(std::move(segment));
                }
                if (view->warp_marker_count != 0 && !view->warp_markers) return false;
                region.warpMarkers.reserve(view->warp_marker_count);
                for (size_t index = 0; index < view->warp_marker_count; ++index) {
                    const auto& source = view->warp_markers[index];
                    region.warpMarkers.push_back(
                        {source.source_sample, source.timeline_sample, source.transient != 0});
                }
                regions.push_back(std::move(region));
                return true;
            } catch (...) {
                return false;
            }
        };
        if (!hirari_project_decode_regions(
                loadedBytes, byteCount, trackSectionOffset, version, state.sampleRate,
                trackIdList.data(), trackIdList.size(), totalStringBytes, &state.regions,
                consumeRegion, &regionSectionOffset) ||
            regionSectionOffset > byteCount) return {};
        size_t tailSectionOffset = 0;
        const auto consumeTail = [](void* context, const HirariProjectTailView* view) -> bool {
            if (!context || !view) return false;
            try {
                auto& project = *static_cast<ProjectState*>(context);
                if ((view->sidechain_count && !view->sidechains) ||
                    (view->route_count && !view->routes) ||
                    (view->marker_count && !view->markers) ||
                    (view->arranger_part_count && !view->arranger_parts) ||
                    (view->tempo_event_count && !view->tempo_events)) return false;
                project.sidechains.reserve(view->sidechain_count);
                for (size_t i = 0; i < view->sidechain_count; ++i) {
                    const auto& source = view->sidechains[i];
                    project.sidechains.push_back({source.source_id, source.destination_id,
                                                  source.plugin_index, source.tap_point});
                }
                project.routes.reserve(view->route_count);
                for (size_t i = 0; i < view->route_count; ++i) {
                    const auto& source = view->routes[i];
                    project.routes.push_back({source.source_id, source.destination_id,
                                              source.gain, source.send != 0,
                                              source.pre_fader != 0});
                }
                project.markers.reserve(view->marker_count);
                for (size_t i = 0; i < view->marker_count; ++i) {
                    const auto& source = view->markers[i];
                    if (source.name_size && !source.name) return false;
                    MarkerState marker{};
                    marker.sample = source.sample;
                    marker.color = source.color;
                    marker.name.assign(reinterpret_cast<const char*>(source.name),
                                       source.name_size);
                    project.markers.push_back(std::move(marker));
                }
                project.arrangerParts.reserve(view->arranger_part_count);
                for (size_t i = 0; i < view->arranger_part_count; ++i) {
                    const auto& source = view->arranger_parts[i];
                    if (source.name_size && !source.name) return false;
                    ArrangerPartState part{};
                    part.start = source.start;
                    part.length = source.length;
                    part.repeats = source.repeats;
                    part.name.assign(reinterpret_cast<const char*>(source.name),
                                     source.name_size);
                    project.arrangerParts.push_back(std::move(part));
                }
                project.tempoEvents.reserve(view->tempo_event_count);
                for (size_t i = 0; i < view->tempo_event_count; ++i) {
                    const auto& source = view->tempo_events[i];
                    project.tempoEvents.push_back({source.sample, source.bpm, source.ramp});
                }
                return true;
            } catch (...) {
                return false;
            }
        };
        if (!hirari_project_decode_tail(loadedBytes, byteCount, regionSectionOffset,
                                        version, &state, consumeTail, &tailSectionOffset) ||
            tailSectionOffset != byteCount) return {};
        state.valid = true;
        return state;
    }
