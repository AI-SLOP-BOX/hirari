//! Decode the remaining native-project graph and arrangement metadata.

use std::collections::HashSet;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectSidechainView {
    pub source_id: u32,
    pub destination_id: u32,
    pub plugin_index: u32,
    pub tap_point: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectRouteView {
    pub source_id: u32,
    pub destination_id: u32,
    pub gain: f32,
    pub send: u8,
    pub pre_fader: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectMarkerView {
    pub sample: u64,
    pub name: *const u8,
    pub name_size: usize,
    pub color: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectArrangerPartView {
    pub start: u64,
    pub length: u64,
    pub repeats: u32,
    pub name: *const u8,
    pub name_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectTempoEventView {
    pub sample: u64,
    pub bpm: f64,
    pub ramp: u8,
}

#[repr(C)]
pub struct HirariProjectTailView {
    pub sidechains: *const HirariProjectSidechainView,
    pub sidechain_count: usize,
    pub routes: *const HirariProjectRouteView,
    pub route_count: usize,
    pub markers: *const HirariProjectMarkerView,
    pub marker_count: usize,
    pub arranger_parts: *const HirariProjectArrangerPartView,
    pub arranger_part_count: usize,
    pub tempo_events: *const HirariProjectTempoEventView,
    pub tempo_event_count: usize,
}

pub type TailConsumer = unsafe extern "C" fn(
    context: *mut std::ffi::c_void,
    tail: *const HirariProjectTailView,
) -> bool;

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(count)?;
        let bytes = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(bytes)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(*self.take(1)?.first()?)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_ne_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_ne_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_ne_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_ne_bytes(self.take(8)?.try_into().ok()?))
    }
    fn text(&mut self, max: usize) -> Option<&'a [u8]> {
        let length = self.u32()? as usize;
        if length == 0 || length > max {
            return None;
        }
        self.take(length)
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_decode_tail(
    bytes: *const u8,
    byte_count: usize,
    offset: usize,
    version: u32,
    context: *mut std::ffi::c_void,
    consume_tail: Option<TailConsumer>,
    consumed_offset: *mut usize,
) -> bool {
    if bytes.is_null()
        || context.is_null()
        || consume_tail.is_none()
        || consumed_offset.is_null()
        || !(1..=42).contains(&version)
    {
        return false;
    }
    unsafe { *consumed_offset = 0 };
    let input = unsafe { std::slice::from_raw_parts(bytes, byte_count) };
    let mut cursor = Cursor {
        bytes: input,
        offset,
    };

    let mut sidechains = Vec::new();
    let mut sidechain_destinations = HashSet::new();
    if version >= 26 {
        let Some(count) = cursor.u32() else {
            return false;
        };
        if count > 100_000 {
            return false;
        }
        sidechains.reserve(count as usize);
        for _ in 0..count {
            let Some(source_id) = cursor.u32() else {
                return false;
            };
            let Some(destination_id) = cursor.u32() else {
                return false;
            };
            let Some(plugin_index) = cursor.u32() else {
                return false;
            };
            let Some(tap_point) = cursor.u32() else {
                return false;
            };
            let key = (u64::from(destination_id) << 32) | u64::from(plugin_index);
            if source_id == destination_id || tap_point > 2 || !sidechain_destinations.insert(key) {
                return false;
            }
            sidechains.push(HirariProjectSidechainView {
                source_id,
                destination_id,
                plugin_index,
                tap_point,
            });
        }
    }

    let mut routes = Vec::new();
    let mut route_keys = HashSet::new();
    if version >= 31 {
        let Some(count) = cursor.u32() else {
            return false;
        };
        if count > 100_000 {
            return false;
        }
        routes.reserve(count as usize);
        for _ in 0..count {
            let Some(source_id) = cursor.u32() else {
                return false;
            };
            let Some(destination_id) = cursor.u32() else {
                return false;
            };
            let Some(gain) = cursor.f32() else {
                return false;
            };
            let (send, pre_fader) = if version >= 40 {
                let Some(send) = cursor.u8() else {
                    return false;
                };
                let Some(pre_fader) = cursor.u8() else {
                    return false;
                };
                (send != 0, pre_fader != 0)
            } else {
                (false, false)
            };
            let key = ((u64::from(source_id) << 32) | u64::from(destination_id))
                ^ (u64::from(send) << 63);
            if source_id == destination_id
                || !gain.is_finite()
                || gain <= 0.0
                || gain > 2.0
                || (!send && pre_fader)
                || !route_keys.insert(key)
            {
                return false;
            }
            routes.push(HirariProjectRouteView {
                source_id,
                destination_id,
                gain,
                send: u8::from(send),
                pre_fader: u8::from(pre_fader),
            });
        }
    }

    let mut markers = Vec::new();
    let mut marker_positions = HashSet::new();
    let mut arranger_parts = Vec::new();
    let mut tempo_events = Vec::new();
    if version >= 34 {
        let Some(marker_count) = cursor.u32() else {
            return false;
        };
        if marker_count > 100_000 {
            return false;
        }
        markers.reserve(marker_count as usize);
        for _ in 0..marker_count {
            let Some(sample) = cursor.u64() else {
                return false;
            };
            let Some(color) = cursor.u32() else {
                return false;
            };
            let Some(name) = cursor.text(1024) else {
                return false;
            };
            if !marker_positions.insert(sample) {
                return false;
            }
            markers.push(HirariProjectMarkerView {
                sample,
                name: name.as_ptr(),
                name_size: name.len(),
                color,
            });
        }
        let Some(part_count) = cursor.u32() else {
            return false;
        };
        if part_count > 100_000 {
            return false;
        }
        arranger_parts.reserve(part_count as usize);
        for _ in 0..part_count {
            let Some(start) = cursor.u64() else {
                return false;
            };
            let Some(length) = cursor.u64() else {
                return false;
            };
            let Some(repeats) = cursor.u32() else {
                return false;
            };
            let Some(name) = cursor.text(1024) else {
                return false;
            };
            if length == 0 || repeats == 0 {
                return false;
            }
            arranger_parts.push(HirariProjectArrangerPartView {
                start,
                length,
                repeats,
                name: name.as_ptr(),
                name_size: name.len(),
            });
        }
        let Some(tempo_count) = cursor.u32() else {
            return false;
        };
        if tempo_count > 100_000 {
            return false;
        }
        tempo_events.reserve(tempo_count as usize);
        let mut previous_sample = 0;
        for _ in 0..tempo_count {
            let Some(sample) = cursor.u64() else {
                return false;
            };
            let Some(bpm) = cursor.f64() else {
                return false;
            };
            let Some(ramp) = cursor.u8() else {
                return false;
            };
            if !bpm.is_finite()
                || !(20.0..=300.0).contains(&bpm)
                || ramp > 1
                || sample < previous_sample
            {
                return false;
            }
            previous_sample = sample;
            tempo_events.push(HirariProjectTempoEventView { sample, bpm, ramp });
        }
    }

    let tail = HirariProjectTailView {
        sidechains: sidechains.as_ptr(),
        sidechain_count: sidechains.len(),
        routes: routes.as_ptr(),
        route_count: routes.len(),
        markers: markers.as_ptr(),
        marker_count: markers.len(),
        arranger_parts: arranger_parts.as_ptr(),
        arranger_part_count: arranger_parts.len(),
        tempo_events: tempo_events.as_ptr(),
        tempo_event_count: tempo_events.len(),
    };
    if !unsafe { consume_tail.unwrap()(context, &tail) } {
        return false;
    }
    unsafe { *consumed_offset = cursor.offset };
    true
}

fn append_u8(output: &mut Vec<u8>, value: u8) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn append_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn append_u64(output: &mut Vec<u8>, value: u64) {
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

/// Encode the versioned routing and arrangement tail of a native project.
/// The returned allocation is released with `hirari_project_free_encoded_tail`.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_encode_tail(
    version: u32,
    tail: *const HirariProjectTailView,
    output_size: *mut usize,
) -> *mut u8 {
    if !(1..=42).contains(&version) || tail.is_null() || output_size.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { *output_size = 0 };
    let tail = unsafe { &*tail };
    if (tail.sidechain_count != 0 && tail.sidechains.is_null())
        || (tail.route_count != 0 && tail.routes.is_null())
        || (tail.marker_count != 0 && tail.markers.is_null())
        || (tail.arranger_part_count != 0 && tail.arranger_parts.is_null())
        || (tail.tempo_event_count != 0 && tail.tempo_events.is_null())
    {
        return std::ptr::null_mut();
    }

    let mut output = Vec::new();
    if version >= 26 {
        let Ok(count) = u32::try_from(tail.sidechain_count) else {
            return std::ptr::null_mut();
        };
        append_u32(&mut output, count);
        for sidechain in unsafe { borrowed_slice(tail.sidechains, tail.sidechain_count) } {
            append_u32(&mut output, sidechain.source_id);
            append_u32(&mut output, sidechain.destination_id);
            append_u32(&mut output, sidechain.plugin_index);
            append_u32(&mut output, sidechain.tap_point);
        }
    }
    if version >= 31 {
        let Ok(count) = u32::try_from(tail.route_count) else {
            return std::ptr::null_mut();
        };
        append_u32(&mut output, count);
        for route in unsafe { borrowed_slice(tail.routes, tail.route_count) } {
            append_u32(&mut output, route.source_id);
            append_u32(&mut output, route.destination_id);
            append_f32(&mut output, route.gain);
            if version >= 40 {
                append_u8(&mut output, route.send);
                append_u8(&mut output, route.pre_fader);
            }
        }
    }
    if version >= 34 {
        let marker_count = tail.marker_count.min(100_000);
        append_u32(&mut output, marker_count as u32);
        for marker in unsafe { borrowed_slice(tail.markers, marker_count) } {
            if marker.name_size != 0 && marker.name.is_null() {
                return std::ptr::null_mut();
            }
            append_u64(&mut output, marker.sample);
            append_u32(&mut output, marker.color);
            let length = marker.name_size.min(1024);
            append_u32(&mut output, length as u32);
            output.extend_from_slice(unsafe { borrowed_slice(marker.name, length) });
        }
        let part_count = tail.arranger_part_count.min(100_000);
        append_u32(&mut output, part_count as u32);
        for part in unsafe { borrowed_slice(tail.arranger_parts, part_count) } {
            if part.name_size != 0 && part.name.is_null() {
                return std::ptr::null_mut();
            }
            append_u64(&mut output, part.start);
            append_u64(&mut output, part.length);
            append_u32(&mut output, part.repeats);
            let length = part.name_size.min(1024);
            append_u32(&mut output, length as u32);
            output.extend_from_slice(unsafe { borrowed_slice(part.name, length) });
        }
        let tempo_count = tail.tempo_event_count.min(100_000);
        append_u32(&mut output, tempo_count as u32);
        for event in unsafe { borrowed_slice(tail.tempo_events, tempo_count) } {
            append_u64(&mut output, event.sample);
            append_f64(&mut output, event.bpm);
            append_u8(&mut output, event.ramp);
        }
    }
    let size = output.len();
    unsafe { *output_size = size };
    Box::into_raw(output.into_boxed_slice()).cast::<u8>()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_free_encoded_tail(data: *mut u8, size: usize) {
    if !data.is_null() {
        let slice = std::ptr::slice_from_raw_parts_mut(data, size);
        drop(unsafe { Box::from_raw(slice) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_assemble_native_payload(
    version: u32,
    sample_rate: u32,
    bpm: f64,
    root_note: i32,
    scale_type: i32,
    tracks: *const u8,
    tracks_size: usize,
    regions: *const u8,
    regions_size: usize,
    tail: *const u8,
    tail_size: usize,
    output_size: *mut usize,
) -> *mut u8 {
    if !(1..=42).contains(&version)
        || output_size.is_null()
        || (tracks_size != 0 && tracks.is_null())
        || (regions_size != 0 && regions.is_null())
        || (tail_size != 0 && tail.is_null())
    {
        return std::ptr::null_mut();
    }
    unsafe { *output_size = 0 };
    let Some(total_size) = 20usize
        .checked_add(if version >= 14 { 8 } else { 0 })
        .and_then(|size| size.checked_add(tracks_size))
        .and_then(|size| size.checked_add(regions_size))
        .and_then(|size| size.checked_add(tail_size))
    else {
        return std::ptr::null_mut();
    };
    let mut output = Vec::with_capacity(total_size);
    append_u32(&mut output, 0x4155_5241);
    append_u32(&mut output, version);
    append_u32(&mut output, sample_rate);
    append_f64(&mut output, bpm);
    if version >= 14 {
        output.extend_from_slice(&root_note.to_ne_bytes());
        output.extend_from_slice(&scale_type.to_ne_bytes());
    }
    output.extend_from_slice(unsafe { borrowed_slice(tracks, tracks_size) });
    output.extend_from_slice(unsafe { borrowed_slice(regions, regions_size) });
    output.extend_from_slice(unsafe { borrowed_slice(tail, tail_size) });
    let size = output.len();
    unsafe { *output_size = size };
    Box::into_raw(output.into_boxed_slice()).cast::<u8>()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_free_assembled_payload(data: *mut u8, size: usize) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, size)) });
    }
}
