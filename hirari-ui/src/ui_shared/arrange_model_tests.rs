//! Tests split out of arrange_model.rs.

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
    let json = r#"[{"id":7,"name":"Drums","type":"Audio","volume":0.5,"pan":-0.25,"mute":true,"solo":true,"record_armed":true,"expression_map":[{"articulation_id":1,"name":"Short","keyswitch_pitch":36}],"plugin_types":[0,7],"plugin_bypass":[false,true],"regions":[
            {"id":1,"name":"Kick","start":0,"len":48000,"sync_group":3,"fade_in_samples":12000,"fade_out_samples":6000,"clip_gain":1.5},
            {"id":2,"name":"Snare","start":96000,"len":48000,"reverse":true}]}]"#;
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
    assert_eq!(tracks[0].clips[0].sync_group, 3);
    assert_eq!(tracks[0].clips[1].sync_group, 0);
    assert!(!tracks[0].clips[0].reverse);
    assert!(tracks[0].clips[1].reverse);
    assert_eq!(tracks[0].clips[0].fade_in, 0.25);
    assert_eq!(tracks[0].clips[0].fade_out, 0.125);
    assert_eq!(tracks[0].clips[0].gain, 1.5);
    assert_eq!(tracks[0].volume, 0.5);
    assert_eq!(tracks[0].pan, -0.25);
    assert!(tracks[0].muted);
    assert!(tracks[0].solo);
    assert!(tracks[0].record_armed);
    assert_eq!(tracks[0].expression_map.len(), 1);
    assert_eq!(tracks[0].expression_map[0].keyswitch_pitch, Some(36));
    assert_eq!(tracks[0].plugin_types, vec![0, 7]);
    assert_eq!(tracks[0].plugin_bypass, vec![false, true]);
    // No base_length published: full-clip trim window.
    assert_eq!(tracks[0].clips[0].trim_start, 0.0);
    assert_eq!(tracks[0].clips[0].trim_end, 1.0);
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
    // Unparseable and missing colors take the sync default.
    assert_eq!(markers[0].color, [0x64, 0x72, 0xa8]);
    let markers = parse_markers(r#"[{"id":3,"label":"C","beat":2.0}]"#);
    assert_eq!(markers[0].color, [0x64, 0x72, 0xa8]);
}

#[test]
fn canvas_geometry_fits_timeline_to_width() {
    // 128 beats across the 536px after the 104px header is 4.1875
    // px/beat; two lanes split the area below the 24px ruler.
    let geometry = arrange_canvas_geometry(640.0, 224.0, 128.0, 2, 1.0, 0.0);
    assert_eq!(geometry.header_width, 104.0);
    assert_eq!(geometry.px_per_beat, 4.1875);
    assert_eq!(geometry.ruler_height, 24.0);
    assert_eq!(geometry.lane_height, 100.0);
    assert_eq!(geometry.x_for_beat(10.0), 145.875);
    assert_eq!(geometry.x_for_beat(-1.0), 99.8125);
    assert_eq!(geometry.beat_for_x(145.875), 10.0);
    assert_eq!(geometry.beat_for_x(-5.0), 0.0);
    assert_eq!(
        arrange_canvas_geometry(0.0, 0.0, 128.0, 0, 1.0, 0.0).beat_for_x(50.0),
        0.0
    );
    assert_eq!(geometry.y_for_lane(1), 124.0);
}

#[test]
fn canvas_geometry_handles_degenerate_sizes() {
    let geometry = arrange_canvas_geometry(0.0, 0.0, 128.0, 0, 1.0, 0.0);
    assert_eq!(geometry.px_per_beat, 0.0);
    assert_eq!(geometry.lane_height, 0.0);
    assert!(geometry.beat_step.is_finite());
    let geometry = arrange_canvas_geometry(f32::NAN, f32::INFINITY, f32::NAN, 3, 1.0, 0.0);
    assert_eq!(geometry.px_per_beat, 0.0);
    assert_eq!(geometry.lane_height, 0.0);
}

