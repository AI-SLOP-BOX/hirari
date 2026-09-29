pub(crate) fn reset_project_scoped_track_overlays(tracks_model: &slint::VecModel<Z_Track>) {
    crate::ui::project_state::mark_ui_routing_changed();
    for row in 0..tracks_model.row_count() {
        let Some(mut track) = tracks_model.row_data(row) else {
            continue;
        };
        track.piano_roll_notes = slint::ModelRc::default();
        track.eq_low_band = 0.0;
        track.eq_low_cut = 0.0;
        track.eq_high_band = 0.0;
        track.eq_high_cut = 0.0;
        track.input = "IN 1/2".into();
        track.input_first_channel = 0;
        track.input_bus_name = "Stereo In 1–2".into();
        track.input_endpoint_uid = "".into();
        track.input_endpoint_name = "".into();
        let clips = track
            .clips
            .iter()
            .map(|mut clip| {
                clip.selected = false;
                clip.points = slint::ModelRc::default();
                clip
            })
            .collect::<Vec<_>>();
        track.clips = slint::ModelRc::new(slint::VecModel::from(clips));
        tracks_model.set_row_data(row, track);
    }
}

fn sync_tracks_from_engine_with_empty_policy(
    tracks_model: &slint::VecModel<Z_Track>,
    core: &hirari_core_bridge::HirariCore,
    allow_empty: bool,
) -> bool {
    let layout_json = core.get_project_layout_json();
    if layout_json.is_empty() {
        // Keep the current UI snapshot when the native engine has not
        // published a layout yet. Clearing the model here produces a blank
        // canvas during startup/device-offline states and makes a recoverable
        // project look broken. Explicit NEW PROJECT still replaces the model
        // through the command path once the engine state is authoritative.
        return false;
    }

    #[allow(dead_code)]
    #[derive(serde::Deserialize)]
    struct RegionLayout {
        id: u32,
        name: String,
        start: u64,
        len: u64,
        #[serde(default)]
        source_length: u64,
        muted: bool,
        path: String,
        #[serde(default)]
        missing: bool,
        #[serde(default = "default_clip_gain")]
        clip_gain: f32,
        #[serde(default)]
        fade_in_samples: u64,
        #[serde(default)]
        fade_out_samples: u64,
        #[serde(default)]
        reverse: bool,
        #[serde(default = "default_warp_ratio")]
        warp_ratio: f64,
        #[serde(default)]
        pitch_preserve_warp: bool,
        #[serde(default)]
        pitch_semitones: f32,
        #[serde(default = "default_loop_count")]
        loop_count: u32,
        #[serde(default)]
        sync_group: u32,
        #[serde(default)]
        source_offset: u64,
        #[serde(default)]
        base_start: u64,
        #[serde(default)]
        base_source_offset: u64,
        #[serde(default)]
        base_length: u64,
        #[serde(default)]
        audio_note_segments: Vec<AudioPitchSegmentLayout>,
    }

    #[derive(serde::Deserialize)]
    struct AudioPitchSegmentLayout {
        start: f64,
        end: f64,
        detected_pitch_cents: f64,
        #[serde(default)]
        pitch_offset_cents: f64,
        #[serde(default)]
        formant_offset_cents: f64,
        #[serde(default)]
        anchors: Vec<serde_json::Value>,
    }

    fn default_clip_gain() -> f32 {
        1.0
    }

    fn default_warp_ratio() -> f64 {
        1.0
    }

    fn default_loop_count() -> u32 {
        1
    }

    #[allow(dead_code)]
    #[derive(serde::Deserialize)]
    struct TrackLayout {
        id: u32,
        name: String,
        #[serde(rename = "type")]
        track_type: String,
        volume: f32,
        pan: f32,
        mute: bool,
        #[serde(default)]
        solo: bool,
        #[serde(default)]
        record_armed: bool,
        #[serde(default)]
        phase_invert: bool,
        #[serde(default)]
        plugin_types: Vec<u32>,
        #[serde(default)]
        plugin_bypass: Vec<bool>,
        #[serde(default, alias = "trackDelaySamples")]
        track_delay_samples: u32,
        #[serde(default)]
        volume_automation: Vec<AutomationLayoutPoint>,
        #[serde(default)]
        pan_automation: Vec<AutomationLayoutPoint>,
        #[serde(default)]
        track_delay_automation: Vec<AutomationLayoutPoint>,
        #[serde(default)]
        plugin_automation: Vec<PluginAutomationLayoutLane>,
        #[serde(default)]
        frozen: bool,
        #[serde(default)]
        frozen_sample_rate: u32,
        #[serde(default)]
        expression_map: Vec<hirari_core_bridge::project_contracts::ExpressionMapEntry>,
        #[serde(default)]
        expression_map_pro: Option<hirari_core_bridge::expression_map::ExpressionMapPro>,
        regions: Vec<RegionLayout>,
    }

    #[derive(serde::Deserialize)]
    struct AutomationLayoutPoint {
        time: f64,
        value: f32,
        curve: f32,
    }

    #[derive(serde::Deserialize)]
    struct PluginAutomationLayoutLane {
        plugin_index: u32,
        parameter_id: u32,
        #[serde(default)]
        points: Vec<PluginAutomationLayoutPoint>,
    }

    #[derive(serde::Deserialize)]
    struct PluginAutomationLayoutPoint {
        sample: u64,
        normalized: f64,
        #[serde(default)]
        curve: f64,
    }

    if let Ok(layouts) = serde_json::from_str::<Vec<TrackLayout>>(&layout_json) {
        // An empty JSON array means the native side has no published layout
        // yet. Keep the last usable UI snapshot instead of erasing the
        // arrangement into a black canvas during startup/offline recovery.
        if layouts.is_empty() {
            if allow_empty {
                tracks_model.set_vec(Vec::new());
                return true;
            }
            return false;
        }
        // Validate the complete snapshot before mutating the UI model. This
        // prevents stale rows, duplicate IDs, and invalid numeric values from
        // partially replacing a healthy project view.
        let mut track_ids = HashSet::with_capacity(layouts.len());
        let mut region_ids = HashSet::new();
        if layouts.iter().any(|track| {
            !track_ids.insert(track.id)
                || !track.volume.is_finite()
                || !track.pan.is_finite()
                || track.volume < 0.0
                || track.volume > 2.0
                || track.pan < -1.0
                || track.pan > 1.0
                || track.track_delay_samples > 8192
                || track.regions.iter().any(|region| {
                    !region_ids.insert(region.id)
                        || region.id == 0
                        || region.len == 0
                        || !region.clip_gain.is_finite()
                        || !region.warp_ratio.is_finite()
                        || region.warp_ratio < 0.5
                        || region.warp_ratio > 2.0
                })
        }) {
            return false;
        }
        // Native layout telemetry can refresh the track model between project
        // boundaries. Preserve same-session MIDI UI models here; hydration
        // explicitly clears them before rebuilding a different project.
        let existing_notes: HashMap<i32, slint::ModelRc<ZNote>> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .map(|track| (track.id, track.piano_roll_notes.clone()))
            .collect();
        // EQ controls are currently UI-owned until the native project layout
        // exposes a serialized EQ snapshot. Preserve them across telemetry
        // refreshes instead of resetting the visible mixer to four zeros.
        let existing_eq: HashMap<i32, (f32, f32, f32, f32)> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .map(|track| {
                (
                    track.id,
                    (
                        track.eq_low_band,
                        track.eq_low_cut,
                        track.eq_high_band,
                        track.eq_high_cut,
                    ),
                )
            })
            .collect();
        let existing_input: HashMap<i32, slint::SharedString> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .map(|track| (track.id, track.input))
            .collect();
        let existing_input_endpoint: HashMap<i32, slint::SharedString> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .map(|track| (track.id, track.input_endpoint_uid.clone()))
            .collect();
        let existing_input_endpoint_name: HashMap<i32, slint::SharedString> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .map(|track| (track.id, track.input_endpoint_name.clone()))
            .collect();
        let current_input_endpoint = core.audio_input_device_uid();
        let current_input_endpoint_name = core.audio_input_device_name();
        let mut existing_waveforms: HashMap<
            (i32, i32, slint::SharedString),
            slint::ModelRc<f32>,
        > = HashMap::new();
        for row in 0..tracks_model.row_count() {
            let Some(track) = tracks_model.row_data(row) else {
                continue;
            };
            for clip in track.clips.iter() {
                if clip.points.row_count() != 0 && !clip.missing {
                    existing_waveforms.insert(
                        (track.id, clip.id, clip.source_path.clone()),
                        clip.points.clone(),
                    );
                }
            }
        }
        let selected_regions: HashSet<u32> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .flat_map(|track| {
                track
                    .clips
                    .iter()
                    .filter(|clip| clip.selected)
                    .map(|clip| clip.id as u32)
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut new_tracks = Vec::new();
        for (idx, layout) in layouts.into_iter().enumerate() {
            let track_id = layout.id as i32;
            let is_folder = layout.track_type == "FOLD" || layout.track_type == "Bus";
            let color = match idx % 5 {
                0 => slint::Color::from_rgb_u8(120, 130, 143), // Slate
                1 => slint::Color::from_rgb_u8(127, 165, 138), // Sage
                2 => slint::Color::from_rgb_u8(169, 104, 104), // Oxide
                3 => slint::Color::from_rgb_u8(177, 154, 104), // Brass
                _ => slint::Color::from_rgb_u8(130, 148, 154), // Steel
            };

            let clips: Vec<Z_Clip> = layout
                .regions
                .into_iter()
                .map(|r| {
                    let source_path: slint::SharedString = r.path.clone().into();
                    let start_beat = core.samples_to_beats(r.start);
                    let end_beat = core.samples_to_beats(r.start.saturating_add(r.len));
                    let length_beats = (end_beat - start_beat).max(0.0);
                    Z_Clip {
                        id: r.id as i32,
                        name: r.name.into(),
                        source_path: source_path.clone(),
                        start_beat: start_beat as f32,
                        length_beats: length_beats as f32,
                        color,
                        // Peak extraction is queued by telemetry after the
                        // project model is visible. Do not synchronously
                        // decode a large region while constructing the UI.
                        // Keep already decoded source peaks through ordinary
                        // model refreshes. Project hydration clears these
                        // overlays before syncing a different project.
                        points: existing_waveforms
                            .get(&(track_id, r.id as i32, source_path))
                            .cloned()
                            .unwrap_or_default(),
                        fade_in: if r.len == 0 {
                            0.0
                        } else {
                            r.fade_in_samples as f32 / r.len as f32
                        },
                        fade_out: if r.len == 0 {
                            0.0
                        } else {
                            r.fade_out_samples as f32 / r.len as f32
                        },
                        gain: r.clip_gain.clamp(0.0, 2.0),
                        reverse: r.reverse,
                        warp_ratio: r.warp_ratio.clamp(0.5, 2.0) as f32,
                        pitch_preserve_warp: r.pitch_preserve_warp,
                        pitch_semitones: r.pitch_semitones.clamp(-24.0, 24.0),
                        loop_count: r.loop_count.clamp(1, 1024) as i32,
                        sync_group: r.sync_group.min(i32::MAX as u32) as i32,
                        trim_start: if r.base_length == 0 {
                            0.0
                        } else {
                            r.source_offset.saturating_sub(r.base_source_offset) as f32
                                / r.base_length as f32
                        },
                        trim_end: if r.base_length == 0 {
                            1.0
                        } else {
                            (r.source_offset
                                .saturating_sub(r.base_source_offset)
                                .saturating_add(if r.source_length == 0 {
                                    r.len
                                } else {
                                    r.source_length
                                })) as f32
                                / r.base_length as f32
                        },
                        layer: 0,
                        // Selection is UI state, but it must survive a native
                        // telemetry refresh. Otherwise a multi-clip edit
                        // loses the remaining selection after its first
                        // successful Core mutation.
                        selected: selected_regions.contains(&r.id),
                        missing: r.missing,
                        audio_pitch_segments: slint::ModelRc::new(slint::VecModel::from(
                            r.audio_note_segments
                                .into_iter()
                                .filter(|segment| {
                                    segment.start.is_finite()
                                        && segment.end.is_finite()
                                        && segment.end > segment.start
                                        && segment.detected_pitch_cents.is_finite()
                                        && segment.pitch_offset_cents.is_finite()
                                        && segment.formant_offset_cents.is_finite()
                                })
                                .map(|segment| {
                                    let anchor_count = segment.anchors.len().min(i32::MAX as usize) as i32;
                                    // Sustained notes can have thousands of analysis anchors.
                                    // Keep the complete count but publish a bounded curve for UI.
                                    let total_anchors = segment.anchors.len();
                                    let stride = total_anchors.div_ceil(64).max(1);
                                    let anchors = segment
                                        .anchors
                                        .into_iter()
                                        .enumerate()
                                        .filter(|(index, _)| index % stride == 0 || index + 1 == total_anchors)
                                        .filter_map(|(_, anchor)| {
                                            let position = anchor.get("position")?.as_f64()?;
                                            let pitch = anchor.get("pitch_cents")?.as_f64()?;
                                            let formant = anchor.get("formant_cents")?.as_f64()?;
                                            (position.is_finite() && pitch.is_finite() && formant.is_finite()).then_some(
                                                Z_AudioPitchAnchor {
                                                    position_seconds: position as f32,
                                                    pitch_cents: pitch as f32,
                                                    formant_cents: formant as f32,
                                                },
                                            )
                                        })
                                        .take(65)
                                        .collect::<Vec<_>>();
                                    Z_AudioPitchSegment {
                                        start_seconds: segment.start as f32,
                                        end_seconds: segment.end as f32,
                                        detected_pitch_cents: segment.detected_pitch_cents as f32,
                                        pitch_offset_cents: segment.pitch_offset_cents as f32,
                                        formant_offset_cents: segment.formant_offset_cents as f32,
                                        anchor_count,
                                        anchors: slint::ModelRc::new(slint::VecModel::from(anchors)),
                                    }
                                })
                                .collect::<Vec<_>>(),
                        )),
                    }
                })
                .collect();

            let automation_points = |source: Vec<AutomationLayoutPoint>, min_value: f32, max_value: f32| {
                source
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
                        Some(Z_AutomationPoint {
                            beat: core.samples_to_beats(point.time as u64) as f32,
                            value: point.value.clamp(min_value, max_value),
                            curve: point.curve.clamp(-1.0, 1.0),
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let volume_points = automation_points(layout.volume_automation, 0.0, 2.0);
            let pan_points = automation_points(layout.pan_automation, -1.0, 1.0);
            let delay_points = automation_points(layout.track_delay_automation, 0.0, 1.0);
            let fx = layout
                .plugin_types
                .iter()
                .enumerate()
                .map(|(index, plugin_type)| Z_Fx {
                    name: match plugin_type {
                        0 => "Hirari Limiter".into(),
                        1 => "Hirari Compressor".into(),
                        2 => "Hirari Gate".into(),
                        3 => "Tube Saturation".into(),
                        4 => "Hirari Transient".into(),
                        5 => "Hirari De-Esser".into(),
                        6 => "Hirari Delay".into(),
                        7 => "Hirari Reverb".into(),
                        8 => "Hirari Dynamic EQ".into(),
                        9 => "Hirari Mid/Side".into(),
                        10 => "Hirari Width".into(),
                        _ => format!("Plugin {}", index + 1).into(),
                    },
                    active: !layout.plugin_bypass.get(index).copied().unwrap_or(false),
                    has_ui: false,
                })
                .collect::<Vec<_>>();
            let saturate_active =
                layout
                    .plugin_types
                    .iter()
                    .enumerate()
                    .any(|(index, plugin_type)| {
                        *plugin_type == 3
                            && !layout.plugin_bypass.get(index).copied().unwrap_or(false)
                    });
            let mut auto_lane_rows = vec![
                Z_AutomationLane {
                    name: "Volume".into(),
                    color: slint::Color::from_rgb_u8(255, 143, 0),
                    active: !volume_points.is_empty(),
                    plugin_index: -1,
                    parameter_id: -1,
                    points: slint::ModelRc::new(slint::VecModel::from(volume_points)),
                },
                Z_AutomationLane {
                    name: "Pan".into(),
                    color: slint::Color::from_rgb_u8(160, 120, 255),
                    active: !pan_points.is_empty(),
                    plugin_index: -1,
                    parameter_id: -1,
                    points: slint::ModelRc::new(slint::VecModel::from(pan_points)),
                },
                Z_AutomationLane {
                    name: "Track Delay".into(),
                    color: slint::Color::from_rgb_u8(70, 180, 255),
                    active: !delay_points.is_empty(),
                    plugin_index: -1,
                    parameter_id: -1,
                    points: slint::ModelRc::new(slint::VecModel::from(delay_points)),
                },
            ];
            for lane in &layout.plugin_automation {
                let parameter_name = core.get_plugin_parameter_name(
                    layout.id,
                    lane.plugin_index,
                    lane.parameter_id,
                );
                let lane_name = if parameter_name.trim().is_empty() {
                    format!("Plugin {} Parameter {}", lane.plugin_index + 1, lane.parameter_id)
                } else {
                    format!("Plugin {} · {}", lane.plugin_index + 1, parameter_name)
                };
                let points = lane
                    .points
                    .iter()
                    .map(|point| Z_AutomationPoint {
                        beat: core.samples_to_beats(point.sample) as f32,
                        value: point.normalized as f32,
                        curve: point.curve as f32,
                    })
                    .collect::<Vec<_>>();
                let hue = (lane.plugin_index.wrapping_mul(67)
                    .wrapping_add(lane.parameter_id.wrapping_mul(29)) % 156) as u8;
                auto_lane_rows.push(Z_AutomationLane {
                    name: lane_name.into(),
                    color: slint::Color::from_rgb_u8(80 + hue, 180, 120 + (hue / 2)),
                    active: !points.is_empty(),
                    plugin_index: lane.plugin_index as i32,
                    parameter_id: lane.parameter_id as i32,
                    points: slint::ModelRc::new(slint::VecModel::from(points)),
                });
            }
            let auto_lanes = slint::ModelRc::new(slint::VecModel::from(auto_lane_rows));
            let (eq_low_band, eq_low_cut, eq_high_band, eq_high_cut) = existing_eq
                .get(&(layout.id as i32))
                .copied()
                .unwrap_or((0.0, 0.0, 0.0, 0.0));
            let mut articulation_switches = vec![-1; 256];
            let mut articulation_names = (0..256).map(|id| if id == 0 { slint::SharedString::default() } else { format!("Articulation {id}").into() }).collect::<Vec<_>>();
            let mut articulation_switch_channels = vec![0; 256];
            let mut articulation_output_kinds = vec![0; 256];
            let mut articulation_output_a = vec![0; 256];
            let mut articulation_output_b = vec![0; 256];
            for mapping in &layout.expression_map {
                if mapping.articulation_id > 0 {
                    let id = mapping.articulation_id as usize;
                    articulation_names[id] = if mapping.name.trim().is_empty() {
                        format!("Articulation {}", mapping.articulation_id).into()
                    } else {
                        mapping.name.clone().into()
                    };
                    articulation_switch_channels[id] = mapping.channel as i32;
                    let key_switch = mapping.outputs.iter().find_map(|output| match output {
                        hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch { note, .. } => Some(*note),
                        _ => None,
                    }).or(mapping.keyswitch_pitch);
                    if let Some(note) = key_switch {
                        articulation_switches[id] = note as i32;
                    }
                    if let Some((output, off_phase)) = mapping.outputs.iter().map(|output| (output, false))
                        .chain(mapping.off_outputs.iter().map(|output| (output, true)))
                        .rev().find(|(output, _)| !matches!(
                            output, hirari_core_bridge::project_contracts::ExpressionMapOutput::KeySwitch { .. }
                        )) {
                        use hirari_core_bridge::project_contracts::ExpressionMapOutput as Output;
                        let phase_offset = if off_phase { 5 } else { 0 };
                        match output {
                            Output::ProgramChange { program, bank_msb, .. } => {
                                articulation_output_kinds[id] = 1 + phase_offset;
                                articulation_output_a[id] = *program as i32;
                                articulation_output_b[id] = bank_msb.map(i32::from).unwrap_or(-1);
                            }
                            Output::ControlChange { controller, value } => {
                                articulation_output_kinds[id] = 2 + phase_offset;
                                articulation_output_a[id] = *controller as i32;
                                articulation_output_b[id] = *value as i32;
                            }
                            Output::ChannelPressure { value } => {
                                articulation_output_kinds[id] = 3 + phase_offset;
                                articulation_output_a[id] = *value as i32;
                            }
                            Output::PitchBend { value } => {
                                let bend = (*value as i32 + 8192) as u32;
                                articulation_output_kinds[id] = 4 + phase_offset;
                                articulation_output_a[id] = (bend & 0x7f) as i32;
                                articulation_output_b[id] = ((bend >> 7) & 0x7f) as i32;
                            }
                            Output::KeySwitch { .. } => {}
                        }
                    }
                }
            }
            let expression_map_json = serde_json::to_string(&layout.expression_map)
                .unwrap_or_else(|_| "[]".to_owned());
            new_tracks.push(Z_Track {
                id: layout.id as i32,
                name: layout.name.into(),
                r#type: layout.track_type.into(),
                color,
                volume: layout.volume,
                pan: ((layout.pan + 1.0) / 2.0).clamp(0.0, 1.0) * 2.0 - 1.0,
                solo: layout.solo,
                mute: layout.mute,
                armed: layout.record_armed,
                expanded: true,
                show_automation: false,
                send_lvl: 0.0,
                is_stereo: true,
                phase_invert: layout.phase_invert,
                auto_rw: 0,
                notes: "".into(),
                width: 1.0,
                delay_ms: (layout.track_delay_samples as f64 * 1000.0
                    / core.get_sample_rate().max(1.0)) as f32,
                filter_lp: 1.0,
                filter_hp: 0.0,
                icon: if is_folder {
                    "📁".into()
                } else {
                    "🔊".into()
                },
                pan_law: "0dB".into(),
                midi_ch: 1,
                group_id: 0,
                input: existing_input
                    .get(&(layout.id as i32))
                    .cloned()
                    .unwrap_or_else(|| "IN 1/2".into()),
                input_first_channel: existing_input
                    .get(&(layout.id as i32))
                    .and_then(|input| crate::ui::track_model::parse_recording_input_channels(input))
                    .and_then(|channels| channels.first().copied())
                    .unwrap_or(0) as i32,
                input_bus_name: crate::ui::track_model::recording_input_bus_label_from_assignment(
                    existing_input
                        .get(&(layout.id as i32))
                        .map_or("IN 1/2", |input| input.as_str()),
                )
                .into(),
                input_endpoint_uid: existing_input_endpoint
                    .get(&(layout.id as i32))
                    .filter(|uid| !uid.is_empty())
                    .cloned()
                    .unwrap_or_else(|| current_input_endpoint.clone().into()),
                input_endpoint_name: existing_input_endpoint_name
                    .get(&(layout.id as i32))
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| current_input_endpoint_name.clone().into()),
                output: "Main Out".into(),
                monitor: false,
                piano_roll_notes: existing_notes
                    .get(&(layout.id as i32))
                    .cloned()
                    .unwrap_or_default(),
                fx: slint::ModelRc::new(slint::VecModel::from(fx)),
                clips: slint::ModelRc::new(slint::VecModel::from(clips)),
                auto_lanes,
                is_folder,
                parent_id: 0,
                folded: false,
                panner_mode: 0,
                pan3d_x: 0.0,
                pan3d_y: 0.0,
                pan3d_z: 0.0,
                saturate_active,
                artic_map: "".into(),
                articulation_switches: slint::ModelRc::new(slint::VecModel::from(articulation_switches)),
                articulation_names: slint::ModelRc::new(slint::VecModel::from(articulation_names)),
                articulation_switch_channels: slint::ModelRc::new(slint::VecModel::from(articulation_switch_channels)),
                articulation_output_kinds: slint::ModelRc::new(slint::VecModel::from(articulation_output_kinds)),
                articulation_output_a: slint::ModelRc::new(slint::VecModel::from(articulation_output_a)),
                articulation_output_b: slint::ModelRc::new(slint::VecModel::from(articulation_output_b)),
                expression_map_json: expression_map_json.into(),
                expression_map_pro_json: layout.expression_map_pro.as_ref()
                    .and_then(|map| map.to_json().ok())
                    .unwrap_or_else(|| "null".to_owned())
                    .into(),
                correlation: 0.0,
                eq_low_band,
                eq_low_cut,
                eq_high_band,
                eq_high_cut,
                frozen: layout.frozen,
                frozen_sample_rate: layout.frozen_sample_rate.min(i32::MAX as u32) as i32,
            });
        }
        tracks_model.set_vec(new_tracks);
        return true;
    }
    false
}

pub(crate) fn fallback_template_tracks(template_tracks: &[(&str, u32)]) -> Vec<Z_Track> {
    template_tracks
        .iter()
        .enumerate()
        .map(|(index, (name, type_id))| {
            let is_bus = *type_id == 3;
            let track_type = match type_id {
                1 => "MIDI",
                2 => "Instrument",
                3 => "Bus",
                4 => "Vocal",
                _ => "Audio",
            };
            let color = match index % 5 {
                0 => slint::Color::from_rgb_u8(65, 122, 166),
                1 => slint::Color::from_rgb_u8(89, 137, 105),
                2 => slint::Color::from_rgb_u8(156, 119, 76),
                3 => slint::Color::from_rgb_u8(125, 97, 137),
                _ => slint::Color::from_rgb_u8(119, 131, 137),
            };
            Z_Track {
                id: index as i32,
                name: (*name).into(),
                r#type: track_type.into(),
                color,
                expanded: true,
                volume: 0.8,
                pan: 0.0,
                solo: false,
                mute: false,
                armed: false,
                is_stereo: true,
                phase_invert: false,
                auto_rw: 0,
                fx: slint::ModelRc::default(),
                clips: slint::ModelRc::default(),
                auto_lanes: slint::ModelRc::default(),
                show_automation: false,
                send_lvl: 0.0,
                notes: "Template track".into(),
                width: 1.0,
                delay_ms: 0.0,
                filter_lp: 1.0,
                filter_hp: 0.0,
                icon: if is_bus { "BUS".into() } else { "TRK".into() },
                pan_law: "0dB".into(),
                midi_ch: 1,
                group_id: 0,
                input: "IN 1/2".into(),
                input_first_channel: 0,
                input_bus_name: "Stereo In 1–2".into(),
                input_endpoint_uid: "".into(),
                input_endpoint_name: "".into(),
                output: "Main Out".into(),
                monitor: false,
                piano_roll_notes: slint::ModelRc::default(),
                is_folder: is_bus,
                parent_id: 0,
                folded: false,
                panner_mode: 0,
                pan3d_x: 0.0,
                pan3d_y: 0.0,
                pan3d_z: 0.0,
                saturate_active: false,
                artic_map: "".into(),
                articulation_switches: slint::ModelRc::new(slint::VecModel::from(vec![-1; 256])),
                articulation_names: slint::ModelRc::new(slint::VecModel::from((0..256).map(|id| if id == 0 { slint::SharedString::default() } else { format!("Articulation {id}").into() }).collect::<Vec<_>>())),
                articulation_switch_channels: slint::ModelRc::new(slint::VecModel::from(vec![0; 256])),
                articulation_output_kinds: slint::ModelRc::new(slint::VecModel::from(vec![0; 256])),
                articulation_output_a: slint::ModelRc::new(slint::VecModel::from(vec![0; 256])),
                articulation_output_b: slint::ModelRc::new(slint::VecModel::from(vec![0; 256])),
                expression_map_json: "[]".into(),
                expression_map_pro_json: "null".into(),
                correlation: 0.0,
                eq_low_band: 0.0,
                eq_low_cut: 0.0,
                eq_high_band: 0.0,
                eq_high_cut: 0.0,
                frozen: false,
                frozen_sample_rate: 0,
            }
        })
        .collect()
}
