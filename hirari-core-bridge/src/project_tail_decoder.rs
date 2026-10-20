//! Native-project routing and arrangement tail codec.

use crate::ffi::{
    NativeArrangerPartState, NativeMarkerState, NativeRouteState, NativeSidechainState,
    NativeTempoEventState,
};
use std::collections::HashSet;

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

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Decode the routing, marker, arranger, and tempo-event tail.
///
/// Returns the decoded sections and the offset just past the tail.
pub(crate) fn decode_tail(
    bytes: &[u8],
    offset: usize,
    version: u32,
) -> Option<(
    Vec<NativeSidechainState>,
    Vec<NativeRouteState>,
    Vec<NativeMarkerState>,
    Vec<NativeArrangerPartState>,
    Vec<NativeTempoEventState>,
    usize,
)> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let mut cursor = Cursor { bytes, offset };

    let mut sidechains = Vec::new();
    let mut sidechain_destinations = HashSet::new();
    if version >= 26 {
        let count = cursor.u32()?;
        if count > 100_000 {
            return None;
        }
        sidechains.reserve(count as usize);
        for _ in 0..count {
            let source_id = cursor.u32()?;
            let destination_id = cursor.u32()?;
            let plugin_index = cursor.u32()?;
            let tap_point = cursor.u32()?;
            let key = (u64::from(destination_id) << 32) | u64::from(plugin_index);
            if source_id == destination_id || tap_point > 2 || !sidechain_destinations.insert(key) {
                return None;
            }
            sidechains.push(NativeSidechainState {
                sourceId: source_id,
                destinationId: destination_id,
                pluginIndex: plugin_index,
                tapPoint: tap_point,
            });
        }
    }

    let mut routes = Vec::new();
    let mut route_keys = HashSet::new();
    if version >= 31 {
        let count = cursor.u32()?;
        if count > 100_000 {
            return None;
        }
        routes.reserve(count as usize);
        for _ in 0..count {
            let source_id = cursor.u32()?;
            let destination_id = cursor.u32()?;
            let gain = cursor.f32()?;
            let (send, pre_fader) = if version >= 40 {
                let send = cursor.u8()?;
                let pre_fader = cursor.u8()?;
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
                return None;
            }
            routes.push(NativeRouteState {
                sourceId: source_id,
                destinationId: destination_id,
                gain,
                send,
                preFader: pre_fader,
            });
        }
    }

    let mut markers = Vec::new();
    let mut marker_positions = HashSet::new();
    let mut arranger_parts = Vec::new();
    let mut tempo_events = Vec::new();
    if version >= 34 {
        let marker_count = cursor.u32()?;
        if marker_count > 100_000 {
            return None;
        }
        markers.reserve(marker_count as usize);
        for _ in 0..marker_count {
            let sample = cursor.u64()?;
            let color = cursor.u32()?;
            let name = cursor.text(1024)?;
            if !marker_positions.insert(sample) {
                return None;
            }
            markers.push(NativeMarkerState {
                sample,
                name: text(name),
                color,
            });
        }
        let part_count = cursor.u32()?;
        if part_count > 100_000 {
            return None;
        }
        arranger_parts.reserve(part_count as usize);
        for _ in 0..part_count {
            let start = cursor.u64()?;
            let length = cursor.u64()?;
            let repeats = cursor.u32()?;
            let name = cursor.text(1024)?;
            if length == 0 || repeats == 0 {
                return None;
            }
            arranger_parts.push(NativeArrangerPartState {
                start,
                length,
                name: text(name),
                repeats,
            });
        }
        let tempo_count = cursor.u32()?;
        if tempo_count > 100_000 {
            return None;
        }
        tempo_events.reserve(tempo_count as usize);
        let mut previous_sample = 0;
        for _ in 0..tempo_count {
            let sample = cursor.u64()?;
            let bpm = cursor.f64()?;
            let ramp = cursor.u8()?;
            if !bpm.is_finite()
                || !(20.0..=300.0).contains(&bpm)
                || ramp > 1
                || sample < previous_sample
            {
                return None;
            }
            previous_sample = sample;
            tempo_events.push(NativeTempoEventState { sample, bpm, ramp });
        }
    }

    Some((
        sidechains,
        routes,
        markers,
        arranger_parts,
        tempo_events,
        cursor.offset,
    ))
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
fn append_text(output: &mut Vec<u8>, value: &str) {
    let length = value.len().min(1024);
    append_u32(output, length as u32);
    output.extend_from_slice(&value.as_bytes()[..length]);
}

