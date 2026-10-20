//! Versioned native-project track codec.
//!
//! Rust owns the binary cursor, compatibility branches, validation, and the
//! encoded byte layout. The engine adapter only produces and consumes the
//! owned [`NativeTrackState`] records from the bridge.

use crate::ffi::{
    NativeAutomationPoint, NativeBlob, NativePluginAutomationLane, NativeTrackState,
};
use std::collections::HashSet;

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, size: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(size)?;
        let value = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(value)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_ne_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_ne_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_ne_bytes(self.take(8)?.try_into().ok()?))
    }

    fn blob(&mut self, maximum: usize) -> Option<&'a [u8]> {
        let size = self.u32()? as usize;
        if size > maximum {
            return None;
        }
        self.take(size)
    }
}

#[derive(Default)]
struct TrackRecord<'a> {
    id: u32,
    track_type: u32,
    volume: f32,
    pan: f32,
    pan3d: [f32; 3],
    muted: u8,
    solo: u8,
    phase_invert: u8,
    record_armed: u8,
    track_delay_samples: u32,
    plugin_name: &'a [u8],
    plugin_data: &'a [u8],
    plugin_states: Vec<&'a [u8]>,
    plugin_gui_states: Vec<&'a [u8]>,
    plugin_bypass: Vec<u8>,
    sandboxed_plugin_paths: Vec<&'a [u8]>,
    sandboxed_plugin_states: Vec<&'a [u8]>,
    volume_automation: Vec<NativeAutomationPoint>,
    pan_automation: Vec<NativeAutomationPoint>,
    track_delay_automation: Vec<NativeAutomationPoint>,
    plugin_automation: Vec<(u32, u32, Vec<NativeAutomationPoint>)>,
}

fn parse_automation(
    cursor: &mut Cursor<'_>,
    version: u32,
    total_points: &mut u64,
) -> Option<Vec<NativeAutomationPoint>> {
    let count = cursor.u32()? as usize;
    *total_points = total_points.checked_add(count as u64)?;
    if count > 100_000 || *total_points > 4_000_000 {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let point = NativeAutomationPoint {
            time: cursor.f64()?,
            value: cursor.f32()?,
            curve: cursor.f32()?,
        };
        if !point.time.is_finite()
            || point.time < 0.0
            || point.time > 9_007_199_254_740_992.0
            || (version >= 30 && point.time.trunc() != point.time)
            || !point.value.is_finite()
            || !point.curve.is_finite()
        {
            return None;
        }
        points.push(point);
    }
    Some(points)
}

