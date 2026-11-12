//! Backend-neutral arrangement model and timeline geometry.
//!
//! The arranger needs the same facts regardless of windowing backend: which
//! tracks and clips exist, where the ruler's measure lines fall, how long the
//! timeline runs, and how MIDI note runs collapse into visible clips. These
//! Routines are backend-neutral: kept stable so the AmeUI canvas and any
//! tooling consume the same arrangement geometry from one source. Nothing
//! here depends on a UI backend or on a
//! live engine; the engine is reached only through the `samples_to_beats`
//! closure passed to [`parse_arrange_tracks`], which keeps the parsers testable.

use serde::Deserialize;

/// One ruler measure line. `number` is the 1-based measure index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasureStart {
    pub beat: f32,
    pub number: i32,
}

/// A single recorded take from the engine's comping snapshot. Region carries
/// are hidden inside the comping structures; the UI only needs the take id
/// and span to drive layout.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct CompTake {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub start_sample: u64,
    #[serde(default)]
    pub end_sample: u64,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct CompingSnapshot {
    #[serde(default)]
    takes: Vec<CompTake>,
}

/// Parses `comping_snapshot_json()` once per poll tick, tolerating the
/// empty-state default (`{"takes":[],"current_comp":[]}`).
pub fn parse_comping_takes(json: &str) -> Vec<CompTake> {
    serde_json::from_str::<CompingSnapshot>(json)
        .map(|snapshot| snapshot.takes)
        .unwrap_or_default()
}

/// A persisted track stack. `member_track_ids` is sorted ascending by the
/// engine boundary, so paths that want members in selected order should sort
/// again or iterate the arrangement selection directly.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct TrackStack {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub member_track_ids: Vec<u32>,
    #[serde(default = "one")]
    pub master_gain: f32,
    #[serde(default)]
    pub collapsed: bool,
}

fn one() -> f32 {
    1.0
}

/// Parse `track_stacks_json()` once at a time the UI needs the current
/// arrangement; kept in `ui_shared` so the canvas, the routing page, and any
/// inset surface share the same source of truth.
pub fn parse_track_stacks(json: &str) -> Vec<TrackStack> {
    serde_json::from_str::<Vec<TrackStack>>(json).unwrap_or_default()
}

/// A visible arrangement clip. Audio clips carry their native region id;
/// synthesized MIDI clips carry a negative id and `midi == true` so region
/// mutations are never offered for them.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ArrangeClip {
    pub id: i32,
    pub name: String,
    pub start_beat: f32,
    pub length_beats: f32,
    pub midi: bool,
    /// Sync group the clip moves with; 0 means it moves alone. Drives the
    /// `move_region_sync_group` engine call.
    pub sync_group: u32,
    /// Trim window as fractions of the source (0..1), as published by the
    /// layout and consumed by `set_region_trim`. Full clip is 0..1.
    pub trim_start: f32,
    pub trim_end: f32,
    /// Reversed playback; flips the waveform peak mapping as the arrangement dock does
    /// dock, and gates trim handles to Audio/Vocal lanes at draw time.
    pub reverse: bool,
    /// Fade in/out as fractions of the clip length (0..1), as published
    /// by the layout and consumed by `set_region_fades`.
    pub fade_in: f32,
    pub fade_out: f32,
    /// Playback speed ratio (0.25–4×, engine range is 0.5–2×).
    pub warp_ratio: f64,
    /// Whether warp preserves pitch (WSOLA).
    pub pitch_preserve_warp: bool,
    /// Clip gain (linear, 0..2), consumed by `set_region_gain`.
    pub gain: f32,
    /// Clip pitch offset in semitones, from the engine layout.
    pub pitch_semitones: f32,
    /// Loop count from the engine layout.
    pub loop_count: u32,
}

/// One arrangement lane reduced to what the timeline draws, plus the
/// mixer state the AmeUI track strips edit.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangeTrack {
    pub id: u32,
    pub name: String,
    pub kind: String,
    pub color: [u8; 3],
    pub clips: Vec<ArrangeClip>,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub record_armed: bool,
    /// Expression-map entries (keyswitch/articulation map editing).
    pub expression_map: Vec<hirari_core_bridge::project_contracts::ExpressionMapEntry>,
    /// Installed plugin chain: engine plugin-type ids and bypass flags.
    pub plugin_types: Vec<u32>,
    pub plugin_bypass: Vec<bool>,
    /// Volume/pan automation as (beat, value) after sample conversion.
    pub volume_automation: Vec<AutomationPoint>,
    pub pan_automation: Vec<AutomationPoint>,
}