#[test]
fn canvas_grid_step_adapts_to_zoom() {
    // At 20 px/beat the grid draws every beat; zoomed far out (below
    // 4 px/beat) it steps by whole beats only when lines would merge.
    assert_eq!(
        arrange_canvas_geometry(2560.0, 100.0, 128.0, 1, 1.0, 0.0).beat_step,
        1.0
    );
    // 128 beats in the 128px timeline after the 104px header is
    // 1 px/beat, so the step is 4 beats.
    assert_eq!(
        arrange_canvas_geometry(232.0, 100.0, 128.0, 1, 1.0, 0.0).beat_step,
        4.0
    );
}

#[test]
fn canvas_zoom_scales_and_scroll_shifts() {
    // Fit is 4.1875 px/beat; zoom 2 doubles it and scroll 10 moves
    // the window so beat 10 sits at the header edge and beat 20 at
    // header + 83.75.
    let geometry = arrange_canvas_geometry(640.0, 224.0, 128.0, 2, 2.0, 10.0);
    assert_eq!(geometry.px_per_beat, 8.375);
    assert_eq!(geometry.scroll_beats, 10.0);
    assert_eq!(geometry.x_for_beat(10.0), 104.0);
    assert_eq!(geometry.x_for_beat(20.0), 187.75);
    assert_eq!(geometry.beat_for_x(187.75), 20.0);
}

#[test]
fn canvas_zoom_and_scroll_are_clamped() {
    // Zoom clamps to [1, 8]; at zoom 8 the 536px timeline shows 16
    // beats, so scroll clamps to 128 - 16 = 112.
    let geometry = arrange_canvas_geometry(640.0, 224.0, 128.0, 2, 100.0, 999.0);
    assert_eq!(geometry.px_per_beat, 33.5);
    assert_eq!(geometry.scroll_beats, 112.0);
    let geometry = arrange_canvas_geometry(640.0, 224.0, 128.0, 2, 0.1, -5.0);
    assert_eq!(geometry.px_per_beat, 4.1875);
    assert_eq!(geometry.scroll_beats, 0.0);
    // At fit zoom there is nothing to scroll.
    let geometry = arrange_canvas_geometry(640.0, 224.0, 128.0, 2, 1.0, 50.0);
    assert_eq!(geometry.scroll_beats, 0.0);
}

#[test]
fn clip_at_finds_clip_under_point() {
    let tracks = vec![ArrangeTrack {
        id: 7,
        name: "A".into(),
        kind: "Audio".into(),
        color: [0, 0, 0],
        volume: 1.0,
        pan: 0.0,
        muted: false,
        solo: false,
        record_armed: false,
        expression_map: Vec::new(),
        plugin_types: Vec::new(),
        plugin_bypass: Vec::new(),
        volume_automation: Vec::new(),
        pan_automation: Vec::new(),
        clips: vec![
            ArrangeClip {
                id: 1,
                name: "a".into(),
                start_beat: 0.0,
                length_beats: 4.0,
                midi: false,
                sync_group: 0,
                trim_start: 0.0,
                trim_end: 1.0,
                reverse: false,
                fade_in: 0.0,
                fade_out: 0.0,
                pitch_semitones: 0.0,
                warp_ratio: 1.0,
                pitch_preserve_warp: false,

                loop_count: 1,

                gain: 1.0,
            },
            ArrangeClip {
                id: 2,
                name: "b".into(),
                start_beat: 8.0,
                length_beats: 4.0,
                midi: false,
                sync_group: 0,
                trim_start: 0.0,
                trim_end: 1.0,
                reverse: false,
                fade_in: 0.0,
                fade_out: 0.0,
                pitch_semitones: 0.0,
                warp_ratio: 1.0,
                pitch_preserve_warp: false,

                loop_count: 1,

                gain: 1.0,
            },
        ],
    }];
    // 536px / 128 beats = 4.1875 px/beat after the 104px header,
    // ruler 24, lane 100. Beat 2 is at x=112.375, beat 9 at 141.6875.
    let geometry = arrange_canvas_geometry(640.0, 124.0, 128.0, 1, 1.0, 0.0);
    assert_eq!(clip_at(&tracks, &geometry, 112.375, 50.0), Some((7, 1)));
    assert_eq!(clip_at(&tracks, &geometry, 141.6875, 50.0), Some((7, 2)));
    // Empty lane space, ruler band and below the lane miss. Beat 6
    // (x=129.125) is the gap between the clips.
    assert_eq!(clip_at(&tracks, &geometry, 129.125, 50.0), None);
    assert_eq!(clip_at(&tracks, &geometry, 10.0, 10.0), None);
    assert_eq!(clip_at(&tracks, &geometry, 10.0, 200.0), None);
    // Zoom 2 (8.375 px/beat) scrolled 8: clip b (8..12) starts at
    // the header edge; beat 8.5 is at x=108.1875.
    let geometry = arrange_canvas_geometry(640.0, 124.0, 128.0, 1, 2.0, 8.0);
    assert_eq!(clip_at(&tracks, &geometry, 108.1875, 50.0), Some((7, 2)));
}