fn parse_track<'a>(
    cursor: &mut Cursor<'a>,
    version: u32,
    total_strings: &mut usize,
    total_plugin_data: &mut usize,
    total_plugin_state_bytes: &mut usize,
    total_state_entries: &mut u64,
    total_automation_points: &mut u64,
) -> Option<TrackRecord<'a>> {
    let mut track = TrackRecord::default();
    track.id = cursor.u32()?;
    if version >= 12 {
        track.track_type = cursor.u32()?;
    }
    track.volume = cursor.f32()?;
    track.pan = cursor.f32()?;
    for position in &mut track.pan3d {
        *position = cursor.f32()?;
    }
    if version >= 12 {
        track.muted = cursor.u8()?;
        track.solo = cursor.u8()?;
        if version >= 15 {
            track.phase_invert = cursor.u8()?;
        }
        if version >= 23 {
            track.record_armed = cursor.u8()?;
        }
        if version >= 29 {
            track.track_delay_samples = cursor.u32()?;
        }
    }
    if !track.volume.is_finite()
        || !(0.0..=2.0).contains(&track.volume)
        || !track.pan.is_finite()
        || !(-1.0..=1.0).contains(&track.pan)
        || track.pan3d.iter().any(|value| !value.is_finite())
        || track.track_delay_samples > 8192
        || (version >= 12 && track.track_type > 4)
    {
        return None;
    }

    track.plugin_name = cursor.blob(16 * 1024 * 1024)?;
    *total_strings = total_strings.checked_add(track.plugin_name.len())?;
    if *total_strings > 64 * 1024 * 1024 {
        return None;
    }
    track.plugin_data = cursor.blob(64 * 1024 * 1024)?;
    *total_plugin_data = total_plugin_data.checked_add(track.plugin_data.len())?;
    if *total_plugin_data > 128 * 1024 * 1024 {
        return None;
    }

    if version >= 24 {
        let state_count = cursor.u32()? as usize;
        *total_state_entries = total_state_entries.checked_add(state_count as u64)?;
        if state_count > 4096 || *total_state_entries > 1_000_000 {
            return None;
        }
        track.plugin_states.reserve(state_count);
        for _ in 0..state_count {
            let blob = cursor.blob(16 * 1024 * 1024)?;
            *total_plugin_state_bytes = total_plugin_state_bytes.checked_add(blob.len())?;
            if *total_plugin_state_bytes > 128 * 1024 * 1024 {
                return None;
            }
            track.plugin_states.push(blob);
        }
        if version >= 25 {
            let bypass_count = cursor.u32()? as usize;
            if bypass_count > 4096 {
                return None;
            }
            track.plugin_bypass.reserve(bypass_count);
            for _ in 0..bypass_count {
                let bypass = cursor.u8()?;
                if bypass > 1 {
                    return None;
                }
                track.plugin_bypass.push(bypass);
            }
        }
        if version >= 32 {
            let gui_count = cursor.u32()? as usize;
            if gui_count > 4096 || gui_count != state_count {
                return None;
            }
            track.plugin_gui_states.reserve(gui_count);
            for _ in 0..gui_count {
                track.plugin_gui_states.push(cursor.blob(1024 * 1024)?);
            }
        }
    }

    if version >= 16 {
        let sandbox_count = cursor.u32()? as usize;
        if sandbox_count > 4096 {
            return None;
        }
        track.sandboxed_plugin_paths.reserve(sandbox_count);
        for _ in 0..sandbox_count {
            let path = cursor.blob(16 * 1024 * 1024)?;
            *total_strings = total_strings.checked_add(path.len())?;
            if path.is_empty() || *total_strings > 64 * 1024 * 1024 {
                return None;
            }
            track.sandboxed_plugin_paths.push(path);
        }
        if version >= 17 {
            let state_count = cursor.u32()? as usize;
            *total_state_entries = total_state_entries.checked_add(state_count as u64)?;
            if state_count > 4096
                || state_count != sandbox_count
                || *total_state_entries > 1_000_000
            {
                return None;
            }
            track.sandboxed_plugin_states.reserve(state_count);
            for _ in 0..state_count {
                let blob = cursor.blob(16 * 1024 * 1024)?;
                *total_plugin_state_bytes = total_plugin_state_bytes.checked_add(blob.len())?;
                if *total_plugin_state_bytes > 128 * 1024 * 1024 {
                    return None;
                }
                track.sandboxed_plugin_states.push(blob);
            }
        }
    }

    if version >= 13 {
        track.volume_automation = parse_automation(cursor, version, total_automation_points)?;
        track.pan_automation = parse_automation(cursor, version, total_automation_points)?;
        if version >= 30 {
            track.track_delay_automation =
                parse_automation(cursor, version, total_automation_points)?;
        }
    }
    if version >= 37 {
        let lane_count = cursor.u32()? as usize;
        *total_state_entries = total_state_entries.checked_add(lane_count as u64)?;
        if lane_count > 65_536 || *total_state_entries > 1_000_000 {
            return None;
        }
        track.plugin_automation.reserve(lane_count);
        for _ in 0..lane_count {
            let plugin_index = cursor.u32()?;
            let parameter_id = cursor.u32()?;
            let point_count = cursor.u32()? as usize;
            *total_automation_points = total_automation_points.checked_add(point_count as u64)?;
            if point_count > 100_000 || *total_automation_points > 4_000_000 {
                return None;
            }
            let mut points = Vec::with_capacity(point_count);
            let mut previous_time = -1.0;
            for _ in 0..point_count {
                let point = NativeAutomationPoint {
                    time: cursor.f64()?,
                    value: cursor.f32()?,
                    curve: cursor.f32()?,
                };
                if !point.time.is_finite()
                    || point.time < 0.0
                    || point.time > 9_007_199_254_740_992.0
                    || point.time.trunc() != point.time
                    || point.time <= previous_time
                    || !point.value.is_finite()
                    || !(0.0..=1.0).contains(&point.value)
                    || !point.curve.is_finite()
                    || !(-1.0..=1.0).contains(&point.curve)
                {
                    return None;
                }
                previous_time = point.time;
                points.push(point);
            }
            track
                .plugin_automation
                .push((plugin_index, parameter_id, points));
        }
    }
    Some(track)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn owned_track(track: TrackRecord<'_>) -> NativeTrackState {
    NativeTrackState {
        id: track.id,
        trackType: track.track_type,
        volume: track.volume,
        pan: track.pan,
        pan3d_x: track.pan3d[0],
        pan3d_y: track.pan3d[1],
        pan3d_z: track.pan3d[2],
        muted: track.muted != 0,
        solo: track.solo != 0,
        phaseInvert: track.phase_invert != 0,
        recordArmed: track.record_armed != 0,
        trackDelaySamples: track.track_delay_samples,
        pluginName: text(track.plugin_name),
        pluginData: track.plugin_data.to_vec(),
        pluginStates: track
            .plugin_states
            .iter()
            .map(|blob| NativeBlob { bytes: blob.to_vec() })
            .collect(),
        pluginGuiStates: track
            .plugin_gui_states
            .iter()
            .map(|blob| NativeBlob { bytes: blob.to_vec() })
            .collect(),
        pluginBypass: track.plugin_bypass,
        sandboxedPluginPaths: track
            .sandboxed_plugin_paths
            .iter()
            .map(|path| text(path))
            .collect(),
        sandboxedPluginStates: track
            .sandboxed_plugin_states
            .iter()
            .map(|blob| NativeBlob { bytes: blob.to_vec() })
            .collect(),
        volumeAutomation: track.volume_automation,
        panAutomation: track.pan_automation,
        trackDelayAutomation: track.track_delay_automation,
        pluginAutomation: track
            .plugin_automation
            .into_iter()
            .map(|(plugin_index, parameter_id, points)| NativePluginAutomationLane {
                pluginIndex: plugin_index,
                parameterId: parameter_id,
                points,
            })
            .collect(),
    }
}

