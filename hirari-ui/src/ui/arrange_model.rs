//! Backend-neutral arrangement model and timeline geometry.
//!
//! The arranger needs the same facts regardless of windowing backend: which
//! tracks and clips exist, where the ruler's measure lines fall, how long the
//! timeline runs, and how MIDI note runs collapse into visible clips. These
//! routines were lifted out of the Slint telemetry and track-sync paths so the
//! Slint surface and the AmeUI Canvas derive identical arrangement geometry
//! from one source during the migration. Nothing here depends on Slint or on a
//! live engine; the engine is reached only through the `samples_to_beats`
//! closure passed to [`parse_arrange_tracks`], which keeps the parsers testable.

use serde::Deserialize;

/// One ruler measure line. `number` is the 1-based measure index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasureStart {
    pub beat: f32,
    pub number: i32,
}

/// A visible arrangement clip. Audio clips carry their native region id;
/// synthesized MIDI clips carry a negative id and `midi == true` so region
/// mutations are never offered for them.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangeClip {
    pub id: i32,
    pub name: String,
    pub start_beat: f32,
    pub length_beats: f32,
    pub midi: bool,
}

/// One arrangement lane reduced to what the timeline draws.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangeTrack {
    pub id: u32,
    pub name: String,
    pub kind: String,
    pub color: [u8; 3],
    pub clips: Vec<ArrangeClip>,
}

/// A timeline marker. `color` is the engine's `#rrggbb` marker color as
/// RGB, matching the Slint `Z_Marker` color so a project keeps its marker
/// colors across the backend migration.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangeMarker {
    pub id: u32,
    pub label: String,
    pub beat: f64,
    pub color: [u8; 3],
}

/// The rotating lane palette shared with the Slint track sync so a project
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
}

#[derive(Deserialize)]
struct TrackLayout {
    id: u32,
    name: String,
    #[serde(rename = "type")]
    track_type: String,
    #[serde(default)]
    regions: Vec<RegionLayout>,
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
                    }
                })
                .collect();
            ArrangeTrack {
                id: layout.id,
                name: layout.name,
                kind: layout.track_type,
                color: track_color(index),
                clips,
            }
        })
        .collect()
}