/// One automation point in beat space (curve weights are edit-time
/// detail and are not needed for the read-only lanes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutomationPoint {
    pub beat: f32,
    pub value: f32,
}

/// Display name for an engine plugin-type id, mirroring the names the
/// the track-sync path publishes for the built-in chain.
pub fn plugin_name(plugin_type: u32, index: usize) -> String {
    match plugin_type {
        0 => "Hirari Limiter".to_owned(),
        1 => "Hirari Compressor".to_owned(),
        2 => "Hirari Gate".to_owned(),
        3 => "Tube Saturation".to_owned(),
        4 => "Hirari Transient".to_owned(),
        5 => "Hirari De-Esser".to_owned(),
        6 => "Hirari Delay".to_owned(),
        7 => "Hirari Reverb".to_owned(),
        8 => "Hirari Dynamic EQ".to_owned(),
        9 => "Hirari Mid/Side".to_owned(),
        10 => "Hirari Width".to_owned(),
        _ => format!("Plugin {}", index + 1),
    }
}

/// A timeline marker. `color` is the engine's `#rrggbb` marker color as
/// RGB, matching the marker color so a project keeps its marker
/// colors across the backend migration.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangeMarker {
    pub id: u32,
    pub label: String,
    pub beat: f64,
    pub color: [u8; 3],
}

/// The rotating lane palette shared with the track-sync path so a project
/// keeps its colors across the backend migration.
pub fn track_color(index: usize) -> [u8; 3] {
    match index % 5 {
        0 => [120, 130, 143], // Slate
        1 => [127, 165, 138], // Sage
        2 => [169, 104, 104], // Oxide
        3 => [177, 154, 104], // Brass
        _ => [130, 148, 154], // Steel
    }
}

/// Validates and deduplicates the flat `[beat, numerator, denominator, ...]`
/// time-signature event stream published by the engine.
pub fn valid_time_signatures(events: &[f64]) -> Vec<(f64, u8, u8)> {
    let mut meters = events
        .chunks_exact(3)
        .filter_map(|event| {
            let beat = event[0];
            let numerator = event[1];
            let denominator = event[2];
            if !beat.is_finite()
                || beat < 0.0
                || !numerator.is_finite()
                || numerator.fract() != 0.0
                || !(1.0..=32.0).contains(&numerator)
                || !denominator.is_finite()
                || denominator.fract() != 0.0
                || ![1.0, 2.0, 4.0, 8.0, 16.0, 32.0].contains(&denominator)
            {
                return None;
            }
            Some((beat, numerator as u8, denominator as u8))
        })
        .collect::<Vec<_>>();
    meters.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut unique_meters = Vec::with_capacity(meters.len());
    for meter in meters {
        if let Some(previous) = unique_meters.last_mut() {
            let previous: &mut (f64, u8, u8) = previous;
            if (previous.0 - meter.0).abs() <= f64::EPSILON {
                *previous = meter;
                continue;
            }
        }
        unique_meters.push(meter);
    }
    unique_meters
}

/// The active meter at a beat, defaulting to 4/4 before the first event.
pub fn time_signature_at(events: &[(f64, u8, u8)], beat: f32) -> (u8, u8) {
    events
        .iter()
        .take_while(|event| event.0 <= f64::from(beat.max(0.0)) + 1e-6)
        .last()
        .map(|event| (event.1, event.2))
        .unwrap_or((4, 4))
}

