//! Layout JSON writer: note segments, regions, processing history and
//! tracks, split out of project_layout.rs.
use super::*;

pub(crate) fn write_automation_points(
    writer: &mut LayoutWriter,
    points: &[HirariLayoutAutomationPoint],
) -> bool {
    writer.raw(b"[");
    for (index, point) in points.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"time\":");
        writer.number(point.time);
        writer.raw(b",\"value\":");
        writer.number32(point.value);
        writer.raw(b",\"curve\":");
        writer.number32(point.curve);
        writer.raw(b"}");
    }
    writer.raw(b"]");
    true
}

pub(crate) fn write_note_segment(
    writer: &mut LayoutWriter,
    start_seconds: f64,
    end_seconds: f64,
    detected_pitch_cents: f64,
    pitch_offset_cents: f64,
    formant_offset_cents: f64,
    anchors: &[HirariAudioNoteAnchor],
) -> bool {
    writer.raw(b"{\"start\":");
    writer.number(start_seconds);
    writer.raw(b",\"end\":");
    writer.number(end_seconds);
    writer.raw(b",\"detected_pitch_cents\":");
    writer.number(detected_pitch_cents);
    writer.raw(b",\"pitch_offset_cents\":");
    writer.number(pitch_offset_cents);
    writer.raw(b",\"formant_offset_cents\":");
    writer.number(formant_offset_cents);
    writer.raw(b",\"anchors\":[");
    for (index, anchor) in anchors.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"position\":");
        writer.number(anchor.position_seconds);
        writer.raw(b",\"pitch_cents\":");
        writer.number(anchor.pitch_cents);
        writer.raw(b",\"formant_cents\":");
        writer.number(anchor.formant_cents);
        writer.raw(b"}");
    }
    writer.raw(b"]}");
    true
}

pub(crate) unsafe fn write_region_note_segments(
    writer: &mut LayoutWriter,
    region: &HirariLayoutRegion,
) -> bool {
    if region.note_store.is_null() {
        return false;
    }
    writer.raw(b"[");
    let mut first = true;
    if !region.note_frozen.is_null() {
        let frozen = unsafe { &*region.note_frozen };
        for (curve, range) in frozen.segment_views() {
            if writer.cancelled() {
                return false;
            }
            let anchors =
                match unsafe { optional_slice(curve.anchors, curve.anchor_count) } {
                    Some(anchors) => anchors,
                    None => return false,
                };
            if !first {
                writer.raw(b",");
            }
            first = false;
            if !write_note_segment(
                writer,
                curve.start_seconds,
                curve.end_seconds,
                range.detected_pitch_cents,
                curve.pitch_offset_cents,
                curve.formant_offset_cents,
                anchors,
            ) {
                return false;
            }
        }
        writer.raw(b"]");
        return true;
    }

    let mut segment_count = 0usize;
    let mut anchor_total = 0usize;
    if !unsafe {
        hirari_region_vari_audio_store_sizes(region.note_store, &mut segment_count, &mut anchor_total)
    } {
        return false;
    }
    // The note store already enforces these bounds; they only keep the
    // materialization allocation honest if a foreign store is passed in.
    if segment_count > 4096 || anchor_total > 4096 * 4096 {
        return false;
    }
    if segment_count > 0 {
        let mut segments = vec![HirariAudioNoteSegment::default(); segment_count];
        let mut anchors = vec![HirariAudioNoteAnchor::default(); anchor_total];
        let mut offsets = vec![0usize; segment_count];
        let mut counts = vec![0usize; segment_count];
        let mut written_segments = 0usize;
        let mut written_anchors = 0usize;
        if !unsafe {
            hirari_region_vari_audio_store_materialize(
                region.note_store,
                segments.as_mut_ptr(),
                segments.len(),
                anchors.as_mut_ptr(),
                anchors.len(),
                offsets.as_mut_ptr(),
                counts.as_mut_ptr(),
                &mut written_segments,
                &mut written_anchors,
            )
        } {
            return false;
        }
        for index in 0..written_segments {
            if writer.cancelled() {
                return false;
            }
            let segment = segments[index];
            let start = offsets[index];
            let count = counts[index];
            if start > written_anchors || count > written_anchors - start {
                return false;
            }
            if !first {
                writer.raw(b",");
            }
            first = false;
            if !write_note_segment(
                writer,
                segment.start_seconds,
                segment.end_seconds,
                segment.detected_pitch_cents,
                segment.pitch_offset_cents,
                segment.formant_offset_cents,
                &anchors[start..start + count],
            ) {
                return false;
            }
        }
    }
    writer.raw(b"]");
    true
}