#[test]
fn parses_trim_window_from_layout() {
    // Halfway through a 96000-sample source, showing 48000 of it.
    let json = r#"[{"id":7,"name":"T","type":"Audio","volume":1.0,"pan":0.0,"mute":false,"regions":[
            {"id":1,"name":"R","start":0,"len":48000,"source_offset":24000,
             "base_source_offset":0,"base_length":96000,"source_length":48000}]}]"#;
    let tracks = parse_arrange_tracks(json, |samples| samples as f64 / 48000.0);
    assert_eq!(tracks[0].clips[0].trim_start, 0.25);
    assert_eq!(tracks[0].clips[0].trim_end, 0.75);
}

#[test]
fn trim_edge_only_on_wide_clip_edges() {
    // 4.1875 px/beat: clip 0..4 beats spans the header edge x=104
    // to x=120.75, edges within 6px of either side.
    let geometry = arrange_canvas_geometry(640.0, 124.0, 128.0, 1, 1.0, 0.0);
    let clip = ArrangeClip {
        id: 1,
        name: "a".into(),
        start_beat: 0.0,
        length_beats: 4.0,
        midi: false,
        sync_group: 0,
        trim_start: 0.0,
        trim_end: 1.0,
        reverse: false,
        fade_in: 0.0,
        fade_out: 0.0,
        pitch_semitones: 0.0,
        warp_ratio: 1.0,
        pitch_preserve_warp: false,
        loop_count: 1,
        gain: 1.0,
    };
    assert_eq!(trim_edge_at(&geometry, &clip, 107.0), Some(TrimEdge::Left));
    assert_eq!(trim_edge_at(&geometry, &clip, 118.0), Some(TrimEdge::Right));
    assert_eq!(trim_edge_at(&geometry, &clip, 112.0), None);
    // A 1-beat clip is narrower than both zones: no trim.
    let narrow = ArrangeClip {
        length_beats: 1.0,
        ..clip
    };
    assert_eq!(trim_edge_at(&geometry, &narrow, 2.0), None);
}

