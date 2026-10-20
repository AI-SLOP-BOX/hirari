//! Versioned native-project region codec, including audio-note edits, comping
//! ranges, processing history, and warp markers.

use crate::ffi::{
    NativeNoteAnchor, NativeNoteSegment, NativeProcessingStep, NativeRangeEdit, NativeRegionState,
    NativeWarpMarker,
};
use std::collections::HashSet;

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, size: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(size)?;
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
    fn blob(&mut self, max_size: usize) -> Option<&'a [u8]> {
        let size = self.u32()? as usize;
        if size > max_size {
            return None;
        }
        self.take(size)
    }
}

struct NoteSegment {
    start: f64,
    end: f64,
    pitch: f64,
    pitch_offset: f64,
    formant_offset: f64,
    anchors: Vec<NativeNoteAnchor>,
}

struct EventStep<'a> {
    id: u32,
    operation: &'a [u8],
    parameter: f32,
    enabled: u8,
}

struct Region<'a> {
    id: u32,
    track_id: u32,
    position: u64,
    length: u64,
    source_length: u64,
    source_rate: u32,
    source_offset: u64,
    base_start: u64,
    base_source_offset: u64,
    base_length: u64,
    muted: u8,
    file_path: &'a [u8],
    name: &'a [u8],
    clip_gain: f32,
    fade_in: u64,
    fade_out: u64,
    reverse: u8,
    warp_ratio: f64,
    pitch_preserve: u8,
    pitch_semitones: f32,
    loop_count: u32,
    locked: u8,
    sync_group: u32,
    range_edits: Vec<NativeRangeEdit>,
    processing_history: Vec<EventStep<'a>>,
    note_segments: Vec<NoteSegment>,
    warp_markers: Vec<NativeWarpMarker>,
}