/// Parses the engine marker snapshot, dropping invalid rows. A missing or
/// malformed `color` falls back to the same default the Slint marker sync
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_end_defaults_to_128_beats_for_empty_projects() {
        assert_eq!(arrangement_timeline_end(0.0, &[]), 128.0);
    }

    #[test]
    fn timeline_end_rounds_up_to_a_bar_boundary() {
        // 4/4: a clip ending mid-bar 5 (beat 18) rounds to beat 20, but the
        // 128-beat floor still dominates a short project.
        assert_eq!(arrangement_timeline_end(18.0, &[]), 128.0);
        // A long project ending at beat 130 rounds up to the next bar (132).
        assert_eq!(arrangement_timeline_end(130.0, &[]), 132.0);
    }

    #[test]
    fn timeline_end_respects_a_meter_change() {
        // 3/4 from beat 0 means 3-beat bars; 130 rounds to 132.
        let meters = valid_time_signatures(&[0.0, 3.0, 4.0]);
        assert_eq!(arrangement_timeline_end(130.0, &meters), 132.0);
    }

    #[test]
    fn timeline_end_snaps_to_a_meter_change_after_the_clip() {
        // 4/4 then 3/4 at beat 18; a clip ending at 17 rounds to the change.
        let meters = valid_time_signatures(&[0.0, 4.0, 4.0, 18.0, 3.0, 4.0]);
        assert_eq!(arrangement_timeline_end(17.0, &meters), 128.0);
        // Past the 128-beat floor the same snap lands on the change itself.
        let meters = valid_time_signatures(&[0.0, 4.0, 4.0, 130.0, 3.0, 4.0]);
        assert_eq!(arrangement_timeline_end(129.0, &meters), 130.0);
    }

    #[test]
    fn timeline_end_snaps_to_a_meter_change_inside_the_bar() {
        // 2/4 at beat 6 interrupts the 4/4 bar 4..8; a clip ending at 5
        // rounds to the interrupting change at 6.
        let meters = valid_time_signatures(&[0.0, 4.0, 4.0, 6.0, 2.0, 4.0]);
        assert_eq!(arrangement_timeline_end(5.0, &meters), 128.0);
        let meters = valid_time_signatures(&[0.0, 4.0, 4.0, 134.0, 2.0, 4.0]);
        assert_eq!(arrangement_timeline_end(133.0, &meters), 134.0);
    }

    #[test]
    fn measure_starts_count_four_four_bars() {
        // A measure starting exactly at `end_beat` is past the timeline, so
        // the starts run through the bar before it.
        let starts = arrangement_measure_starts(&[], 16.0);
        let beats = starts.iter().map(|m| m.beat).collect::<Vec<_>>();
        assert_eq!(beats, vec![0.0, 4.0, 8.0, 12.0]);
        assert_eq!(starts.last().unwrap().number, 4);
    }

    #[test]
    fn measure_starts_follow_a_meter_change() {
        // 4/4 then 3/4 at beat 8: bars at 0,4,8,11,14.
        let starts = arrangement_measure_starts(&[0.0, 4.0, 4.0, 8.0, 3.0, 4.0], 16.0);
        let beats = starts.iter().map(|m| m.beat).collect::<Vec<_>>();
        assert_eq!(beats, vec![0.0, 4.0, 8.0, 11.0, 14.0]);
    }

    #[test]
    fn invalid_time_signatures_are_dropped() {
        // Denominator 3 is not a power of two, so the middle event drops;
        // 5/4 is a valid meter and is kept.
        let meters = valid_time_signatures(&[0.0, 4.0, 4.0, 4.0, 4.0, 3.0, 8.0, 3.0, 4.0]);
        assert_eq!(meters, vec![(0.0, 4, 4), (8.0, 3, 4)]);
        let meters = valid_time_signatures(&[0.0, 5.0, 4.0]);
        assert_eq!(meters, vec![(0.0, 5, 4)]);
    }

    #[test]
    fn midi_spans_group_by_contiguity() {
        // Three adjacent notes then an isolated note five beats later.
        let groups = group_midi_spans(vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0), (8.0, 9.0)]);
        assert_eq!(groups, vec![(0.0, 3.0), (8.0, 9.0)]);
    }

    #[test]
    fn midi_spans_sort_unordered_input() {
        let groups = group_midi_spans(vec![(8.0, 9.0), (0.0, 1.0), (1.0, 2.0)]);
        assert_eq!(groups, vec![(0.0, 2.0), (8.0, 9.0)]);
    }

    #[test]
    fn parses_tracks_and_regions_from_layout_json() {
        let json = r#"[{"id":7,"name":"Drums","type":"Audio","regions":[
            {"id":1,"name":"Kick","start":0,"len":48000},
            {"id":2,"name":"Snare","start":96000,"len":48000}]}]"#;
        // 48000 samples == 1 beat in this fixture's conversion.
        let tracks = parse_arrange_tracks(json, |samples| samples as f64 / 48000.0);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, 7);
        assert_eq!(tracks[0].kind, "Audio");
        assert_eq!(tracks[0].clips.len(), 2);
        assert_eq!(tracks[0].clips[0].start_beat, 0.0);
        assert_eq!(tracks[0].clips[0].length_beats, 1.0);
        assert_eq!(tracks[0].clips[1].start_beat, 2.0);
        assert!(!tracks[0].clips[0].midi);
    }

    #[test]
    fn malformed_layout_json_yields_no_tracks() {
        assert!(parse_arrange_tracks("not json", |_| 0.0).is_empty());
        assert!(parse_arrange_tracks("[]", |_| 0.0).is_empty());
    }

    #[test]
    fn markers_parse_sorted_and_drop_invalid() {
        let json = r##"[{"id":2,"label":"B","beat":8.0,"color":"#ff0000"},{"id":0,"label":"bad","beat":1.0},
            {"id":1,"label":"A","beat":4.0,"color":"not-a-color"}]"##;
        let markers = parse_markers(json);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].id, 1);
        assert_eq!(markers[1].beat, 8.0);
        assert_eq!(markers[1].color, [255, 0, 0]);
        // Unparseable and missing colors take the Slint sync's default.
        assert_eq!(markers[0].color, [0x64, 0x72, 0xa8]);
        let markers = parse_markers(r#"[{"id":3,"label":"C","beat":2.0}]"#);
        assert_eq!(markers[0].color, [0x64, 0x72, 0xa8]);
    }

    #[test]
    fn last_clip_end_spans_all_lanes() {
        let tracks = vec![
            ArrangeTrack {
                id: 1,
                name: "A".into(),
                kind: "Audio".into(),
                color: [0, 0, 0],
                clips: vec![ArrangeClip {
                    id: 1,
                    name: "a".into(),
                    start_beat: 2.0,
                    length_beats: 3.0,
                    midi: false,
                }],
            },
            ArrangeTrack {
                id: 2,
                name: "B".into(),
                kind: "Audio".into(),
                color: [0, 0, 0],
                clips: vec![ArrangeClip {
                    id: 2,
                    name: "b".into(),
                    start_beat: 10.0,
                    length_beats: 4.0,
                    midi: false,
                }],
            },
        ];
        assert_eq!(last_clip_end(&tracks), 14.0);
    }
}