#[test]
fn parses_midi_notes_sorted_and_sanitized() {
    let json = r#"[
            {"track_id":1,"pitch":72,"velocity":100,"start_sample":960,"length_samples":240},
            {"track_id":1,"pitch":60,"velocity":0,"start_sample":0,"length_samples":0},
            {"track_id":0,"pitch":200,"velocity":100,"start_sample":0,"length_samples":480}]"#;
    // 480 samples == 1 beat in this fixture's conversion.
    let notes = parse_midi_notes(json, |samples| samples as f64 / 480.0);
    assert_eq!(notes.len(), 3);
    // Sorted by track, then start: track 0 first.
    assert_eq!(notes[0].track_id, 0);
    assert_eq!(notes[0].pitch, 127);
    assert_eq!(notes[1].velocity, 1);
    assert_eq!(notes[1].length_beats, 0.015625);
    assert_eq!(notes[2].start_beat, 2.0);
    assert_eq!(notes[2].probability, 100);
    assert_eq!(notes[2].repeat_count, 1);
    assert!(parse_midi_notes("nope", |_| 0.0).is_empty());
}

#[test]
fn piano_geometry_fits_notes_and_inverts() {
    let notes = vec![
        PianoNote {
            track_id: 0,
            pitch: 60,
            velocity: 100,
            start_beat: 0.0,
            length_beats: 1.0,
            region_id: 0,
            midi_channel: 0,
            articulation: 0,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            vibrato_rate_millihz: 5_000,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        },
        PianoNote {
            track_id: 0,
            pitch: 72,
            velocity: 90,
            start_beat: 4.0,
            length_beats: 2.0,
            region_id: 0,
            midi_channel: 0,
            articulation: 0,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            vibrato_rate_millihz: 5_000,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        },
    ];
    // 640px / 128 beats = 5 px/beat; pitches 58..74 = 17 rows.
    let geometry = piano_roll_geometry(640.0, 170.0, 128.0, &notes);
    assert_eq!(geometry.px_per_beat, 5.0);
    assert_eq!(geometry.top_pitch, 74);
    assert_eq!(geometry.bottom_pitch, 58);
    assert_eq!(geometry.px_per_pitch, 10.0);
    assert_eq!(geometry.x_for_beat(4.0), 20.0);
    assert_eq!(geometry.beat_at_x(20.0), 4.0);
    assert_eq!(geometry.y_for_pitch(74), 0.0);
    assert_eq!(geometry.pitch_at_y(0.0), 74);
    assert_eq!(geometry.pitch_at_y(165.0), 58);
    // Empty roll shows the default window.
    let empty = piano_roll_geometry(640.0, 130.0, 128.0, &[]);
    assert_eq!((empty.top_pitch, empty.bottom_pitch), (72, 60));
}

#[test]
fn note_at_hits_pitch_and_beat() {
    let notes = vec![
        PianoNote {
            track_id: 0,
            region_id: 0,
            pitch: 60,
            midi_channel: 0,
            articulation: 0,
            velocity: 100,
            start_beat: 0.0,
            length_beats: 1.0,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            vibrato_rate_millihz: 5_000,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        },
        PianoNote {
            track_id: 0,
            region_id: 0,
            pitch: 72,
            midi_channel: 0,
            articulation: 0,
            velocity: 90,
            start_beat: 4.0,
            length_beats: 2.0,
            lyric: String::new(),
            phoneme: String::new(),
            pitch_curve_cents: Vec::new(),
            vibrato_depth_cents: 0,
            vibrato_rate_millihz: 5_000,
            portamento_samples: 0,
            probability: 100,
            repeat_count: 1,
        },
    ];
    let geometry = piano_roll_geometry(640.0, 170.0, 128.0, &notes);
    // Pitch 72 is the top row's 3rd line down (74,73,72): y=25 hits it
    // at beat 5 (x=25).
    assert_eq!(note_at(&notes, &geometry, 25.0, 25.0), Some(1));
    assert_eq!(note_at(&notes, &geometry, 25.0, 5.0), None);
}

#[test]
fn plugin_names_follow_the_builtin_chain() {
    assert_eq!(plugin_name(0, 0), "Hirari Limiter");
    assert_eq!(plugin_name(3, 0), "Tube Saturation");
    assert_eq!(plugin_name(99, 4), "Plugin 5");
}