pub(crate) unsafe fn write_region_processing_history(
    writer: &mut LayoutWriter,
    region: &HirariLayoutRegion,
) -> bool {
    if region.processing_store.is_null() {
        return false;
    }
    let mut count = 0usize;
    let mut byte_total = 0usize;
    if !unsafe {
        crate::region_processing_history::hirari_region_processing_history_sizes(
            region.processing_store,
            &mut count,
            &mut byte_total,
        )
    } {
        return false;
    }
    if count > 4096 || byte_total > 1_048_576 {
        return false;
    }
    let mut views = vec![HirariEventStepView::default(); count];
    let mut bytes = vec![0u8; byte_total];
    let mut written_count = 0usize;
    let mut written_bytes = 0usize;
    if !unsafe {
        crate::region_processing_history::hirari_region_processing_history_materialize(
            region.processing_store,
            views.as_mut_ptr(),
            views.len(),
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut written_count,
            &mut written_bytes,
        )
    } {
        return false;
    }
    writer.raw(b"[");
    for (index, view) in views.iter().take(written_count).enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        if view.operation_offset > written_bytes
            || view.operation_len > written_bytes - view.operation_offset
        {
            return false;
        }
        let operation = &bytes[view.operation_offset..view.operation_offset + view.operation_len];
        writer.raw(b"{\"id\":");
        writer.raw(view.id.to_string().as_bytes());
        writer.raw(b",\"operation\":\"");
        writer.text(operation);
        writer.raw(b"\",\"parameter\":");
        writer.number32(view.parameter);
        writer.raw(b",\"enabled\":");
        writer.boolean(view.enabled);
        writer.raw(b"}");
    }
    writer.raw(b"]");
    true
}