/// Rounds the last clip end up to a musical bar boundary (respecting meter
/// changes) and leaves at least 128 beats of arrange room.
pub fn arrangement_timeline_end(last_clip_end: f32, events: &[(f64, u8, u8)]) -> f32 {
    const EPSILON_BEATS: f64 = 1e-6;
    let last_clip_end = f64::from(last_clip_end.max(0.0));
    let active_meter = events
        .iter()
        .take_while(|event| event.0 <= last_clip_end + EPSILON_BEATS)
        .last()
        .copied()
        .unwrap_or((0.0, 4, 4));
    let bar_length = f64::from(active_meter.1) * 4.0 / f64::from(active_meter.2);
    let bars_from_anchor = ((last_clip_end - active_meter.0).max(0.0) / bar_length).ceil();
    let mut boundary = active_meter.0 + bars_from_anchor * bar_length;
    if boundary + EPSILON_BEATS < last_clip_end {
        boundary += bar_length;
    }
    if let Some(next_meter) = events.iter().find(|event| {
        event.0 > active_meter.0 + EPSILON_BEATS && event.0 <= boundary + EPSILON_BEATS
    }) {
        boundary = next_meter.0;
    }

    // A meter change inside the bar that starts at the boundary interrupts
    // that bar, so snap the boundary to the change and re-check from there.
    // With no interrupting change the boundary is already a bar line, so
    // stop: advancing unconditionally here overshot the rounded-up boundary
    // by up to four bars.
    for _ in 0..4 {
        let meter = events
            .iter()
            .take_while(|event| event.0 <= boundary + EPSILON_BEATS)
            .last()
            .copied()
            .unwrap_or((0.0, 4, 4));
        let next_bar = boundary + f64::from(meter.1) * 4.0 / f64::from(meter.2);
        match events
            .iter()
            .find(|event| event.0 > boundary + EPSILON_BEATS && event.0 < next_bar - EPSILON_BEATS)
        {
            Some(event) => boundary = event.0,
            None => break,
        }
    }

    let timeline_end = boundary as f32;
    if timeline_end.is_finite() {
        timeline_end.max(128.0)
    } else {
        128.0
    }
}

/// The ruler measure lines through `end_beat`, honoring meter changes.
pub fn arrangement_measure_starts(events: &[f64], end_beat: f32) -> Vec<MeasureStart> {
    const MAX_MEASURE_MARKERS: usize = 16_384;
    let mut unique_meters = valid_time_signatures(events);
    if unique_meters
        .first()
        .is_none_or(|meter| meter.0 > f64::EPSILON)
    {
        unique_meters.insert(0, (0.0, 4, 4));
    }

    let end_beat = f64::from(end_beat.max(0.0));
    let mut starts = Vec::with_capacity(128);
    starts.push(MeasureStart {
        beat: 0.0,
        number: 1,
    });
    for (index, (segment_start, numerator, denominator)) in
        unique_meters.iter().copied().enumerate()
    {
        if segment_start > end_beat {
            break;
        }
        if segment_start > starts.last().map_or(0.0, |marker| f64::from(marker.beat)) + 1e-6 {
            starts.push(MeasureStart {
                beat: segment_start as f32,
                number: starts.len() as i32 + 1,
            });
        }
        let segment_end = unique_meters
            .get(index + 1)
            .map_or(end_beat, |next| next.0.min(end_beat));
        let beats_per_measure = f64::from(numerator) * 4.0 / f64::from(denominator);
        let mut next_start = segment_start + beats_per_measure;
        while next_start < segment_end - 1e-6
            && next_start <= end_beat
            && starts.len() < MAX_MEASURE_MARKERS
        {
            starts.push(MeasureStart {
                beat: next_start as f32,
                number: starts.len() as i32 + 1,
            });
            next_start += beats_per_measure;
        }
        if starts.len() >= MAX_MEASURE_MARKERS {
            break;
        }
    }
    starts
}

/// Collapses note spans into contiguous clips. Notes closer than a quarter
/// beat belong to one region; a larger gap starts a new take. Spans are sorted
/// internally, so callers may pass them in any order.
pub fn group_midi_spans(spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    const MAX_GAP_BEATS: f64 = 0.25;
    let mut spans = spans;
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut groups: Vec<(f64, f64)> = Vec::new();
    for span in spans {
        match groups.last_mut() {
            Some(group) if span.0 - group.1 <= MAX_GAP_BEATS => {
                group.1 = group.1.max(span.1);
            }
            _ => groups.push((span.0, span.1)),
        }
    }
    groups
}

#[derive(Deserialize)]
struct RegionLayout {
    id: u32,
    name: String,
    start: u64,
    len: u64,
    #[serde(default)]
    sync_group: u32,
    #[serde(default)]
    source_offset: u64,
    #[serde(default)]
    base_source_offset: u64,
    #[serde(default)]
    base_length: u64,
    #[serde(default)]
    source_length: u64,
    #[serde(default)]
    reverse: bool,
    #[serde(default)]
    fade_in_samples: u64,
    #[serde(default)]
    fade_out_samples: u64,
    #[serde(default = "default_clip_gain")]
    clip_gain: f32,
    #[serde(default)]
    pitch_semitones: f32,
    #[serde(default = "default_warp_ratio")]
    warp_ratio: f64,
    #[serde(default)]
    pitch_preserve_warp: bool,
    #[serde(default = "default_loop_count")]
    loop_count: u32,
}