#[test]
fn parses_automation_points_clamped_and_filtered() {
    // 48000 samples == 1 beat here. Volume clamps to 0..2, pan to
    // -1..1; a non-integer sample time drops.
    let json = r#"[{"id":7,"name":"T","type":"Audio","volume":1.0,"pan":0.0,"mute":false,
            "volume_automation":[{"time":48000,"value":1.5,"curve":0.5},{"time":96000,"value":3.0,"curve":0.0},{"time":48000.5,"value":1.0,"curve":0.0}],
            "pan_automation":[{"time":48000,"value":-2.0,"curve":0.0}]}]"#;
    let tracks = parse_arrange_tracks(json, |samples| samples as f64 / 48000.0);
    assert_eq!(tracks[0].volume_automation.len(), 2);
    assert_eq!(tracks[0].volume_automation[0].beat, 1.0);
    assert_eq!(tracks[0].volume_automation[0].value, 1.5);
    assert_eq!(tracks[0].volume_automation[1].value, 2.0);
    assert_eq!(tracks[0].pan_automation.len(), 1);
    assert_eq!(tracks[0].pan_automation[0].value, -1.0);
}

#[test]
fn parses_audio_devices() {
    let json = r#"[{"id":12,"name":"Built-in Output"},{"id":13,"name":"USB Mic"}]"#;
    let devices = parse_audio_devices(json);
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[1].id, 13);
    assert_eq!(devices[1].name, "USB Mic");
    assert!(parse_audio_devices("nope").is_empty());
}

#[test]
fn parses_preview_catalog() {
    let json = r#"{"ok":true,"assets":[
            {"id":5,"name":"Kick","path":"/snd/kick.wav","sample_rate":48000,"size":12345},
            {"id":6,"name":"","path":"/snd/snare.wav"}]}"#;
    let assets = parse_preview_catalog(json);
    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0].id, 5);
    assert_eq!(assets[0].size_bytes, 12345);
    assert_eq!(assets[1].name, "Audio");
    assert!(parse_preview_catalog("nope").is_empty());
}

#[test]
fn last_clip_end_spans_all_lanes() {
    let tracks = vec![
        ArrangeTrack {
            id: 1,
            name: "A".into(),
            kind: "Audio".into(),
            color: [0, 0, 0],
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            record_armed: false,
            expression_map: Vec::new(),
            plugin_types: Vec::new(),
            plugin_bypass: Vec::new(),
            volume_automation: Vec::new(),
            pan_automation: Vec::new(),
            clips: vec![ArrangeClip {
                id: 1,
                name: "a".into(),
                start_beat: 2.0,
                length_beats: 3.0,
                midi: false,
                sync_group: 0,
                trim_start: 0.0,
                trim_end: 1.0,
                reverse: false,
                fade_in: 0.0,
                fade_out: 0.0,
                pitch_semitones: 0.0,
                warp_ratio: 1.0,
                pitch_preserve_warp: false,

                loop_count: 1,

                gain: 1.0,
            }],
        },
        ArrangeTrack {
            id: 2,
            name: "B".into(),
            kind: "Audio".into(),
            color: [0, 0, 0],
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            record_armed: false,
            expression_map: Vec::new(),
            plugin_types: Vec::new(),
            plugin_bypass: Vec::new(),
            volume_automation: Vec::new(),
            pan_automation: Vec::new(),
            clips: vec![ArrangeClip {
                id: 2,
                name: "b".into(),
                start_beat: 10.0,
                length_beats: 4.0,
                midi: false,
                sync_group: 0,
                trim_start: 0.0,
                trim_end: 1.0,
                reverse: false,
                fade_in: 0.0,
                fade_out: 0.0,
                pitch_semitones: 0.0,
                warp_ratio: 1.0,
                pitch_preserve_warp: false,

                loop_count: 1,

                gain: 1.0,
            }],
        },
    ];
    assert_eq!(last_clip_end(&tracks), 14.0);
}