/// Encode the routing and arrangement tail.
pub(crate) fn encode_tail(
    version: u32,
    sidechains: &[NativeSidechainState],
    routes: &[NativeRouteState],
    markers: &[NativeMarkerState],
    arranger_parts: &[NativeArrangerPartState],
    tempo_events: &[NativeTempoEventState],
) -> Option<Vec<u8>> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let mut output = Vec::new();
    if version >= 26 {
        append_u32(&mut output, u32::try_from(sidechains.len()).ok()?);
        for sidechain in sidechains {
            append_u32(&mut output, sidechain.sourceId);
            append_u32(&mut output, sidechain.destinationId);
            append_u32(&mut output, sidechain.pluginIndex);
            append_u32(&mut output, sidechain.tapPoint);
        }
    }
    if version >= 31 {
        append_u32(&mut output, u32::try_from(routes.len()).ok()?);
        for route in routes {
            append_u32(&mut output, route.sourceId);
            append_u32(&mut output, route.destinationId);
            append_f32(&mut output, route.gain);
            if version >= 40 {
                append_u8(&mut output, u8::from(route.send));
                append_u8(&mut output, u8::from(route.preFader));
            }
        }
    }
    if version >= 34 {
        let marker_count = markers.len().min(100_000);
        append_u32(&mut output, marker_count as u32);
        for marker in markers.iter().take(marker_count) {
            append_u64(&mut output, marker.sample);
            append_u32(&mut output, marker.color);
            append_text(&mut output, &marker.name);
        }
        let part_count = arranger_parts.len().min(100_000);
        append_u32(&mut output, part_count as u32);
        for part in arranger_parts.iter().take(part_count) {
            append_u64(&mut output, part.start);
            append_u64(&mut output, part.length);
            append_u32(&mut output, part.repeats);
            append_text(&mut output, &part.name);
        }
        let tempo_count = tempo_events.len().min(100_000);
        append_u32(&mut output, tempo_count as u32);
        for event in tempo_events.iter().take(tempo_count) {
            append_u64(&mut output, event.sample);
            append_f64(&mut output, event.bpm);
            append_u8(&mut output, event.ramp);
        }
    }
    Some(output)
}

/// Assemble the `ARUA` payload: fixed header, track section, region section,
/// and tail. The checksum trailer is added by the envelope writer.
pub(crate) fn assemble_payload(
    version: u32,
    sample_rate: u32,
    bpm: f64,
    root_note: i32,
    scale_type: i32,
    tracks: &[u8],
    regions: &[u8],
    tail: &[u8],
) -> Option<Vec<u8>> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let total_size = 20usize
        .checked_add(if version >= 14 { 8 } else { 0 })
        .and_then(|size| size.checked_add(tracks.len()))
        .and_then(|size| size.checked_add(regions.len()))
        .and_then(|size| size.checked_add(tail.len()))?;
    let mut output = Vec::with_capacity(total_size);
    append_u32(&mut output, 0x4155_5241);
    append_u32(&mut output, version);
    append_u32(&mut output, sample_rate);
    append_f64(&mut output, bpm);
    if version >= 14 {
        output.extend_from_slice(&root_note.to_ne_bytes());
        output.extend_from_slice(&scale_type.to_ne_bytes());
    }
    output.extend_from_slice(tracks);
    output.extend_from_slice(regions);
    output.extend_from_slice(tail);
    Some(output)
}