pub(crate) unsafe fn write_region(writer: &mut LayoutWriter, region: &HirariLayoutRegion) -> bool {
    let state = &region.state;
    let name = match unsafe { string_bytes(&region.name) } {
        Some(name) => name,
        None => return false,
    };
    let path = match unsafe { string_bytes(&region.path) } {
        Some(path) => path,
        None => return false,
    };
    let markers = match unsafe { optional_slice(region.warp_markers, region.warp_marker_count) } {
        Some(markers) => markers,
        None => return false,
    };
    let ranges = match unsafe { optional_slice(region.range_edits, region.range_edit_count) } {
        Some(ranges) => ranges,
        None => return false,
    };

    writer.raw(b"{\"id\":");
    writer.raw(region.id.to_string().as_bytes());
    writer.raw(b",\"name\":\"");
    writer.text(name);
    writer.raw(b"\",\"start\":");
    writer.raw(state.start.to_string().as_bytes());
    writer.raw(b",\"len\":");
    writer.raw(state.len.to_string().as_bytes());
    writer.raw(b",\"source_length\":");
    writer.raw(state.source_length.to_string().as_bytes());
    writer.raw(b",\"source_sample_rate\":");
    writer.raw(region.source_sample_rate.to_string().as_bytes());
    writer.raw(b",\"source_offset\":");
    writer.raw(state.source_offset.to_string().as_bytes());
    writer.raw(b",\"base_start\":");
    writer.raw(state.base_start.to_string().as_bytes());
    writer.raw(b",\"base_source_offset\":");
    writer.raw(state.base_source_offset.to_string().as_bytes());
    writer.raw(b",\"base_length\":");
    writer.raw(state.base_length.to_string().as_bytes());
    writer.raw(b",\"muted\":");
    writer.boolean(state.muted);
    writer.raw(b",\"path\":\"");
    writer.text(path);
    writer.raw(b"\",\"missing\":");
    writer.boolean(region.missing);
    writer.raw(b",\"audio_loaded\":");
    writer.boolean(region.audio_loaded);
    writer.raw(b",\"clip_gain\":");
    writer.number32(state.clip_gain);
    writer.raw(b",\"fade_in_samples\":");
    writer.raw(state.fade_in_samples.to_string().as_bytes());
    writer.raw(b",\"fade_out_samples\":");
    writer.raw(state.fade_out_samples.to_string().as_bytes());
    writer.raw(b",\"reverse\":");
    writer.boolean(state.reverse);
    writer.raw(b",\"warp_ratio\":");
    writer.number(state.warp_ratio);
    writer.raw(b",\"pitch_preserve_warp\":");
    writer.boolean(state.pitch_preserve_warp);
    writer.raw(b",\"pitch_semitones\":");
    writer.number32(state.pitch_semitones);
    writer.raw(b",\"loop_count\":");
    writer.raw(state.loop_count.to_string().as_bytes());
    writer.raw(b",\"sync_group\":");
    writer.raw(state.sync_group.to_string().as_bytes());
    writer.raw(b",\"warp_markers\":[");
    for (index, marker) in markers.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"source_sample\":");
        writer.raw(marker.source_sample.to_string().as_bytes());
        writer.raw(b",\"timeline_sample\":");
        writer.raw(marker.timeline_sample.to_string().as_bytes());
        writer.raw(b",\"transient\":");
        writer.boolean(marker.transient != 0);
        writer.raw(b"}");
    }
    writer.raw(b"],\"range_edits\":[");
    for (index, edit) in ranges.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"{\"start\":");
        writer.raw(edit.start.to_string().as_bytes());
        writer.raw(b",\"end\":");
        writer.raw(edit.end.to_string().as_bytes());
        writer.raw(b",\"gain\":");
        writer.number32(edit.gain);
        writer.raw(b",\"fade_in\":");
        writer.raw(edit.fade_in.to_string().as_bytes());
        writer.raw(b",\"fade_out\":");
        writer.raw(edit.fade_out.to_string().as_bytes());
        writer.raw(b"}");
    }
    writer.raw(b"],\"audio_note_segments\":");
    if !unsafe { write_region_note_segments(writer, region) } {
        return false;
    }
    writer.raw(b",\"processing_history\":");
    if !unsafe { write_region_processing_history(writer, region) } {
        return false;
    }
    writer.raw(b"}");
    true
}

pub(crate) fn marker_value(parameters: &[HirariLayoutParameterBlock], index: usize) -> Option<f32> {
    let Some(block) = parameters.get(index) else {
        return Some(0.0);
    };
    let values = unsafe { optional_slice(block.values, block.count) }?;
    Some(values.first().copied().unwrap_or(0.0))
}