fn default_warp_ratio() -> f64 {
    1.0
}

fn default_loop_count() -> u32 {
    1
}

fn default_clip_gain() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct TrackLayout {
    id: u32,
    name: String,
    #[serde(rename = "type")]
    track_type: String,
    // Required exactly as in the track-sync path; the engine always
    // publishes them, and a snapshot missing them is not a track list.
    volume: f32,
    pan: f32,
    mute: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    record_armed: bool,
    #[serde(default)]
    expression_map: Vec<hirari_core_bridge::project_contracts::ExpressionMapEntry>,
    #[serde(default)]
    plugin_types: Vec<u32>,
    #[serde(default)]
    plugin_bypass: Vec<bool>,
    #[serde(default)]
    volume_automation: Vec<AutomationLayoutPoint>,
    #[serde(default)]
    pan_automation: Vec<AutomationLayoutPoint>,
    #[serde(default)]
    regions: Vec<RegionLayout>,
}

#[derive(Deserialize)]
struct AutomationLayoutPoint {
    time: f64,
    value: f32,
    curve: f32,
}

/// Mirrors the automation conversion: integer sample times become
/// beats, values clamp to the lane range, invalid rows drop.
fn parse_automation(
    points: Vec<AutomationLayoutPoint>,
    samples_to_beats: impl Fn(u64) -> f64,
    min_value: f32,
    max_value: f32,
) -> Vec<AutomationPoint> {
    points
        .into_iter()
        .filter_map(|point| {
            if !point.time.is_finite()
                || point.time < 0.0
                || point.time.fract() != 0.0
                || !point.value.is_finite()
                || !point.curve.is_finite()
            {
                return None;
            }
            let beat = samples_to_beats(point.time as u64);
            if !beat.is_finite() {
                return None;
            }
            Some(AutomationPoint {
                beat: beat as f32,
                value: point.value.clamp(min_value, max_value),
            })
        })
        .collect()
}

/// Reduces the native project layout JSON to arrangement lanes. Malformed or
/// empty input yields no tracks; the caller keeps its last snapshot in that
/// case rather than blanking the arranger during startup/offline states.
pub fn parse_arrange_tracks(
    layout_json: &str,
    samples_to_beats: impl Fn(u64) -> f64,
) -> Vec<ArrangeTrack> {
    let Ok(layouts) = serde_json::from_str::<Vec<TrackLayout>>(layout_json) else {
        return Vec::new();
    };
    layouts
        .into_iter()
        .enumerate()
        .map(|(index, layout)| {
            let volume_automation =
                parse_automation(layout.volume_automation, &samples_to_beats, 0.0, 2.0);
            let pan_automation =
                parse_automation(layout.pan_automation, &samples_to_beats, -1.0, 1.0);
            let clips = layout
                .regions
                .into_iter()
                .filter(|region| region.id != 0 && region.len != 0)
                .map(|region| {
                    let start_beat = samples_to_beats(region.start);
                    let end_beat = samples_to_beats(region.start.saturating_add(region.len));
                    ArrangeClip {
                        id: region.id as i32,
                        name: region.name,
                        start_beat: start_beat.max(0.0) as f32,
                        length_beats: (end_beat - start_beat).max(0.0) as f32,
                        midi: false,
                        sync_group: region.sync_group,
                        // Same trim-fraction formulas as the track
                        // sync, so both surfaces trim the same window.
                        trim_start: if region.base_length == 0 {
                            0.0
                        } else {
                            region
                                .source_offset
                                .saturating_sub(region.base_source_offset)
                                as f32
                                / region.base_length as f32
                        },
                        trim_end: if region.base_length == 0 {
                            1.0
                        } else {
                            (region
                                .source_offset
                                .saturating_sub(region.base_source_offset)
                                .saturating_add(if region.source_length == 0 {
                                    region.len
                                } else {
                                    region.source_length
                                })) as f32
                                / region.base_length as f32
                        },
                        reverse: region.reverse,
                        fade_in: if region.len == 0 {
                            0.0
                        } else {
                            region.fade_in_samples as f32 / region.len as f32
                        },
                        fade_out: if region.len == 0 {
                            0.0
                        } else {
                            region.fade_out_samples as f32 / region.len as f32
                        },
                        pitch_semitones: region.pitch_semitones,
                        warp_ratio: region.warp_ratio,
                        pitch_preserve_warp: region.pitch_preserve_warp,
                        loop_count: region.loop_count,
                        gain: region.clip_gain.clamp(0.0, 2.0),
                    }
                })
                .collect();
            ArrangeTrack {
                id: layout.id,
                name: layout.name,
                kind: layout.track_type,
                color: track_color(index),
                clips,
                volume: layout.volume,
                pan: layout.pan,
                muted: layout.mute,
                solo: layout.solo,
                record_armed: layout.record_armed,
                expression_map: layout.expression_map,
                plugin_types: layout.plugin_types,
                plugin_bypass: layout.plugin_bypass,
                volume_automation,
                pan_automation,
            }
        })
        .collect()
}