fn append_u8(output: &mut Vec<u8>, value: u8) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn append_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn append_f32(output: &mut Vec<u8>, value: f32) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn append_f64(output: &mut Vec<u8>, value: f64) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn append_blob(output: &mut Vec<u8>, data: &[u8]) -> bool {
    let Ok(size) = u32::try_from(data.len()) else {
        return false;
    };
    append_u32(output, size);
    output.extend_from_slice(data);
    true
}
fn append_automation(output: &mut Vec<u8>, points: &[NativeAutomationPoint]) -> bool {
    let Ok(count) = u32::try_from(points.len()) else {
        return false;
    };
    append_u32(output, count);
    for point in points {
        append_f64(output, point.time);
        append_f32(output, point.value);
        append_f32(output, point.curve);
    }
    true
}

/// Encode the versioned track section, including its leading track count.
pub(crate) fn encode_tracks(version: u32, tracks: &[NativeTrackState]) -> Option<Vec<u8>> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let count = u32::try_from(tracks.len()).ok()?;
    let mut output = Vec::new();
    append_u32(&mut output, count);
    for track in tracks {
        append_u32(&mut output, track.id);
        if version >= 12 {
            append_u32(&mut output, track.trackType);
        }
        for value in [
            track.volume,
            track.pan,
            track.pan3d_x,
            track.pan3d_y,
            track.pan3d_z,
        ] {
            append_f32(&mut output, value);
        }
        if version >= 12 {
            append_u8(&mut output, u8::from(track.muted));
            append_u8(&mut output, u8::from(track.solo));
            if version >= 15 {
                append_u8(&mut output, u8::from(track.phaseInvert));
            }
            if version >= 23 {
                append_u8(&mut output, u8::from(track.recordArmed));
            }
            if version >= 29 {
                append_u32(&mut output, track.trackDelaySamples);
            }
        }
        if !append_blob(&mut output, track.pluginName.as_bytes())
            || !append_blob(&mut output, &track.pluginData)
        {
            return None;
        }
        if version >= 24 {
            let Ok(state_count) = u32::try_from(track.pluginStates.len()) else {
                return None;
            };
            append_u32(&mut output, state_count);
            for blob in &track.pluginStates {
                if !append_blob(&mut output, &blob.bytes) {
                    return None;
                }
            }
            if version >= 25 {
                let Ok(bypass_count) = u32::try_from(track.pluginBypass.len()) else {
                    return None;
                };
                append_u32(&mut output, bypass_count);
                output.extend_from_slice(&track.pluginBypass);
            }
            if version >= 32 {
                let Ok(gui_count) = u32::try_from(track.pluginGuiStates.len()) else {
                    return None;
                };
                append_u32(&mut output, gui_count);
                for blob in &track.pluginGuiStates {
                    if !append_blob(&mut output, &blob.bytes) {
                        return None;
                    }
                }
            }
        }
        if version >= 16 {
            let Ok(path_count) = u32::try_from(track.sandboxedPluginPaths.len()) else {
                return None;
            };
            append_u32(&mut output, path_count);
            for path in &track.sandboxedPluginPaths {
                if !append_blob(&mut output, path.as_bytes()) {
                    return None;
                }
            }
            if version >= 17 {
                let Ok(state_count) = u32::try_from(track.sandboxedPluginStates.len()) else {
                    return None;
                };
                append_u32(&mut output, state_count);
                for blob in &track.sandboxedPluginStates {
                    if !append_blob(&mut output, &blob.bytes) {
                        return None;
                    }
                }
            }
        }
        if version >= 13
            && (!append_automation(&mut output, &track.volumeAutomation)
                || !append_automation(&mut output, &track.panAutomation)
                || (version >= 30
                    && !append_automation(&mut output, &track.trackDelayAutomation)))
        {
            return None;
        }
        if version >= 37 {
            let Ok(lane_count) = u32::try_from(track.pluginAutomation.len()) else {
                return None;
            };
            append_u32(&mut output, lane_count);
            for lane in &track.pluginAutomation {
                append_u32(&mut output, lane.pluginIndex);
                append_u32(&mut output, lane.parameterId);
                if !append_automation(&mut output, &lane.points) {
                    return None;
                }
            }
        }
    }
    Some(output)
}

