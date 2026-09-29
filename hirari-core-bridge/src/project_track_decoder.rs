//! Versioned native-project track decoding. The C++ project model is still the
//! destination type; Rust owns the binary cursor, compatibility branches, and
//! validation before each decoded track is handed across the callback.

use std::collections::HashSet;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectBlobView {
    pub data: *const u8,
    pub size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariProjectAutomationPoint {
    pub time: f64,
    pub value: f32,
    pub curve: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectPluginAutomationLaneView {
    pub plugin_index: u32,
    pub parameter_id: u32,
    pub points: *const HirariProjectAutomationPoint,
    pub point_count: usize,
}

#[repr(C)]
pub struct HirariProjectTrackView {
    pub id: u32,
    pub track_type: u32,
    pub volume: f32,
    pub pan: f32,
    pub pan3d_x: f32,
    pub pan3d_y: f32,
    pub pan3d_z: f32,
    pub muted: u8,
    pub solo: u8,
    pub phase_invert: u8,
    pub record_armed: u8,
    pub track_delay_samples: u32,
    pub plugin_name: *const u8,
    pub plugin_name_size: usize,
    pub plugin_data: *const u8,
    pub plugin_data_size: usize,
    pub plugin_states: *const HirariProjectBlobView,
    pub plugin_state_count: usize,
    pub plugin_gui_states: *const HirariProjectBlobView,
    pub plugin_gui_state_count: usize,
    pub plugin_bypass: *const u8,
    pub plugin_bypass_count: usize,
    pub sandboxed_plugin_paths: *const HirariProjectBlobView,
    pub sandboxed_plugin_path_count: usize,
    pub sandboxed_plugin_states: *const HirariProjectBlobView,
    pub sandboxed_plugin_state_count: usize,
    pub volume_automation: *const HirariProjectAutomationPoint,
    pub volume_automation_count: usize,
    pub pan_automation: *const HirariProjectAutomationPoint,
    pub pan_automation_count: usize,
    pub track_delay_automation: *const HirariProjectAutomationPoint,
    pub track_delay_automation_count: usize,
    pub plugin_automation: *const HirariProjectPluginAutomationLaneView,
    pub plugin_automation_count: usize,
}

pub type TrackConsumer = unsafe extern "C" fn(
    context: *mut std::ffi::c_void,
    track: *const HirariProjectTrackView,
) -> bool;

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
    volume_automation: Vec<HirariProjectAutomationPoint>,
    pan_automation: Vec<HirariProjectAutomationPoint>,
    track_delay_automation: Vec<HirariProjectAutomationPoint>,
    plugin_automation: Vec<(u32, u32, Vec<HirariProjectAutomationPoint>)>,
}

fn parse_automation(
    cursor: &mut Cursor<'_>,
    version: u32,
    total_points: &mut u64,
) -> Option<Vec<HirariProjectAutomationPoint>> {
    let count = cursor.u32()? as usize;
    *total_points = total_points.checked_add(count as u64)?;
    if count > 100_000 || *total_points > 4_000_000 {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let point = HirariProjectAutomationPoint {
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
                let point = HirariProjectAutomationPoint {
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

fn blob_views(blobs: &[&[u8]]) -> Vec<HirariProjectBlobView> {
    blobs
        .iter()
        .map(|blob| HirariProjectBlobView {
            data: blob.as_ptr(),
            size: blob.len(),
        })
        .collect()
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
unsafe fn borrowed_slice<'a, T>(pointer: *const T, length: usize) -> &'a [T] {
    if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pointer, length) }
    }
}
fn append_blob(output: &mut Vec<u8>, data: *const u8, size: usize) -> bool {
    let Ok(size_u32) = u32::try_from(size) else {
        return false;
    };
    if size != 0 && data.is_null() {
        return false;
    }
    append_u32(output, size_u32);
    output.extend_from_slice(unsafe { borrowed_slice(data, size) });
    true
}
fn append_automation(
    output: &mut Vec<u8>,
    points: *const HirariProjectAutomationPoint,
    count: usize,
) -> bool {
    let Ok(count_u32) = u32::try_from(count) else {
        return false;
    };
    if count != 0 && points.is_null() {
        return false;
    }
    append_u32(output, count_u32);
    for point in unsafe { borrowed_slice(points, count) } {
        append_f64(output, point.time);
        append_f32(output, point.value);
        append_f32(output, point.curve);
    }
    true
}

/// Encode the versioned native-project track records from borrowed model views.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_encode_tracks(
    version: u32,
    tracks: *const HirariProjectTrackView,
    track_count: usize,
    output_size: *mut usize,
) -> *mut u8 {
    if !(1..=42).contains(&version)
        || output_size.is_null()
        || (track_count != 0 && tracks.is_null())
    {
        return std::ptr::null_mut();
    }
    unsafe { *output_size = 0 };
    let Ok(count_u32) = u32::try_from(track_count) else {
        return std::ptr::null_mut();
    };
    let mut output = Vec::new();
    append_u32(&mut output, count_u32);
    for track in unsafe { borrowed_slice(tracks, track_count) } {
        append_u32(&mut output, track.id);
        if version >= 12 {
            append_u32(&mut output, track.track_type);
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
            append_u8(&mut output, track.muted);
            append_u8(&mut output, track.solo);
            if version >= 15 {
                append_u8(&mut output, track.phase_invert);
            }
            if version >= 23 {
                append_u8(&mut output, track.record_armed);
            }
            if version >= 29 {
                output.extend_from_slice(&track.track_delay_samples.to_ne_bytes());
            }
        }
        if !append_blob(&mut output, track.plugin_name, track.plugin_name_size)
            || !append_blob(&mut output, track.plugin_data, track.plugin_data_size)
        {
            return std::ptr::null_mut();
        }
        if version >= 24 {
            if track.plugin_state_count != 0 && track.plugin_states.is_null() {
                return std::ptr::null_mut();
            }
            let states = unsafe { borrowed_slice(track.plugin_states, track.plugin_state_count) };
            let Ok(state_count) = u32::try_from(states.len()) else {
                return std::ptr::null_mut();
            };
            append_u32(&mut output, state_count);
            for blob in states {
                if !append_blob(&mut output, blob.data, blob.size) {
                    return std::ptr::null_mut();
                }
            }
            if version >= 25 {
                if track.plugin_bypass_count != 0 && track.plugin_bypass.is_null() {
                    return std::ptr::null_mut();
                }
                let Ok(bypass_count) = u32::try_from(track.plugin_bypass_count) else {
                    return std::ptr::null_mut();
                };
                append_u32(&mut output, bypass_count);
                output.extend_from_slice(unsafe {
                    borrowed_slice(track.plugin_bypass, track.plugin_bypass_count)
                });
            }
            if version >= 32 {
                if track.plugin_gui_state_count != 0 && track.plugin_gui_states.is_null() {
                    return std::ptr::null_mut();
                }
                let gui_states = unsafe {
                    borrowed_slice(track.plugin_gui_states, track.plugin_gui_state_count)
                };
                let Ok(gui_count) = u32::try_from(gui_states.len()) else {
                    return std::ptr::null_mut();
                };
                append_u32(&mut output, gui_count);
                for blob in gui_states {
                    if !append_blob(&mut output, blob.data, blob.size) {
                        return std::ptr::null_mut();
                    }
                }
            }
        }
        if version >= 16 {
            if track.sandboxed_plugin_path_count != 0 && track.sandboxed_plugin_paths.is_null() {
                return std::ptr::null_mut();
            }
            let paths = unsafe {
                borrowed_slice(
                    track.sandboxed_plugin_paths,
                    track.sandboxed_plugin_path_count,
                )
            };
            let Ok(path_count) = u32::try_from(paths.len()) else {
                return std::ptr::null_mut();
            };
            append_u32(&mut output, path_count);
            for path in paths {
                if !append_blob(&mut output, path.data, path.size) {
                    return std::ptr::null_mut();
                }
            }
            if version >= 17 {
                if track.sandboxed_plugin_state_count != 0
                    && track.sandboxed_plugin_states.is_null()
                {
                    return std::ptr::null_mut();
                }
                let sandbox_states = unsafe {
                    borrowed_slice(
                        track.sandboxed_plugin_states,
                        track.sandboxed_plugin_state_count,
                    )
                };
                let Ok(state_count) = u32::try_from(sandbox_states.len()) else {
                    return std::ptr::null_mut();
                };
                append_u32(&mut output, state_count);
                for blob in sandbox_states {
                    if !append_blob(&mut output, blob.data, blob.size) {
                        return std::ptr::null_mut();
                    }
                }
            }
        }
        if version >= 13
            && (!append_automation(
                &mut output,
                track.volume_automation,
                track.volume_automation_count,
            ) || !append_automation(
                &mut output,
                track.pan_automation,
                track.pan_automation_count,
            ) || (version >= 30
                && !append_automation(
                    &mut output,
                    track.track_delay_automation,
                    track.track_delay_automation_count,
                )))
        {
            return std::ptr::null_mut();
        }
        if version >= 37 {
            if track.plugin_automation_count != 0 && track.plugin_automation.is_null() {
                return std::ptr::null_mut();
            }
            let lanes =
                unsafe { borrowed_slice(track.plugin_automation, track.plugin_automation_count) };
            let Ok(lane_count) = u32::try_from(lanes.len()) else {
                return std::ptr::null_mut();
            };
            append_u32(&mut output, lane_count);
            for lane in lanes {
                append_u32(&mut output, lane.plugin_index);
                append_u32(&mut output, lane.parameter_id);
                if !append_automation(&mut output, lane.points, lane.point_count) {
                    return std::ptr::null_mut();
                }
            }
        }
    }
    let size = output.len();
    unsafe { *output_size = size };
    Box::into_raw(output.into_boxed_slice()).cast::<u8>()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_free_encoded_tracks(data: *mut u8, size: usize) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, size)) });
    }
}