/// Parses the engine marker snapshot, dropping invalid rows. A missing or
/// malformed `color` falls back to the same default as the marker sync
/// uses, rather than discarding the whole snapshot.
pub fn parse_markers(markers_json: &str) -> Vec<ArrangeMarker> {
    #[derive(Deserialize)]
    struct MarkerLayout {
        id: u32,
        label: String,
        beat: f64,
        #[serde(default)]
        color: String,
    }
    let Ok(markers) = serde_json::from_str::<Vec<MarkerLayout>>(markers_json) else {
        return Vec::new();
    };
    let mut markers = markers
        .into_iter()
        .filter(|marker| marker.id != 0 && marker.beat.is_finite() && marker.beat >= 0.0)
        .map(|marker| {
            let rgb =
                u32::from_str_radix(marker.color.trim_start_matches('#'), 16).unwrap_or(0x6472a8);
            ArrangeMarker {
                id: marker.id,
                label: marker.label,
                beat: marker.beat,
                color: [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8],
            }
        })
        .collect::<Vec<_>>();
    markers.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    markers
}

/// The furthest clip end across all lanes, used to size the timeline.
pub fn last_clip_end(tracks: &[ArrangeTrack]) -> f32 {
    tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .map(|clip| clip.start_beat + clip.length_beats.max(0.0))
        .filter(|end| end.is_finite())
        .fold(0.0f32, f32::max)
}

/// Zoom limits, matching the toolbar buttons (×/÷1.25 per step).
/// Fit-to-width is zoom 1.0; zooming out past fit would only leave empty
/// space, so the floor is 1.0 rather than 0.25.
pub const ARRANGE_ZOOM_MIN: f32 = 1.0;
pub const ARRANGE_ZOOM_MAX: f32 = 8.0;

/// Pixel layout of one arrangement canvas. Zoom 1.0 fits the whole
/// timeline into the slot (the FIT zoom); higher zoom shows a
/// window of it scrolled by `scroll_beats`. The draw step stays a pure
/// function of this struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrangeCanvasGeometry {
    /// Horizontal scale: canvas pixels per beat.
    pub px_per_beat: f32,
    /// Height of the measure/marker ruler band at the top.
    pub ruler_height: f32,
    /// Height of one track lane below the ruler.
    pub lane_height: f32,
    /// Beat step for the faint grid lines (`visible-beat-step`).
    pub beat_step: f32,
    /// Left edge of the visible window, in beats (clamped).
    pub scroll_beats: f32,
    /// Width of the lane-header column at the left (104px).
    pub header_width: f32,
}

impl ArrangeCanvasGeometry {
    /// Canvas x of a beat, in canvas-local pixels. Beats outside the
    /// visible window map off-canvas (negative or past the width) and are
    /// clipped by the renderer; they are not clamped to the edges.
    pub fn x_for_beat(&self, beat: f32) -> f32 {
        self.header_width + (beat - self.scroll_beats) * self.px_per_beat
    }

    /// Beat at a canvas x (click-to-seek inverse of [`Self::x_for_beat`]).
    /// A zero scale (never drawn / collapsed canvas) seeks to beat 0.
    pub fn beat_for_x(&self, x: f32) -> f32 {
        if self.px_per_beat > 0.0 && x.is_finite() {
            ((x - self.header_width) / self.px_per_beat + self.scroll_beats).max(0.0)
        } else {
            0.0
        }
    }