pub(crate) unsafe fn write_track(writer: &mut LayoutWriter, track: &HirariLayoutTrack) -> bool {
    if writer.cancelled() {
        return false;
    }
    let name = match unsafe { string_bytes(&track.name) } {
        Some(name) => name,
        None => return false,
    };
    let frozen_path = match unsafe { string_bytes(&track.frozen_path) } {
        Some(path) => path,
        None => return false,
    };
    let expression_map = match unsafe { string_bytes(&track.expression_map_json) } {
        Some(json) => json,
        None => return false,
    };
    let expression_map_pro = match unsafe { string_bytes(&track.expression_map_pro_json) } {
        Some(json) => json,
        None => return false,
    };
    let volume_automation =
        match unsafe { optional_slice(track.volume_automation, track.volume_automation_count) } {
            Some(points) => points,
            None => return false,
        };
    let pan_automation =
        match unsafe { optional_slice(track.pan_automation, track.pan_automation_count) } {
            Some(points) => points,
            None => return false,
        };
    let delay_automation = match unsafe {
        optional_slice(track.track_delay_automation, track.track_delay_automation_count)
    } {
        Some(points) => points,
        None => return false,
    };
    let lanes = match unsafe {
        optional_slice(track.plugin_automation, track.plugin_automation_count)
    } {
        Some(lanes) => lanes,
        None => return false,
    };
    let plugin_types = match unsafe { optional_slice(track.plugin_types, track.plugin_type_count) } {
        Some(types) => types,
        None => return false,
    };
    let bypass = match unsafe { optional_slice(track.plugin_bypass, plugin_types.len()) } {
        Some(bypass) => bypass,
        None => return false,
    };
    let parameters =
        match unsafe { optional_slice(track.plugin_parameters, plugin_types.len()) } {
            Some(parameters) => parameters,
            None => return false,
        };
    let states = match unsafe {
        optional_slice(track.plugin_states, track.plugin_state_count)
    } {
        Some(states) => states,
        None => return false,
    };
    let sandbox_paths = match unsafe {
        optional_slice(track.sandbox_plugin_paths, track.sandbox_plugin_path_count)
    } {
        Some(paths) => paths,
        None => return false,
    };
    let sandbox_states = match unsafe {
        optional_slice(track.sandbox_plugin_states, track.sandbox_plugin_state_count)
    } {
        Some(states) => states,
        None => return false,
    };
    let regions = match unsafe { optional_slice(track.regions, track.region_count) } {
        Some(regions) => regions,
        None => return false,
    };
    for state in states {
        if unsafe { optional_slice(state.bytes, state.count) }.is_none() {
            return false;
        }
    }
    for state in sandbox_states {
        if unsafe { optional_slice(state.bytes, state.count) }.is_none() {
            return false;
        }
    }
    for path in sandbox_paths {
        if unsafe { optional_slice(path.data, path.len) }.is_none() {
            return false;
        }
    }

    let type_name: &[u8] = match track.track_type {
        1 => b"Midi",
        2 => b"Instrument",
        3 => b"Bus",
        4 => b"Vocal",
        _ => b"Audio",
    };

    writer.raw(b"{\"id\":");
    writer.raw(track.id.to_string().as_bytes());
    writer.raw(b",\"name\":\"");
    writer.text(name);
    writer.raw(b"\",\"type\":\"");
    writer.raw(type_name);
    writer.raw(b"\",\"volume\":");
    writer.number32(track.volume);
    writer.raw(b",\"pan\":");
    writer.number32(track.pan);
    writer.raw(b",\"mute\":");
    writer.boolean(track.muted);
    writer.raw(b",\"solo\":");
    writer.boolean(track.solo);
    writer.raw(b",\"record_armed\":");
    writer.boolean(track.record_armed);
    writer.raw(b",\"phase_invert\":");
    writer.boolean(track.phase_inverted);
    writer.raw(b",\"track_delay_samples\":");
    writer.raw(track.track_delay_samples.to_string().as_bytes());
    writer.raw(b",\"frozen\":");
    writer.boolean(track.frozen);
    writer.raw(b",\"frozen_total_samples\":");
    writer.raw(track.frozen_total_samples.to_string().as_bytes());
    writer.raw(b",\"frozen_sample_rate\":");
    writer.raw(track.frozen_sample_rate.to_string().as_bytes());
    writer.raw(b",\"frozen_path\":\"");
    writer.text(frozen_path);
    writer.raw(b"\",\"volume_automation\":");
    if !write_automation_points(writer, volume_automation) {
        return false;
    }
    writer.raw(b",\"pan_automation\":");
    if !write_automation_points(writer, pan_automation) {
        return false;
    }
    writer.raw(b",\"track_delay_automation\":");
    if !write_automation_points(writer, delay_automation) {
        return false;
    }
    writer.raw(b",\"expression_map\":");
    writer.raw(expression_map);
    writer.raw(b",\"expression_map_pro\":");
    writer.raw(expression_map_pro);
    writer.raw(b",\"plugin_automation\":[");
    for (lane_index, lane) in lanes.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if lane_index > 0 {
            writer.raw(b",");
        }
        let points = match unsafe { optional_slice(lane.points, lane.point_count) } {
            Some(points) => points,
            None => return false,
        };
        writer.raw(b"{\"plugin_index\":");
        writer.raw(lane.plugin_index.to_string().as_bytes());
        writer.raw(b",\"parameter_id\":");
        writer.raw(lane.parameter_id.to_string().as_bytes());
        writer.raw(b",\"points\":[");
        for (point_index, point) in points.iter().enumerate() {
            if writer.cancelled() {
                return false;
            }
            if point_index > 0 {
                writer.raw(b",");
            }
            writer.raw(b"{\"sample\":");
            writer.raw((point.time as u64).to_string().as_bytes());
            writer.raw(b",\"normalized\":");
            writer.number32(point.value);
            writer.raw(b",\"curve\":");
            writer.number32(point.curve);
            writer.raw(b"}");
        }
        writer.raw(b"]}");
    }
    writer.raw(b"],\"plugin_types\":[");
    for (index, plugin_type) in plugin_types.iter().enumerate() {
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(plugin_type.to_string().as_bytes());
    }
    writer.raw(b"],\"plugin_bypass\":[");
    for (index, value) in bypass.iter().enumerate() {
        if index > 0 {
            writer.raw(b",");
        }
        writer.boolean(*value != 0);
    }
    writer.raw(b"],\"plugin_parameter_values\":[");
    for (plugin_index, block) in parameters.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if plugin_index > 0 {
            writer.raw(b",");
        }
        let values = match unsafe { optional_slice(block.values, block.count) } {
            Some(values) => values,
            None => return false,
        };
        writer.raw(b"[");
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                writer.raw(b",");
            }
            writer.parameter(*value);
        }
        writer.raw(b"]");
    }
    writer.raw(b"],\"plugin_state_sizes\":[");
    let padded_state_count = states.len().max(plugin_types.len());
    for index in 0..padded_state_count {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        let size = match states.get(index) {
            Some(state) => state.count,
            None => std::mem::size_of::<f32>(),
        };
        writer.raw(size.to_string().as_bytes());
    }
    writer.raw(b"],\"plugin_state_hashes\":[");
    for index in 0..padded_state_count {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        let hash = match states.get(index) {
            Some(state) => {
                let bytes = match unsafe { optional_slice(state.bytes, state.count) } {
                    Some(bytes) => bytes,
                    None => return false,
                };
                fnv1a(bytes)
            }
            None => match marker_value(parameters, index) {
                Some(value) => fnv1a(&value.to_ne_bytes()),
                None => return false,
            },
        };
        writer.raw(hash.to_string().as_bytes());
    }
    writer.raw(b"],\"plugin_state_hex\":[");
    for index in 0..padded_state_count {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"\"");
        match states.get(index) {
            Some(state) => {
                let bytes = match unsafe { optional_slice(state.bytes, state.count) } {
                    Some(bytes) => bytes,
                    None => return false,
                };
                writer.hex(bytes);
            }
            None => match marker_value(parameters, index) {
                Some(value) => writer.hex(&value.to_ne_bytes()),
                None => return false,
            },
        }
        writer.raw(b"\"");
    }
    writer.raw(b"],\"sandbox_plugin_paths\":[");
    for (index, path) in sandbox_paths.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        writer.raw(b"\"");
        let bytes = match unsafe { optional_slice(path.data, path.len) } {
            Some(bytes) => bytes,
            None => return false,
        };
        writer.text(bytes);
        writer.raw(b"\"");
    }
    writer.raw(b"],\"sandbox_plugin_state_hex\":[");
    for (index, state) in sandbox_states.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        let bytes = match unsafe { optional_slice(state.bytes, state.count) } {
            Some(bytes) => bytes,
            None => return false,
        };
        writer.raw(b"\"");
        writer.hex(bytes);
        writer.raw(b"\"");
    }
    writer.raw(b"],\"sidechain_routes\":[");
    let mut first_sidechain = true;
    let sidechain_routes = writer.sidechain_routes;
    for route in sidechain_routes
        .iter()
        .filter(|route| route.destination_id == track.id)
    {
        if writer.cancelled() {
            return false;
        }
        if !first_sidechain {
            writer.raw(b",");
        }
        first_sidechain = false;
        writer.raw(b"{\"source_id\":");
        writer.raw(route.source_id.to_string().as_bytes());
        writer.raw(b",\"destination_id\":");
        writer.raw(route.destination_id.to_string().as_bytes());
        writer.raw(b",\"plugin_index\":");
        writer.raw(route.plugin_index.to_string().as_bytes());
        writer.raw(b",\"tap_point\":");
        writer.raw(route.tap_point.to_string().as_bytes());
        writer.raw(b"}");
    }
    writer.raw(b"],\"feedback_routes\":[");
    let mut first_feedback = true;
    let feedback_routes = writer.feedback_routes;
    for route in feedback_routes
        .iter()
        .filter(|route| route.source_id == track.id)
    {
        if writer.cancelled() {
            return false;
        }
        if !first_feedback {
            writer.raw(b",");
        }
        first_feedback = false;
        writer.raw(b"{\"source_id\":");
        writer.raw(route.source_id.to_string().as_bytes());
        writer.raw(b",\"destination_id\":");
        writer.raw(route.destination_id.to_string().as_bytes());
        writer.raw(b",\"gain\":");
        writer.number32(route.gain);
        writer.raw(b"}");
    }
    writer.raw(b"],\"regions\":[");
    for (index, region) in regions.iter().enumerate() {
        if writer.cancelled() {
            return false;
        }
        if index > 0 {
            writer.raw(b",");
        }
        if !unsafe { write_region(writer, region) } {
            return false;
        }
    }
    writer.raw(b"]}");
    true
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 1469598103934665603u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

