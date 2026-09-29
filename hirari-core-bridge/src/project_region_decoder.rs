//! Versioned native-project region decoding, including audio-note edits,
//! comping ranges, processing history, and warp markers.

use std::collections::HashSet;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectRangeEditView {
    pub start: u64,
    pub end: u64,
    pub gain: f32,
    pub fade_in: u64,
    pub fade_out: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectWarpMarkerView {
    pub source_sample: u64,
    pub timeline_sample: u64,
    pub transient: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectEventStepView {
    pub id: u32,
    pub operation: *const u8,
    pub operation_size: usize,
    pub parameter: f32,
    pub enabled: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariProjectAudioNoteAnchorView {
    pub position_seconds: f64,
    pub pitch_cents: f64,
    pub formant_cents: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariProjectAudioNoteSegmentView {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub detected_pitch_cents: f64,
    pub pitch_offset_cents: f64,
    pub formant_offset_cents: f64,
    pub anchors: *const HirariProjectAudioNoteAnchorView,
    pub anchor_count: usize,
}

#[repr(C)]
pub struct HirariProjectRegionView {
    pub id: u32,
    pub track_id: u32,
    pub sample_position: u64,
    pub sample_length: u64,
    pub source_length: u64,
    pub source_sample_rate: u32,
    pub source_offset: u64,
    pub base_start: u64,
    pub base_source_offset: u64,
    pub base_length: u64,
    pub muted: u8,
    pub file_path: *const u8,
    pub file_path_size: usize,
    pub name: *const u8,
    pub name_size: usize,
    pub clip_gain: f32,
    pub fade_in_samples: u64,
    pub fade_out_samples: u64,
    pub reverse: u8,
    pub warp_ratio: f64,
    pub pitch_preserve_warp: u8,
    pub pitch_semitones: f32,
    pub loop_count: u32,
    pub locked: u8,
    pub sync_group: u32,
    pub range_edits: *const HirariProjectRangeEditView,
    pub range_edit_count: usize,
    pub processing_history: *const HirariProjectEventStepView,
    pub processing_history_count: usize,
    pub audio_note_segments: *const HirariProjectAudioNoteSegmentView,
    pub audio_note_segment_count: usize,
    pub warp_markers: *const HirariProjectWarpMarkerView,
    pub warp_marker_count: usize,
}

pub type RegionConsumer = unsafe extern "C" fn(
    context: *mut std::ffi::c_void,
    region: *const HirariProjectRegionView,
) -> bool;

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

struct NoteSegment<'a> {
    start: f64,
    end: f64,
    pitch: f64,
    pitch_offset: f64,
    formant_offset: f64,
    anchors: Vec<HirariProjectAudioNoteAnchorView>,
    _source: std::marker::PhantomData<&'a [u8]>,
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
    range_edits: Vec<HirariProjectRangeEditView>,
    processing_history: Vec<EventStep<'a>>,
    note_segments: Vec<NoteSegment<'a>>,
    warp_markers: Vec<HirariProjectWarpMarkerView>,
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
                    let anchor = HirariProjectAudioNoteAnchorView {
                        position_seconds: cursor.f64()?,
                        pitch_cents: cursor.f64()?,
                        formant_cents: cursor.f64()?,
                    };
                    if !anchor.position_seconds.is_finite()
                        || !anchor.pitch_cents.is_finite()
                        || !anchor.formant_cents.is_finite()
                        || anchor.position_seconds < start
                        || anchor.position_seconds > end
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
                    _source: std::marker::PhantomData,
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
                let edit = HirariProjectRangeEditView {
                    start: cursor.u64()?,
                    end: cursor.u64()?,
                    gain: cursor.f32()?,
                    fade_in: cursor.u64()?,
                    fade_out: cursor.u64()?,
                };
                let length = edit.end.checked_sub(edit.start)?;
                if length == 0
                    || !edit.gain.is_finite()
                    || !(0.0..=16.0).contains(&edit.gain)
                    || edit.fade_in > length
                    || edit.fade_out > length
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
                region.warp_markers.push(HirariProjectWarpMarkerView {
                    source_sample: cursor.u64()?,
                    timeline_sample: cursor.u64()?,
                    transient: cursor.u8()?,
                });
            }
        }
    }
    Some(region)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_decode_regions(
    bytes: *const u8,
    byte_count: usize,
    offset: usize,
    version: u32,
    sample_rate: u32,
    track_ids: *const u32,
    track_id_count: usize,
    initial_string_bytes: usize,
    context: *mut std::ffi::c_void,
    consume_region: Option<RegionConsumer>,
    consumed_offset: *mut usize,
) -> bool {
    if bytes.is_null()
        || (track_id_count != 0 && track_ids.is_null())
        || context.is_null()
        || consume_region.is_none()
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
    let Some(region_count) = cursor.u32() else {
        return false;
    };
    if region_count > 1_000_000 {
        return false;
    }
    let known_tracks: HashSet<u32> = if track_id_count == 0 {
        HashSet::new()
    } else {
        unsafe { std::slice::from_raw_parts(track_ids, track_id_count) }
            .iter()
            .copied()
            .collect()
    };
    let consume_region = consume_region.unwrap();
    let mut region_ids = HashSet::with_capacity(region_count as usize);
    let mut total_strings = initial_string_bytes;
    for _ in 0..region_count {
        let Some(region) = parse_region(
            &mut cursor,
            version,
            sample_rate,
            &known_tracks,
            &mut region_ids,
            &mut total_strings,
        ) else {
            return false;
        };
        let note_segments: Vec<_> = region
            .note_segments
            .iter()
            .map(|segment| HirariProjectAudioNoteSegmentView {
                start_seconds: segment.start,
                end_seconds: segment.end,
                detected_pitch_cents: segment.pitch,
                pitch_offset_cents: segment.pitch_offset,
                formant_offset_cents: segment.formant_offset,
                anchors: segment.anchors.as_ptr(),
                anchor_count: segment.anchors.len(),
            })
            .collect();
        let history: Vec<_> = region
            .processing_history
            .iter()
            .map(|step| HirariProjectEventStepView {
                id: step.id,
                operation: step.operation.as_ptr(),
                operation_size: step.operation.len(),
                parameter: step.parameter,
                enabled: step.enabled,
            })
            .collect();
        let view = HirariProjectRegionView {
            id: region.id,
            track_id: region.track_id,
            sample_position: region.position,
            sample_length: region.length,
            source_length: region.source_length,
            source_sample_rate: region.source_rate,
            source_offset: region.source_offset,
            base_start: region.base_start,
            base_source_offset: region.base_source_offset,
            base_length: region.base_length,
            muted: region.muted,
            file_path: region.file_path.as_ptr(),
            file_path_size: region.file_path.len(),
            name: region.name.as_ptr(),
            name_size: region.name.len(),
            clip_gain: region.clip_gain,
            fade_in_samples: region.fade_in,
            fade_out_samples: region.fade_out,
            reverse: region.reverse,
            warp_ratio: region.warp_ratio,
            pitch_preserve_warp: region.pitch_preserve,
            pitch_semitones: region.pitch_semitones,
            loop_count: region.loop_count,
            locked: region.locked,
            sync_group: region.sync_group,
            range_edits: region.range_edits.as_ptr(),
            range_edit_count: region.range_edits.len(),
            processing_history: history.as_ptr(),
            processing_history_count: history.len(),
            audio_note_segments: note_segments.as_ptr(),
            audio_note_segment_count: note_segments.len(),
            warp_markers: region.warp_markers.as_ptr(),
            warp_marker_count: region.warp_markers.len(),
        };
        if !unsafe { consume_region(context, &view) } {
            return false;
        }
    }
    unsafe { *consumed_offset = cursor.offset };
    true
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
unsafe fn borrowed_slice<'a, T>(pointer: *const T, length: usize) -> &'a [T] {
    if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pointer, length) }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_encode_regions(
    version: u32,
    sample_rate: u32,
    regions: *const HirariProjectRegionView,
    region_count: usize,
    output_size: *mut usize,
) -> *mut u8 {
    if !(1..=42).contains(&version)
        || output_size.is_null()
        || (region_count != 0 && regions.is_null())
    {
        return std::ptr::null_mut();
    }
    unsafe { *output_size = 0 };
    let Ok(count) = u32::try_from(region_count) else {
        return std::ptr::null_mut();
    };
    let mut output = Vec::new();
    encode_u32(&mut output, count);
    for region in unsafe { borrowed_slice(regions, region_count) } {
        if version >= 28 {
            encode_u32(&mut output, region.id);
        }
        encode_u32(&mut output, region.track_id);
        encode_u64(&mut output, region.sample_position);
        encode_u64(&mut output, region.sample_length);
        if version >= 19 {
            encode_u64(&mut output, region.source_offset);
            encode_u64(&mut output, region.base_start);
            encode_u64(&mut output, region.base_source_offset);
            encode_u64(&mut output, region.base_length);
        }
        encode_u8(&mut output, region.muted);
        for (pointer, size) in [
            (region.file_path, region.file_path_size),
            (region.name, region.name_size),
        ] {
            let Ok(size_u32) = u32::try_from(size) else {
                return std::ptr::null_mut();
            };
            if size != 0 && pointer.is_null() {
                return std::ptr::null_mut();
            }
            encode_u32(&mut output, size_u32);
            output.extend_from_slice(unsafe { borrowed_slice(pointer, size) });
        }
        encode_f32(&mut output, region.clip_gain);
        encode_u64(&mut output, region.fade_in_samples);
        encode_u64(&mut output, region.fade_out_samples);
        if version >= 18 {
            encode_u8(&mut output, region.reverse);
        }
        if version >= 20 {
            encode_f64(&mut output, region.warp_ratio);
        }
        if version >= 21 {
            encode_f32(&mut output, region.pitch_semitones);
        }
        if version >= 38 {
            encode_u8(&mut output, region.pitch_preserve_warp);
        }
        if version >= 39 {
            encode_u64(
                &mut output,
                if region.source_length == 0 {
                    region.sample_length
                } else {
                    region.source_length
                },
            );
        }
        if version >= 42 {
            output.extend_from_slice(
                &(if region.source_sample_rate == 0 {
                    sample_rate
                } else {
                    region.source_sample_rate
                })
                .to_ne_bytes(),
            );
        }
        if version >= 22 {
            encode_u32(&mut output, region.loop_count);
        }
        if version >= 33 {
            if region.audio_note_segment_count != 0 && region.audio_note_segments.is_null() {
                return std::ptr::null_mut();
            }
            let segment_count = region.audio_note_segment_count.min(100_000);
            encode_u32(&mut output, segment_count as u32);
            for segment in unsafe { borrowed_slice(region.audio_note_segments, segment_count) } {
                encode_f64(&mut output, segment.start_seconds);
                encode_f64(&mut output, segment.end_seconds);
                encode_f64(&mut output, segment.detected_pitch_cents);
                encode_f64(&mut output, segment.pitch_offset_cents);
                encode_f64(&mut output, segment.formant_offset_cents);
                if segment.anchor_count != 0 && segment.anchors.is_null() {
                    return std::ptr::null_mut();
                }
                let anchor_count = segment.anchor_count.min(10_000);
                encode_u32(&mut output, anchor_count as u32);
                for anchor in unsafe { borrowed_slice(segment.anchors, anchor_count) } {
                    encode_f64(&mut output, anchor.position_seconds);
                    encode_f64(&mut output, anchor.pitch_cents);
                    encode_f64(&mut output, anchor.formant_cents);
                }
            }
        }
        if version >= 35 {
            encode_u8(&mut output, region.locked);
            encode_u32(&mut output, region.sync_group);
            if region.processing_history_count != 0 && region.processing_history.is_null() {
                return std::ptr::null_mut();
            }
            let step_count = region.processing_history_count.min(4096);
            encode_u32(&mut output, step_count as u32);
            for step in unsafe { borrowed_slice(region.processing_history, step_count) } {
                if step.operation_size != 0 && step.operation.is_null() {
                    return std::ptr::null_mut();
                }
                let operation_size = step.operation_size.min(256);
                encode_u32(&mut output, step.id);
                encode_f32(&mut output, step.parameter);
                encode_u8(&mut output, step.enabled);
                encode_u32(&mut output, operation_size as u32);
                output.extend_from_slice(unsafe { borrowed_slice(step.operation, operation_size) });
            }
        }
        if version >= 36 {
            if region.range_edit_count != 0 && region.range_edits.is_null() {
                return std::ptr::null_mut();
            }
            let valid_edits: Vec<_> =
                unsafe { borrowed_slice(region.range_edits, region.range_edit_count) }
                    .iter()
                    .filter(|edit| {
                        edit.start < edit.end
                            && edit.gain.is_finite()
                            && (0.0..=16.0).contains(&edit.gain)
                            && edit.fade_in <= edit.end - edit.start
                            && edit.fade_out <= edit.end - edit.start
                    })
                    .take(4096)
                    .collect();
            encode_u32(&mut output, valid_edits.len() as u32);
            for edit in valid_edits {
                encode_u64(&mut output, edit.start);
                encode_u64(&mut output, edit.end);
                encode_f32(&mut output, edit.gain);
                encode_u64(&mut output, edit.fade_in);
                encode_u64(&mut output, edit.fade_out);
            }
        }
        if version >= 41 {
            if region.warp_marker_count != 0 && region.warp_markers.is_null() {
                return std::ptr::null_mut();
            }
            let marker_count = region.warp_marker_count.min(100_000);
            encode_u32(&mut output, marker_count as u32);
            for marker in unsafe { borrowed_slice(region.warp_markers, marker_count) } {
                encode_u64(&mut output, marker.source_sample);
                encode_u64(&mut output, marker.timeline_sample);
                encode_u8(&mut output, marker.transient);
            }
        }
    }
    let size = output.len();
    unsafe { *output_size = size };
    Box::into_raw(output.into_boxed_slice()).cast::<u8>()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_free_encoded_regions(data: *mut u8, size: usize) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, size)) });
    }
}