    /// Top y of a lane, in canvas-local pixels.
    pub fn y_for_lane(&self, lane: usize) -> f32 {
        self.ruler_height + lane as f32 * self.lane_height
    }
}

/// Derives the canvas layout for a `width` x `height` slot showing
/// `timeline_end_beats` beats across `lane_count` lanes at `zoom`, scrolled
/// to `scroll_beats`. Zoom and scroll are clamped to what the canvas can
/// show; degenerate sizes yield a zeroed layout rather than NaNs, so a
/// collapsed window is safe.
pub fn arrange_canvas_geometry(
    width: f32,
    height: f32,
    timeline_end_beats: f32,
    lane_count: usize,
    zoom: f32,
    scroll_beats: f32,
) -> ArrangeCanvasGeometry {
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    let height = if height.is_finite() {
        height.max(0.0)
    } else {
        0.0
    };
    let timeline_end = if timeline_end_beats.is_finite() {
        timeline_end_beats.max(1.0)
    } else {
        1.0
    };
    let zoom = if zoom.is_finite() {
        zoom.clamp(ARRANGE_ZOOM_MIN, ARRANGE_ZOOM_MAX)
    } else {
        1.0
    };
    // The timeline area starts after the lane-header column.
    let header_width = 104.0_f32.min(width);
    let timeline_width = (width - header_width).max(0.0);
    let px_per_beat = timeline_width / timeline_end * zoom;
    let visible_beats = if px_per_beat > 0.0 {
        timeline_width / px_per_beat
    } else {
        timeline_end
    };
    let max_scroll = (timeline_end - visible_beats).max(0.0);
    let scroll_beats = if scroll_beats.is_finite() {
        scroll_beats.clamp(0.0, max_scroll)
    } else {
        0.0
    };
    let ruler_height = 24.0_f32.min(height);
    let lane_height = if lane_count == 0 {
        0.0
    } else {
        (height - ruler_height).max(0.0) / lane_count as f32
    };
    // Same adaptive step as the arrange dock: at least one beat, and never so
    // many grid lines that they merge into a solid band.
    let beat_step = (4.0 / px_per_beat.max(f32::EPSILON))
        .max(timeline_end / 2048.0)
        .max(1.0)
        .ceil();
    ArrangeCanvasGeometry {
        px_per_beat,
        ruler_height,
        lane_height,
        beat_step,
        scroll_beats,
        header_width,
    }
}

/// The clip under a canvas point, as `(track_id, clip_id)`. Returns `None`
/// in the ruler band, below the last lane, or on empty lane space, so a
/// click there keeps its seek meaning. This is the hit-test half of clip
/// selection; the drag/trim mutations still belong to a later slice.
pub fn clip_at(
    tracks: &[ArrangeTrack],
    geometry: &ArrangeCanvasGeometry,
    x: f32,
    y: f32,
) -> Option<(u32, i32)> {
    if geometry.lane_height <= 0.0 || y < geometry.ruler_height {
        return None;
    }
    let lane = ((y - geometry.ruler_height) / geometry.lane_height) as usize;
    let track = tracks.get(lane)?;
    let beat = geometry.beat_for_x(x);
    track
        .clips
        .iter()
        .find(|clip| beat >= clip.start_beat && beat < clip.start_beat + clip.length_beats)
        .map(|clip| (track.id, clip.id))
}

/// Which clip edge a canvas x is on, for trim drags. The dock puts
/// an 8px handle on each edge; here the zones are 6px and only exist when
/// the clip is wide enough to hold both without overlapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrimEdge {
    Left,
    Right,
}

pub fn trim_edge_at(
    geometry: &ArrangeCanvasGeometry,
    clip: &ArrangeClip,
    x: f32,
) -> Option<TrimEdge> {
    const EDGE_PX: f32 = 6.0;
    let left = geometry.x_for_beat(clip.start_beat);
    let width = clip.length_beats * geometry.px_per_beat;
    if width <= EDGE_PX * 2.0 {
        return None;
    }
    if (x - left).abs() <= EDGE_PX {
        Some(TrimEdge::Left)
    } else if (x - (left + width)).abs() <= EDGE_PX {
        Some(TrimEdge::Right)
    } else {
        None
    }
}

/// One audio device from the engine's device catalog, as the settings
/// UI lists it. `id` is the `select_audio_device` handle.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    pub id: u32,
    pub name: String,
}