/// Parses all versioned track records and invokes the C++ model adapter once
/// per validated record. `consumed_offset` points to the following region
/// count in the original file buffer.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_decode_tracks(
    bytes: *const u8,
    byte_count: usize,
    header_size: u32,
    version: u32,
    expected_track_count: u32,
    context: *mut std::ffi::c_void,
    consume_track: Option<TrackConsumer>,
    consumed_offset: *mut usize,
    total_string_bytes: *mut usize,
) -> bool {
    if bytes.is_null()
        || context.is_null()
        || consume_track.is_none()
        || consumed_offset.is_null()
        || total_string_bytes.is_null()
        || !(1..=42).contains(&version)
    {
        return false;
    }
    unsafe {
        *consumed_offset = 0;
        *total_string_bytes = 0;
    }
    let input = unsafe { std::slice::from_raw_parts(bytes, byte_count) };
    let mut cursor = Cursor {
        bytes: input,
        offset: header_size as usize,
    };
    let Some(track_count) = cursor.u32() else {
        return false;
    };
    if track_count != expected_track_count || track_count > 100_000 {
        return false;
    }
    let consume_track = consume_track.unwrap();
    let mut ids = HashSet::with_capacity(track_count as usize);
    let mut total_strings = 0usize;
    let mut total_plugin_data = 0usize;
    let mut total_plugin_state_bytes = 0usize;
    let mut total_state_entries = 0u64;
    let mut total_automation_points = 0u64;
    for _ in 0..track_count {
        let Some(track) = parse_track(
            &mut cursor,
            version,
            &mut total_strings,
            &mut total_plugin_data,
            &mut total_plugin_state_bytes,
            &mut total_state_entries,
            &mut total_automation_points,
        ) else {
            return false;
        };
        if !ids.insert(track.id) {
            return false;
        }
        let plugin_states = blob_views(&track.plugin_states);
        let plugin_gui_states = blob_views(&track.plugin_gui_states);
        let sandboxed_plugin_paths = blob_views(&track.sandboxed_plugin_paths);
        let sandboxed_plugin_states = blob_views(&track.sandboxed_plugin_states);
        let plugin_automation: Vec<_> = track
            .plugin_automation
            .iter()
            .map(
                |(plugin_index, parameter_id, points)| HirariProjectPluginAutomationLaneView {
                    plugin_index: *plugin_index,
                    parameter_id: *parameter_id,
                    points: points.as_ptr(),
                    point_count: points.len(),
                },
            )
            .collect();
        let view = HirariProjectTrackView {
            id: track.id,
            track_type: track.track_type,
            volume: track.volume,
            pan: track.pan,
            pan3d_x: track.pan3d[0],
            pan3d_y: track.pan3d[1],
            pan3d_z: track.pan3d[2],
            muted: track.muted,
            solo: track.solo,
            phase_invert: track.phase_invert,
            record_armed: track.record_armed,
            track_delay_samples: track.track_delay_samples,
            plugin_name: track.plugin_name.as_ptr(),
            plugin_name_size: track.plugin_name.len(),
            plugin_data: track.plugin_data.as_ptr(),
            plugin_data_size: track.plugin_data.len(),
            plugin_states: plugin_states.as_ptr(),
            plugin_state_count: plugin_states.len(),
            plugin_gui_states: plugin_gui_states.as_ptr(),
            plugin_gui_state_count: plugin_gui_states.len(),
            plugin_bypass: track.plugin_bypass.as_ptr(),
            plugin_bypass_count: track.plugin_bypass.len(),
            sandboxed_plugin_paths: sandboxed_plugin_paths.as_ptr(),
            sandboxed_plugin_path_count: sandboxed_plugin_paths.len(),
            sandboxed_plugin_states: sandboxed_plugin_states.as_ptr(),
            sandboxed_plugin_state_count: sandboxed_plugin_states.len(),
            volume_automation: track.volume_automation.as_ptr(),
            volume_automation_count: track.volume_automation.len(),
            pan_automation: track.pan_automation.as_ptr(),
            pan_automation_count: track.pan_automation.len(),
            track_delay_automation: track.track_delay_automation.as_ptr(),
            track_delay_automation_count: track.track_delay_automation.len(),
            plugin_automation: plugin_automation.as_ptr(),
            plugin_automation_count: plugin_automation.len(),
        };
        if !unsafe { consume_track(context, &view) } {
            return false;
        }
    }
    unsafe {
        *consumed_offset = cursor.offset;
        *total_string_bytes = total_strings;
    }
    true
}