fn parse_region<'a>(
    cursor: &mut Cursor<'a>,
    version: u32,
    sample_rate: u32,
    track_ids: &HashSet<u32>,
    region_ids: &mut HashSet<u32>,
    total_strings: &mut usize,
) -> Option<Region<'a>> {
    let mut region = Region {
        id: 0,
        track_id: 0,
        position: 0,
        length: 0,
        source_length: 0,
        source_rate: 0,
        source_offset: 0,
        base_start: 0,
        base_source_offset: 0,
        base_length: 0,
        muted: 0,
        file_path: &[],
        name: &[],
        clip_gain: 1.0,
        fade_in: 64,
        fade_out: 64,
        reverse: 0,
        warp_ratio: 1.0,
        pitch_preserve: 0,
        pitch_semitones: 0.0,
        loop_count: 1,
        locked: 0,
        sync_group: 0,
        range_edits: Vec::new(),
        processing_history: Vec::new(),
        note_segments: Vec::new(),
        warp_markers: Vec::new(),
    };
    if version >= 28 {
        region.id = cursor.u32()?;
        if region.id == 0 || !region_ids.insert(region.id) {
            return None;
        }
    }
    region.track_id = cursor.u32()?;
    region.position = cursor.u64()?;
    region.length = cursor.u64()?;
    if !track_ids.contains(&region.track_id) || region.length == 0 {
        return None;
    }
    if version >= 19 {
        region.source_offset = cursor.u64()?;
        region.base_start = cursor.u64()?;
        region.base_source_offset = cursor.u64()?;
        region.base_length = cursor.u64()?;
        if region.source_offset.checked_add(region.length).is_none()
            || region
                .base_source_offset
                .checked_add(region.base_length)
                .is_none()
        {
            return None;
        }
    }
    region.muted = cursor.u8()?;
    region.file_path = cursor.blob(16 * 1024 * 1024)?;
    region.name = cursor.blob(16 * 1024 * 1024)?;
    *total_strings = total_strings.checked_add(region.file_path.len())?;
    *total_strings = total_strings.checked_add(region.name.len())?;
    if *total_strings > 64 * 1024 * 1024
        || region.file_path.is_empty()
        || region.position.checked_add(region.length).is_none()
    {
        return None;
    }
    if version >= 11 {
        region.clip_gain = cursor.f32()?;
        region.fade_in = cursor.u64()?;
        region.fade_out = cursor.u64()?;
        if !region.clip_gain.is_finite()
            || !(0.0..=2.0).contains(&region.clip_gain)
            || region.fade_in > region.length
            || region.fade_out > region.length
        {
            return None;
        }
        if version >= 18 {
            region.reverse = cursor.u8()?;
        }
        if version >= 20 {
            region.warp_ratio = cursor.f64()?;
            if !region.warp_ratio.is_finite() || !(0.5..=2.0).contains(&region.warp_ratio) {
                return None;
            }
        }
        if version >= 21 {
            region.pitch_semitones = cursor.f32()?;
            if !region.pitch_semitones.is_finite()
                || !(-24.0..=24.0).contains(&region.pitch_semitones)
            {
                return None;
            }
        }
        if version >= 38 {
            region.pitch_preserve = cursor.u8()?;
            if region.pitch_preserve > 1 {
                return None;
            }
        }
        if version >= 39 {
            region.source_length = cursor.u64()?;
            if region.source_length == 0
                || region
                    .source_offset
                    .checked_add(region.source_length)
                    .is_none()
            {
                return None;
            }
        } else {
            let legacy_length = region.length;
            region.source_length = legacy_length;
            let duration = (legacy_length as f64 / region.warp_ratio).ceil();
            if !duration.is_finite() || duration < 1.0 || duration >= 18_446_744_073_709_551_616.0 {
                return None;
            }
            region.length = duration as u64;
            if region.position.checked_add(region.length).is_none() {
                return None;
            }
        }
        if version >= 42 {
            region.source_rate = cursor.u32()?;
            if !(8_000..=384_000).contains(&region.source_rate) {
                return None;
            }
        } else {
            region.source_rate = sample_rate;
        }
        if version >= 22 {
            region.loop_count = cursor.u32()?;
            if region.loop_count == 0
                || region.loop_count > 1024
                || region
                    .length
                    .checked_mul(u64::from(region.loop_count))
                    .is_none()
                || region
                    .position
                    .checked_add(region.length * u64::from(region.loop_count))
                    .is_none()
            {
                return None;
            }
        }
        if version >= 33 {
            let segment_count = cursor.u32()? as usize;
            if segment_count > 100_000 {
                return None;
            }
            region.note_segments.reserve(segment_count);
            for _ in 0..segment_count {
                let start = cursor.f64()?;
                let end = cursor.f64()?;
                let pitch = cursor.f64()?;
                let pitch_offset = cursor.f64()?;
                let formant_offset = cursor.f64()?;
                let anchor_count = cursor.u32()? as usize;
                if anchor_count > 10_000
                    || !start.is_finite()
                    || !end.is_finite()
                    || end <= start
                    || !pitch.is_finite()
                    || !pitch_offset.is_finite()
                    || !formant_offset.is_finite()
                {
                    return None;
                }
                let mut anchors = Vec::with_capacity(anchor_count);
                for _ in 0..anchor_count {
                    let anchor = NativeNoteAnchor {
                        positionSeconds: cursor.f64()?,
                        pitchCents: cursor.f64()?,
                        formantCents: cursor.f64()?,
                    };
                    if !anchor.positionSeconds.is_finite()
                        || !anchor.pitchCents.is_finite()
                        || !anchor.formantCents.is_finite()
                        || anchor.positionSeconds < start
                        || anchor.positionSeconds > end
                    {
                        return None;
                    }
                    anchors.push(anchor);
                }
                region.note_segments.push(NoteSegment {
                    start,
                    end,
                    pitch,
                    pitch_offset,
                    formant_offset,
                    anchors,
                });
            }
        }
        if version >= 35 {
            region.locked = cursor.u8()?;
            region.sync_group = cursor.u32()?;
        }
        if version >= 35 {
            let count = cursor.u32()? as usize;
            if count > 4096 {
                return None;
            }
            region.processing_history.reserve(count);
            for _ in 0..count {
                let id = cursor.u32()?;
                let parameter = cursor.f32()?;
                let enabled = cursor.u8()?;
                let operation = cursor.blob(256)?;
                if operation.is_empty() {
                    return None;
                }
                region.processing_history.push(EventStep {
                    id,
                    operation,
                    parameter,
                    enabled,
                });
            }
        }
        if version >= 36 {
            let count = cursor.u32()? as usize;
            if count > 4096 {
                return None;
            }
            region.range_edits.reserve(count);
            for _ in 0..count {
                let edit = NativeRangeEdit {
                    start: cursor.u64()?,
                    end: cursor.u64()?,
                    gain: cursor.f32()?,
                    fadeIn: cursor.u64()?,
                    fadeOut: cursor.u64()?,
                };
                let length = edit.end.checked_sub(edit.start)?;
                if length == 0
                    || !edit.gain.is_finite()
                    || !(0.0..=16.0).contains(&edit.gain)
                    || edit.fadeIn > length
                    || edit.fadeOut > length
                {
                    return None;
                }
                region.range_edits.push(edit);
            }
        }
        if version >= 41 {
            let count = cursor.u32()? as usize;
            if count > 100_000 {
                return None;
            }
            region.warp_markers.reserve(count);
            for _ in 0..count {
                region.warp_markers.push(NativeWarpMarker {
                    sourceSample: cursor.u64()?,
                    timelineSample: cursor.u64()?,
                    transient: cursor.u8()? != 0,
                });
            }
        }
    }
    Some(region)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn owned_region(region: Region<'_>) -> NativeRegionState {
    NativeRegionState {
        id: region.id,
        trackId: region.track_id,
        samplePosition: region.position,
        sampleLength: region.length,
        sourceLength: region.source_length,
        sourceSampleRate: region.source_rate,
        sourceOffset: region.source_offset,
        baseStart: region.base_start,
        baseSourceOffset: region.base_source_offset,
        baseLength: region.base_length,
        isMuted: region.muted != 0,
        filePath: text(region.file_path),
        name: text(region.name),
        clipGain: region.clip_gain,
        fadeInSamples: region.fade_in,
        fadeOutSamples: region.fade_out,
        reverse: region.reverse != 0,
        warpRatio: region.warp_ratio,
        pitchPreserveWarp: region.pitch_preserve != 0,
        pitchSemitones: region.pitch_semitones,
        loopCount: region.loop_count,
        locked: region.locked != 0,
        syncGroup: region.sync_group,
        rangeEdits: region.range_edits,
        processingHistory: region
            .processing_history
            .into_iter()
            .map(|step| NativeProcessingStep {
                id: step.id,
                operation: text(step.operation),
                parameter: step.parameter,
                enabled: step.enabled != 0,
            })
            .collect(),
        audioNoteSegments: region
            .note_segments
            .into_iter()
            .map(|segment| NativeNoteSegment {
                startSeconds: segment.start,
                endSeconds: segment.end,
                detectedPitchCents: segment.pitch,
                pitchOffsetCents: segment.pitch_offset,
                formantOffsetCents: segment.formant_offset,
                anchors: segment.anchors,
            })
            .collect(),
        warpMarkers: region.warp_markers,
    }
}