/// Parses `list_audio_devices_json`; malformed input yields no devices.
pub fn parse_audio_devices(catalog_json: &str) -> Vec<AudioDevice> {
    #[derive(Deserialize)]
    struct DeviceLayout {
        id: u32,
        #[serde(default)]
        name: String,
    }
    let Ok(devices) = serde_json::from_str::<Vec<DeviceLayout>>(catalog_json) else {
        return Vec::new();
    };
    devices
        .into_iter()
        .map(|device| AudioDevice {
            id: device.id,
            name: device.name,
        })
        .collect()
}

/// One audio-library asset from the preview-audio catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryAsset {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub sample_rate: f64,
    pub size_bytes: u64,
}

/// Parses `preview_audio_catalog_json`; malformed input yields no assets.
pub fn parse_preview_catalog(catalog_json: &str) -> Vec<LibraryAsset> {
    #[derive(Deserialize)]
    struct AssetLayout {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        name: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        sample_rate: f64,
        #[serde(default)]
        size: u64,
    }
    #[derive(Deserialize)]
    struct CatalogLayout {
        #[serde(default)]
        assets: Vec<AssetLayout>,
    }
    let Ok(catalog) = serde_json::from_str::<CatalogLayout>(catalog_json) else {
        return Vec::new();
    };
    catalog
        .assets
        .into_iter()
        .map(|asset| LibraryAsset {
            id: asset.id,
            name: if asset.name.is_empty() {
                "Audio".to_owned()
            } else {
                asset.name
            },
            path: asset.path,
            sample_rate: asset.sample_rate,
            size_bytes: asset.size,
        })
        .collect()
}

/// One MIDI note with its full authoring contract, so piano-roll edits
/// can commit a lossless `replace_midi_note_contracts` (lyric,
/// articulation, vibrato and curve metadata survive an edit, exactly as
/// the roll preserves them through its model).
#[derive(Debug, Clone, PartialEq)]
pub struct PianoNote {
    pub track_id: u32,
    pub region_id: u32,
    pub pitch: u8,
    pub midi_channel: u8,
    pub articulation: u8,
    pub velocity: u8,
    pub start_beat: f32,
    pub length_beats: f32,
    pub lyric: String,
    pub phoneme: String,
    pub pitch_curve_cents: Vec<i16>,
    pub vibrato_depth_cents: u16,
    pub vibrato_rate_millihz: u16,
    pub portamento_samples: u32,
    pub probability: u8,
    pub repeat_count: u16,
}

/// Parses the engine's canonical `midi_notes_json` into notes, sanitized
/// like the piano roll (`sanitize_notes`): pitches and velocities
/// clamped, lengths floored, sorted by track/start/pitch. Malformed
/// input yields no notes.
pub fn parse_midi_notes(notes_json: &str, samples_to_beats: impl Fn(u64) -> f64) -> Vec<PianoNote> {
    fn default_vibrato_rate() -> u16 {
        5_000
    }
    fn default_probability() -> u8 {
        100
    }
    fn default_repeat_count() -> u16 {
        1
    }
    #[derive(Deserialize)]
    struct NoteLayout {
        track_id: u32,
        pitch: u8,
        velocity: u8,
        start_sample: u64,
        length_samples: u64,
        #[serde(default)]
        region_id: u32,
        #[serde(default)]
        midi_channel: u8,
        #[serde(default)]
        articulation: u8,
        #[serde(default)]
        lyric: String,
        #[serde(default)]
        phoneme: String,
        #[serde(default)]
        pitch_curve_cents: Vec<i16>,
        #[serde(default)]
        vibrato_depth_cents: u16,
        #[serde(default = "default_vibrato_rate")]
        vibrato_rate_millihz: u16,
        #[serde(default)]
        portamento_samples: u32,
        #[serde(default = "default_probability")]
        probability: u8,
        #[serde(default = "default_repeat_count")]
        repeat_count: u16,
    }
    let Ok(layouts) = serde_json::from_str::<Vec<NoteLayout>>(notes_json) else {
        return Vec::new();
    };
    let mut notes = layouts
        .into_iter()
        .filter_map(|layout| {
            let start_beat = samples_to_beats(layout.start_sample);
            let end_beat =
                samples_to_beats(layout.start_sample.saturating_add(layout.length_samples));
            if !start_beat.is_finite() || !end_beat.is_finite() {
                return None;
            }
            Some(PianoNote {
                track_id: layout.track_id,
                region_id: layout.region_id,
                pitch: layout.pitch.min(127),
                midi_channel: layout.midi_channel.min(15),
                articulation: layout.articulation.min(127),
                velocity: layout.velocity.clamp(1, 127),
                start_beat: start_beat.max(0.0) as f32,
                length_beats: ((end_beat - start_beat).max(0.015625)) as f32,
                lyric: layout.lyric,
                phoneme: layout.phoneme,
                pitch_curve_cents: layout.pitch_curve_cents,
                vibrato_depth_cents: layout.vibrato_depth_cents,
                vibrato_rate_millihz: layout.vibrato_rate_millihz.clamp(500, 20_000),
                portamento_samples: layout.portamento_samples,
                probability: layout.probability.min(100),
                repeat_count: layout.repeat_count.max(1),
            })
        })
        .collect::<Vec<_>>();
    notes.sort_by(|a, b| {
        a.track_id
            .cmp(&b.track_id)
            .then_with(|| a.start_beat.total_cmp(&b.start_beat))
            .then_with(|| a.pitch.cmp(&b.pitch))
    });
    notes
}