impl<'a> LayoutWriter<'a> {
    pub(crate) fn snapshot(&mut self, snapshot: &HirariProjectLayoutSnapshot) -> bool {
        let tracks = match unsafe { optional_slice(snapshot.tracks, snapshot.track_count) } {
            Some(tracks) => tracks,
            None => return false,
        };
        let routes =
            match unsafe { optional_slice(snapshot.sidechain_routes, snapshot.sidechain_route_count) }
            {
                Some(routes) => routes,
                None => return false,
            };
        let feedback =
            match unsafe { optional_slice(snapshot.feedback_routes, snapshot.feedback_route_count) }
            {
                Some(feedback) => feedback,
                None => return false,
            };
        self.sidechain_routes = routes;
        self.feedback_routes = feedback;
        self.raw(b"[");
        let mut serialized = 0usize;
        for track in tracks {
            if self.cancelled() {
                return false;
            }
            // The bootstrap track is a placeholder when a real project has
            // replaced it; it was never part of the saved layout.
            let is_empty_bootstrap = track.id == 0
                && unsafe { string_bytes(&track.name) } == Some(b"Audio 1".as_slice())
                && track.plugin_type_count == 0
                && track.region_count == 0
                && tracks.len() > 1;
            if is_empty_bootstrap {
                continue;
            }
            if serialized > 0 {
                self.raw(b",");
            }
            serialized += 1;
            if !unsafe { write_track(self, track) } {
                return false;
            }
        }
        self.raw(b"]");
        true
    }
}