/// Decode the region section starting at `offset`.
pub(crate) fn decode_regions(
    bytes: &[u8],
    offset: usize,
    version: u32,
    sample_rate: u32,
    track_ids: &[u32],
    initial_string_bytes: usize,
) -> Option<(Vec<NativeRegionState>, usize)> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let mut cursor = Cursor { bytes, offset };
    let region_count = cursor.u32()?;
    if region_count > 1_000_000 {
        return None;
    }
    let known_tracks: HashSet<u32> = track_ids.iter().copied().collect();
    let mut region_ids = HashSet::with_capacity(region_count as usize);
    let mut total_strings = initial_string_bytes;
    let mut regions = Vec::with_capacity(region_count.min(1024) as usize);
    for _ in 0..region_count {
        let region = parse_region(
            &mut cursor,
            version,
            sample_rate,
            &known_tracks,
            &mut region_ids,
            &mut total_strings,
        )?;
        regions.push(owned_region(region));
    }
    Some((regions, cursor.offset))
}

fn encode_u8(output: &mut Vec<u8>, value: u8) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn encode_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn encode_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn encode_f32(output: &mut Vec<u8>, value: f32) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn encode_f64(output: &mut Vec<u8>, value: f64) {
    output.extend_from_slice(&value.to_ne_bytes());
}
fn encode_text(output: &mut Vec<u8>, value: &str) -> bool {
    let Ok(size) = u32::try_from(value.len()) else {
        return false;
    };
    encode_u32(output, size);
    output.extend_from_slice(value.as_bytes());
    true
}