/// Index (into `notes`) of the note under a canvas point, if any.
pub fn note_at(notes: &[PianoNote], geometry: &PianoRollGeometry, x: f32, y: f32) -> Option<usize> {
    let pitch = geometry.pitch_at_y(y);
    let beat = geometry.beat_at_x(x);
    notes.iter().position(|note| {
        note.pitch == pitch && beat >= note.start_beat && beat < note.start_beat + note.length_beats
    })
}

/// Pixel layout of one piano-roll canvas, fit-to-width like the arrange
/// canvas. The pitch window fits the visible notes (expanded by two
/// semitones, clamped to 0..127); an empty roll shows a default octave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PianoRollGeometry {
    pub px_per_beat: f32,
    pub px_per_pitch: f32,
    pub top_pitch: u8,
    pub bottom_pitch: u8,
}

impl PianoRollGeometry {
    pub fn x_for_beat(&self, beat: f32) -> f32 {
        beat.max(0.0) * self.px_per_beat
    }

    pub fn y_for_pitch(&self, pitch: u8) -> f32 {
        (self.top_pitch.saturating_sub(pitch) as f32) * self.px_per_pitch
    }

    /// Beat at a canvas x (inverse of [`Self::x_for_beat`]).
    pub fn beat_at_x(&self, x: f32) -> f32 {
        if self.px_per_beat > 0.0 && x.is_finite() {
            (x / self.px_per_beat).max(0.0)
        } else {
            0.0
        }
    }

    /// Pitch at a canvas y (inverse of [`Self::y_for_pitch`]), clamped to
    /// the visible window.
    pub fn pitch_at_y(&self, y: f32) -> u8 {
        if self.px_per_pitch <= 0.0 || !y.is_finite() {
            return self.top_pitch;
        }
        let row = (y / self.px_per_pitch).floor() as i32;
        (i32::from(self.top_pitch) - row)
            .clamp(i32::from(self.bottom_pitch), i32::from(self.top_pitch)) as u8
    }
}

pub fn piano_roll_geometry(
    width: f32,
    height: f32,
    timeline_end_beats: f32,
    notes: &[PianoNote],
) -> PianoRollGeometry {
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    let height = if height.is_finite() {
        height.max(0.0)
    } else {
        0.0
    };
    let timeline_end = if timeline_end_beats.is_finite() {
        timeline_end_beats.max(1.0)
    } else {
        1.0
    };
    let (top_pitch, bottom_pitch) = match (
        notes.iter().map(|note| note.pitch).min(),
        notes.iter().map(|note| note.pitch).max(),
    ) {
        (Some(low), Some(high)) => {
            let top = high.saturating_add(2).min(127);
            let bottom = low.saturating_sub(2);
            // Always show at least an octave so a lone note has context.
            let bottom = bottom.min(top.saturating_sub(12));
            (top, bottom)
        }
        _ => (72, 60),
    };
    let rows = (top_pitch - bottom_pitch + 1).max(1) as f32;
    PianoRollGeometry {
        px_per_beat: width / timeline_end,
        px_per_pitch: height / rows,
        top_pitch,
        bottom_pitch,
    }
}

#[cfg(test)]
#[path = "arrange_model_tests.rs"]
mod tests;