/// Decode every versioned track record.
///
/// Returns the decoded records, the offset of the following region count in
/// `bytes`, and the total string bytes consumed so far.
pub(crate) fn decode_tracks(
    bytes: &[u8],
    header_size: u32,
    version: u32,
    expected_track_count: u32,
) -> Option<(Vec<NativeTrackState>, usize, usize)> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let mut cursor = Cursor {
        bytes,
        offset: header_size as usize,
    };
    let track_count = cursor.u32()?;
    if track_count != expected_track_count || track_count > 100_000 {
        return None;
    }
    let mut ids = HashSet::with_capacity(track_count as usize);
    let mut total_strings = 0usize;
    let mut total_plugin_data = 0usize;
    let mut total_plugin_state_bytes = 0usize;
    let mut total_state_entries = 0u64;
    let mut total_automation_points = 0u64;
    let mut tracks = Vec::with_capacity(track_count as usize);
    for _ in 0..track_count {
        let track = parse_track(
            &mut cursor,
            version,
            &mut total_strings,
            &mut total_plugin_data,
            &mut total_plugin_state_bytes,
            &mut total_state_entries,
            &mut total_automation_points,
        )?;
        if !ids.insert(track.id) {
            return None;
        }
        tracks.push(owned_track(track));
    }
    Some((tracks, cursor.offset, total_strings))
}