/// Encode the region section, including its leading region count.
pub(crate) fn encode_regions(
    version: u32,
    sample_rate: u32,
    regions: &[NativeRegionState],
) -> Option<Vec<u8>> {
    if !(1..=42).contains(&version) {
        return None;
    }
    let count = u32::try_from(regions.len()).ok()?;
    let mut output = Vec::new();
    encode_u32(&mut output, count);
    for region in regions {
        if version >= 28 {
            encode_u32(&mut output, region.id);
        }
        encode_u32(&mut output, region.trackId);
        encode_u64(&mut output, region.samplePosition);
        encode_u64(&mut output, region.sampleLength);
        if version >= 19 {
            encode_u64(&mut output, region.sourceOffset);
            encode_u64(&mut output, region.baseStart);
            encode_u64(&mut output, region.baseSourceOffset);
            encode_u64(&mut output, region.baseLength);
        }
        encode_u8(&mut output, u8::from(region.isMuted));
        if !encode_text(&mut output, &region.filePath) || !encode_text(&mut output, &region.name) {
            return None;
        }
        encode_f32(&mut output, region.clipGain);
        encode_u64(&mut output, region.fadeInSamples);
        encode_u64(&mut output, region.fadeOutSamples);
        if version >= 18 {
            encode_u8(&mut output, u8::from(region.reverse));
        }
        if version >= 20 {
            encode_f64(&mut output, region.warpRatio);
        }
        if version >= 21 {
            encode_f32(&mut output, region.pitchSemitones);
        }
        if version >= 38 {
            encode_u8(&mut output, u8::from(region.pitchPreserveWarp));
        }
        if version >= 39 {
            encode_u64(
                &mut output,
                if region.sourceLength == 0 {
                    region.sampleLength
                } else {
                    region.sourceLength
                },
            );
        }
        if version >= 42 {
            encode_u32(
                &mut output,
                if region.sourceSampleRate == 0 {
                    sample_rate
                } else {
                    region.sourceSampleRate
                },
            );
        }
        if version >= 22 {
            encode_u32(&mut output, region.loopCount);
        }
        if version >= 33 {
            let segment_count = region.audioNoteSegments.len().min(100_000);
            encode_u32(&mut output, segment_count as u32);
            for segment in region.audioNoteSegments.iter().take(segment_count) {
                encode_f64(&mut output, segment.startSeconds);
                encode_f64(&mut output, segment.endSeconds);
                encode_f64(&mut output, segment.detectedPitchCents);
                encode_f64(&mut output, segment.pitchOffsetCents);
                encode_f64(&mut output, segment.formantOffsetCents);
                let anchor_count = segment.anchors.len().min(10_000);
                encode_u32(&mut output, anchor_count as u32);
                for anchor in segment.anchors.iter().take(anchor_count) {
                    encode_f64(&mut output, anchor.positionSeconds);
                    encode_f64(&mut output, anchor.pitchCents);
                    encode_f64(&mut output, anchor.formantCents);
                }
            }
        }
        if version >= 35 {
            encode_u8(&mut output, u8::from(region.locked));
            encode_u32(&mut output, region.syncGroup);
            let step_count = region.processingHistory.len().min(4096);
            encode_u32(&mut output, step_count as u32);
            for step in region.processingHistory.iter().take(step_count) {
                let operation_size = step.operation.len().min(256);
                let operation = &step.operation.as_bytes()[..operation_size];
                encode_u32(&mut output, step.id);
                encode_f32(&mut output, step.parameter);
                encode_u8(&mut output, u8::from(step.enabled));
                encode_u32(&mut output, operation_size as u32);
                output.extend_from_slice(operation);
            }
        }
        if version >= 36 {
            let valid_edits: Vec<&NativeRangeEdit> = region
                .rangeEdits
                .iter()
                .filter(|edit| {
                    edit.start < edit.end
                        && edit.gain.is_finite()
                        && (0.0..=16.0).contains(&edit.gain)
                        && edit.fadeIn <= edit.end - edit.start
                        && edit.fadeOut <= edit.end - edit.start
                })
                .take(4096)
                .collect();
            encode_u32(&mut output, valid_edits.len() as u32);
            for edit in valid_edits {
                encode_u64(&mut output, edit.start);
                encode_u64(&mut output, edit.end);
                encode_f32(&mut output, edit.gain);
                encode_u64(&mut output, edit.fadeIn);
                encode_u64(&mut output, edit.fadeOut);
            }
        }
        if version >= 41 {
            let marker_count = region.warpMarkers.len().min(100_000);
            encode_u32(&mut output, marker_count as u32);
            for marker in region.warpMarkers.iter().take(marker_count) {
                encode_u64(&mut output, marker.sourceSample);
                encode_u64(&mut output, marker.timelineSample);
                encode_u8(&mut output, u8::from(marker.transient));
            }
        }
    }
    Some(output)
}
