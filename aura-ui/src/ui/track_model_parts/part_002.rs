fn sync_tracks_from_engine_with_empty_policy(
    tracks_model: &slint::VecModel<Z_Track>,
    core: &aura_core_bridge::AuraCore,
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
        pitch_semitones: f32,
        #[serde(default = "default_loop_count")]
        loop_count: u32,
        #[serde(default)]
        source_offset: u64,
        #[serde(default)]
        base_start: u64,
        #[serde(default)]
        base_source_offset: u64,
        #[serde(default)]
        base_length: u64,
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
        frozen: bool,
        #[serde(default)]
        frozen_sample_rate: u32,
        regions: Vec<RegionLayout>,
    }

    #[derive(serde::Deserialize)]
    struct AutomationLayoutPoint {
        time: f64,
        value: f32,
        curve: f32,
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
                        || region.warp_ratio < 0.25
                        || region.warp_ratio > 4.0
                })
        }) {
            return false;
        }
        // Native layout telemetry can refresh the track model after project
        // hydration. Keep the UI-owned MIDI model across that refresh; the
        // previous implementation rebuilt every track with an empty note
        // model, making a valid large MIDI arrangement disappear seconds
        // after restore.
        let existing_notes: HashMap<i32, slint::ModelRc<ZNote>> = (0..tracks_model.row_count())
            .filter_map(|row| tracks_model.row_data(row))
            .map(|track| (track.id, track.piano_roll_notes.clone()))
            .collect();
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
                    let start_beat = core.samples_to_beats(r.start);
                    let end_beat = core.samples_to_beats(r.start.saturating_add(r.len));
                    let length_beats = (end_beat - start_beat).max(0.0);
                    Z_Clip {
                        id: r.id as i32,
                        name: r.name.into(),
                        start_beat: start_beat as f32,
                        length_beats: length_beats as f32,
                        color,
                        // Peak extraction is queued by telemetry after the
                        // project model is visible. Do not synchronously
                        // decode a large region while constructing the UI.
                        points: slint::ModelRc::new(slint::VecModel::default()),
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
                        warp_ratio: r.warp_ratio.clamp(0.25, 4.0) as f32,
                        pitch_semitones: r.pitch_semitones.clamp(-24.0, 24.0),
                        loop_count: r.loop_count.clamp(1, 1024) as i32,
                        trim_start: if r.base_length == 0 {
                            0.0
                        } else {
                            r.source_offset.saturating_sub(r.base_source_offset) as f32
                                / r.base_length as f32
                        },
                        trim_end: if r.base_length == 0 {
                            1.0
                        } else {
                            (r.source_offset.saturating_sub(r.base_source_offset) + r.len) as f32
                                / r.base_length as f32
                        },
                        layer: 0,
                        // Selection is UI state, but it must survive a native
                        // telemetry refresh. Otherwise a multi-clip edit
                        // loses the remaining selection after its first
                        // successful Core mutation.
                        selected: selected_regions.contains(&r.id),
                        missing: r.missing,
                    }
                })
                .collect();

            let automation_points = |source: Vec<AutomationLayoutPoint>| {
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
                            value: point.value.clamp(0.0, 1.0),
                            curve: point.curve.clamp(-1.0, 1.0),
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let volume_points = automation_points(layout.volume_automation);
            let pan_points = automation_points(layout.pan_automation);
            let delay_points = automation_points(layout.track_delay_automation);
            let fx = layout
                .plugin_types
                .iter()
                .enumerate()
                .map(|(index, _)| Z_Fx {
                    name: format!("Plugin {}", index + 1).into(),
                    active: !layout.plugin_bypass.get(index).copied().unwrap_or(false),
                    has_ui: false,
                })
                .collect::<Vec<_>>();
            let auto_lanes = slint::ModelRc::new(slint::VecModel::from(vec![
                Z_AutomationLane {
                    name: "Volume".into(),
                    color: slint::Color::from_rgb_u8(255, 143, 0),
                    active: !volume_points.is_empty(),
                    points: slint::ModelRc::new(slint::VecModel::from(volume_points)),
                },
                Z_AutomationLane {
                    name: "Pan".into(),
                    color: slint::Color::from_rgb_u8(160, 120, 255),
                    active: !pan_points.is_empty(),
                    points: slint::ModelRc::new(slint::VecModel::from(pan_points)),
                },
                Z_AutomationLane {
                    name: "Track Delay".into(),
                    color: slint::Color::from_rgb_u8(70, 180, 255),
                    active: !delay_points.is_empty(),
                    points: slint::ModelRc::new(slint::VecModel::from(delay_points)),
                },
            ]));
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
                input: "IN 1/2".into(),
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
                saturate_active: false,
                artic_map: "".into(),
                correlation: 0.0,
                eq_low_band: 0.0,
                eq_low_cut: 0.0,
                eq_high_band: 0.0,
                eq_high_cut: 0.0,
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
            let notes = if !is_bus && *type_id != 0 {
                let phrase = (0..8)
                    .map(|step| ZNote {
                        pitch: 48 + ((step * 7 + index * 3) % 12) as i32,
                        start_beat: step as f32 * 2.0,
                        length_beats: 1.5,
                        velocity: 76 + (step % 4) as i32 * 8,
                        articulation: 0,
                        vibrato_amount: 0.0,
                        vibrato_rate_millihz: 5000,
                        selected: false,
                        lyric: "".into(),
                    })
                    .collect::<Vec<_>>();
                slint::ModelRc::new(slint::VecModel::from(phrase))
            } else {
                slint::ModelRc::default()
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
                output: "Main Out".into(),
                monitor: false,
                piano_roll_notes: notes,
                is_folder: is_bus,
                parent_id: 0,
                folded: false,
                panner_mode: 0,
                pan3d_x: 0.0,
                pan3d_y: 0.0,
                pan3d_z: 0.0,
                saturate_active: false,
                artic_map: "".into(),
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
