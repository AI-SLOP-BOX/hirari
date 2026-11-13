//! Lane imports for the DAWproject import: automation passes and media/clip decode,
//! split out of dawproject_import.rs.
use super::*;

pub(in crate::dawproject) struct MediaImportOutcome {
    pub regions: Vec<ProjectRegion>,
    pub warp_markers: Vec<WarpMarkerContract>,
    pub midi_notes: Vec<MidiNoteContract>,
    pub embedded_media_count: usize,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::dawproject) fn import_content(
    archive: &mut ZipArchive<File>,
    arrangement: roxmltree::Node<'_, '_>,
    external_tracks: &[roxmltree::Node<'_, '_>],
    track_ids: &HashMap<String, u32>,
    track_content: &[(String, String)],
    tracks: &mut Vec<ProjectTrack>,
    plugin_parameter_refs_by_track: &HashMap<String, HashMap<String, (u32, u32)>>,
    tempo_events: &[TempoEventContract],
    base_bpm: f64,
    sample_rate: f64,
    asset_dir: &Path,
    asset_dir_name: &str,
    destination_parent: &Path,
) -> Result<MediaImportOutcome> {
    let mut regions = Vec::new();
    let mut warp_markers = Vec::new();
    let mut midi_notes = Vec::new();
let mut media_path_by_source = HashMap::<String, String>::new();
    let mut total_media_bytes = 0u64;
    let mut embedded_media_count = 0usize;
    let mut automation_point_count = 0usize;
    for (external_id, _) in track_content.iter() {
        let track_id = track_ids[external_id];
        let track_index = usize::try_from(track_id - 1).context("imported track index overflow")?;
        let external_track = external_tracks
            .iter()
            .find(|track| track.attribute("id") == Some(external_id.as_str()))
            .context("DAWproject track lookup failed")?;
        let Some(lane) = arrangement.descendants().find(|node| {
            node.has_tag_name("Lanes") && node.attribute("track") == Some(external_id.as_str())
        }) else {
            continue;
        };
        if let Some(channel) = external_track
            .children()
            .find(|node| node.has_tag_name("Channel"))
        {
            let parameter_id = |name: &str| {
                channel
                    .children()
                    .find(|node| node.has_tag_name(name))
                    .and_then(|node| node.attribute("id"))
                    .map(str::to_owned)
            };
            let volume_id = parameter_id("Volume");
            let pan_id = parameter_id("Pan");
            for points in lane
                .descendants()
                .filter(|node| node.has_tag_name("Points"))
            {
                let target_id = points
                    .children()
                    .find(|node| node.has_tag_name("Target"))
                    .and_then(|node| node.attribute("parameter"));
                let is_volume = target_id.is_some() && target_id == volume_id.as_deref();
                let is_pan = target_id.is_some() && target_id == pan_id.as_deref();
                let plugin_parameter = target_id.and_then(|target_id| {
                    plugin_parameter_refs_by_track
                        .get(external_id)
                        .and_then(|references| references.get(target_id))
                        .copied()
                });
                if !is_volume && !is_pan && plugin_parameter.is_none() {
                    continue;
                }
                let unit = points.attribute("timeUnit").unwrap_or("beats");
                let parameter_unit = if is_pan {
                    channel
                        .children()
                        .find(|node| node.has_tag_name("Pan"))
                        .and_then(|node| node.attribute("unit"))
                        .unwrap_or("normalized")
                } else {
                    channel
                        .children()
                        .find(|node| node.has_tag_name("Volume"))
                        .and_then(|node| node.attribute("unit"))
                        .unwrap_or("linear")
                };
                let mut imported_plugin_points = Vec::new();
                for point in points
                    .children()
                    .filter(|node| node.has_tag_name("RealPoint"))
                {
                    let point_time = required_number(&point, "time")?;
                    let time = timeline_position_to_samples(
                        point_time,
                        unit,
                        sample_rate,
                        &tempo_events,
                        base_bpm,
                    )?;
                    let mut value = required_number(&point, "value")? as f32;
                    if plugin_parameter.is_some() {
                        if points.attribute("unit").unwrap_or("normalized") != "normalized" {
                            let min = points
                                .attribute("min")
                                .and_then(|value| value.parse::<f32>().ok())
                                .unwrap_or(0.0);
                            let max = points
                                .attribute("max")
                                .and_then(|value| value.parse::<f32>().ok())
                                .unwrap_or(1.0);
                            if !min.is_finite() || !max.is_finite() || max <= min {
                                continue;
                            }
                            value = (value - min) / (max - min);
                        }
                        let automation = crate::plugin_parameters::PluginAutomationPoint {
                            sample: time,
                            normalized: f64::from(value.clamp(0.0, 1.0)),
                            curve: 0.0,
                        };
                        automation_point_count = automation_point_count.saturating_add(1);
                        if automation_point_count > crate::project::MAX_PROJECT_REGIONS {
                            bail!("DAWproject contains too many automation points");
                        }
                        imported_plugin_points.push(automation);
                        continue;
                    }
                    if is_pan {
                        if parameter_unit == "normalized" {
                            value = value * 2.0 - 1.0;
                        }
                        value = value.clamp(-1.0, 1.0);
                    } else {
                        if parameter_unit == "decibel" {
                            value = 10.0f32.powf(value / 20.0);
                        }
                        value = value.clamp(0.0, 2.0);
                    }
                    let automation = AutomationPointContract {
                        time: time as f64,
                        value,
                        curve: crate::project_contracts::Curve::Shortcut(0.0),
                    };
                    automation_point_count = automation_point_count.saturating_add(1);
                    if automation_point_count > crate::project::MAX_PROJECT_REGIONS {
                        bail!("DAWproject contains too many track automation points");
                    }
                    if is_volume {
                        tracks[track_index].volume_automation.push(automation);
                    } else {
                        tracks[track_index].pan_automation.push(automation);
                    }
                }
                if let Some((plugin_index, parameter_id)) = plugin_parameter {
                    if !imported_plugin_points.is_empty() {
                        let lane = tracks[track_index]
                            .plugin_automation
                            .iter_mut()
                            .find(|lane| {
                                lane.plugin_index == plugin_index
                                    && lane.parameter_id == parameter_id
                            });
                        if let Some(lane) = lane {
                            lane.points.extend(imported_plugin_points);
                        } else {
                            tracks[track_index].plugin_automation.push(
                                crate::plugin_parameters::PluginAutomationLane {
                                    plugin_index,
                                    parameter_id,
                                    points: imported_plugin_points,
                                },
                            );
                        }
                    }
                }
            }
            tracks[track_index]
                .volume_automation
                .sort_by(|left, right| left.time.total_cmp(&right.time));
            tracks[track_index]
                .volume_automation
                .dedup_by(|left, right| {
                    if (left.time - right.time).abs() <= f64::EPSILON {
                        left.value = right.value;
                        true
                    } else {
                        false
                    }
                });
            tracks[track_index]
                .pan_automation
                .sort_by(|left, right| left.time.total_cmp(&right.time));
            tracks[track_index].pan_automation.dedup_by(|left, right| {
                if (left.time - right.time).abs() <= f64::EPSILON {
                    left.value = right.value;
                    true
                } else {
                    false
                }
            });
            for lane in &mut tracks[track_index].plugin_automation {
                lane.points.sort_by_key(|point| point.sample);
                lane.points.dedup_by(|left, right| {
                    if left.sample == right.sample {
                        left.normalized = right.normalized;
                        true
                    } else {
                        false
                    }
                });
            }
        }
        for clips in lane.descendants().filter(|node| node.has_tag_name("Clips")) {
            let clips_unit = clips.attribute("timeUnit").unwrap_or("beats");
            for clip in clips.children().filter(|node| node.has_tag_name("Clip")) {
                // Read media owned by this clip only. DAWproject may nest a
                // content clip inside an arrangement clip; searching all
                // descendants here imports the same nested audio twice.
                let clip_lanes = clip.children().find(|node| node.has_tag_name("Lanes"));
                let warp_container = clip
                    .children()
                    .find(|node| node.has_tag_name("Warps"))
                    .or_else(|| {
                        clip_lanes.and_then(|lanes| {
                            lanes.children().find(|node| node.has_tag_name("Warps"))
                        })
                    });
                let audio = clip
                    .children()
                    .find(|node| node.has_tag_name("Audio"))
                    .or_else(|| {
                        clip_lanes.and_then(|lanes| {
                            lanes.children().find(|node| node.has_tag_name("Audio"))
                        })
                    })
                    .or_else(|| {
                        warp_container.and_then(|warps| {
                            warps.children().find(|node| node.has_tag_name("Audio"))
                        })
                    });
                if let Some(audio) = audio {
                    let file_node = audio.children().find(|node| node.has_tag_name("File"));
                    if let Some(file_node) = file_node {
                        if regions.len() >= crate::project::MAX_PROJECT_REGIONS {
                            bail!("DAWproject contains too many audio clips");
                        }
                        let archive_path = file_node
                            .attribute("path")
                            .context("audio file path is missing")?;
                        validate_archive_media_path(archive_path)?;
                        let relative_media_path = if let Some(path) =
                            media_path_by_source.get(archive_path)
                        {
                            path.clone()
                        } else {
                            let entry = archive.by_name(archive_path).with_context(|| {
                                format!("embedded audio is missing: {archive_path}")
                            })?;
                            let size = entry.size();
                            if size == 0 || size > MAX_MEDIA_FILE_BYTES {
                                bail!("embedded audio size is outside supported limits: {archive_path}");
                            }
                            total_media_bytes = total_media_bytes
                                .checked_add(size)
                                .context("embedded audio size overflow")?;
                            if total_media_bytes > MAX_TOTAL_MEDIA_BYTES {
                                bail!("embedded media exceeds Hirari's 8 GiB import limit");
                            }
                            if !asset_dir.exists() {
                                fs::create_dir(&asset_dir).with_context(|| {
                                    format!(
                                        "could not create media directory {}",
                                        asset_dir.display()
                                    )
                                })?;
                            }
                            let extension = supported_audio_extension(archive_path)
                                .context("DAWproject media uses an unsupported audio extension")?;
                            let file_name =
                                format!("media-{:05}.{extension}", embedded_media_count + 1);
                            let destination_media = asset_dir.join(&file_name);
                            let mut output = OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&destination_media)?;
                            let copied = std::io::copy(&mut entry.take(size + 1), &mut output)?;
                            output.sync_all()?;
                            if copied != size {
                                bail!("embedded audio size did not match its archive entry");
                            }
                            let (_, _, frames) = read_audio_info(&destination_media)?;
                            if frames == 0 {
                                bail!("embedded audio contains no frames");
                            }
                            embedded_media_count += 1;
                            let relative_path = format!("{asset_dir_name}/{file_name}");
                            media_path_by_source
                                .insert(archive_path.to_owned(), relative_path.clone());
                            relative_path
                        };
                        let media_file = destination_parent.join(&relative_media_path);
                        let (channels, source_rate, source_frames) = read_audio_info(&media_file)?;
                        if channels == 0 || source_rate == 0 || source_frames == 0 {
                            bail!("embedded audio metadata is invalid: {archive_path}");
                        }
                        let start_value = required_number(&clip, "time")?;
                        let duration = required_number(&clip, "duration")?;
                        if duration <= 0.0 {
                            bail!("DAWproject audio clip duration must be positive");
                        }
                        let start_sample = timeline_position_to_samples(
                            start_value,
                            clips_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let end_sample = timeline_position_to_samples(
                            start_value + duration,
                            clips_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let fade_unit = clip.attribute("fadeTimeUnit").unwrap_or("seconds");
                        let fade_in = optional_number(&clip, "fadeInTime")
                            .unwrap_or(0.0)
                            .clamp(0.0, duration);
                        let fade_out = optional_number(&clip, "fadeOutTime")
                            .unwrap_or(0.0)
                            .clamp(0.0, duration);
                        let fade_in_samples = duration_to_samples(
                            start_value,
                            fade_in,
                            fade_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let fade_out_samples = duration_to_samples(
                            start_value + duration - fade_out,
                            fade_out,
                            fade_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let source_offset_seconds =
                            optional_number(&clip, "playStart").unwrap_or(0.0).max(0.0);
                        let source_offset =
                            (source_offset_seconds * f64::from(source_rate)).round() as u64;
                        let timeline_length = end_sample.saturating_sub(start_sample).max(1);
                        let timeline_source_length = (timeline_length as f64
                            * f64::from(source_rate)
                            / f64::from(sample_rate))
                        .round();
                        if !timeline_source_length.is_finite()
                            || timeline_source_length < 1.0
                            || timeline_source_length > u64::MAX as f64
                        {
                            bail!("DAWproject audio clip source duration is out of range");
                        }
                        let id =
                            u32::try_from(regions.len() + 1).context("region id range exceeded")?;
                        let warp_time_unit = warp_container
                            .and_then(|node| node.attribute("timeUnit"))
                            .unwrap_or(clips_unit);
                        let warp_content_time_unit = warp_container
                            .and_then(|node| node.attribute("contentTimeUnit"))
                            .or_else(|| clip.attribute("contentTimeUnit"))
                            .unwrap_or("seconds");
                        let warp_nodes = if let Some(container) = warp_container {
                            container
                                .children()
                                .filter(|node| node.has_tag_name("Warp"))
                                .collect::<Vec<_>>()
                        } else {
                            clip.children()
                                .filter(|node| node.has_tag_name("Warp"))
                                .collect::<Vec<_>>()
                        };
                        // A DAWproject warp map is a non-linear time mapping.
                        // Keep the source pitch stable when the Hirari renderer
                        // applies those anchors; varispeed would change pitch
                        // whenever the source/timeline slope differs from 1.
                        let pitch_preserve_warp = warp_nodes.len() >= 2;
                        // The clip duration lives on the arrangement timeline;
                        // it is not a reliable source span after time-warping.
                        // Prefer explicit playStop, then the last warp content
                        // anchor, and use the old rate-derived span only for
                        // clips that carry neither source bound.
                        let source_stop_seconds = if let Some(play_stop) =
                            optional_number(&clip, "playStop")
                        {
                            play_stop
                        } else if !warp_nodes.is_empty() {
                            if warp_content_time_unit != "seconds" {
                                bail!("unsupported DAWproject warp content unit: {warp_content_time_unit}");
                            }
                            let mut last_content_time = source_offset_seconds;
                            for warp in &warp_nodes {
                                last_content_time =
                                    last_content_time.max(required_number(warp, "contentTime")?);
                            }
                            if last_content_time > source_offset_seconds {
                                last_content_time
                            } else {
                                source_offset_seconds
                                    + timeline_source_length / f64::from(source_rate)
                            }
                        } else {
                            source_offset_seconds + timeline_source_length / f64::from(source_rate)
                        };
                        if !source_stop_seconds.is_finite()
                            || source_stop_seconds <= source_offset_seconds
                        {
                            bail!("DAWproject audio clip source range is invalid");
                        }
                        let source_length = ((source_stop_seconds - source_offset_seconds)
                            * f64::from(source_rate))
                        .round();
                        if !source_length.is_finite()
                            || source_length < 1.0
                            || source_length > u64::MAX as f64
                        {
                            bail!("DAWproject audio clip source duration is out of range");
                        }
                        let source_length = source_length as u64;
                        let warp_base = match warp_time_unit {
                            unit if unit == clips_unit => start_value,
                            "seconds" => start_sample as f64 / sample_rate,
                            "beats" => samples_to_beats(
                                start_sample,
                                sample_rate,
                                &tempo_events,
                                base_bpm,
                            )?,
                            other => bail!("unsupported DAWproject warp timeline unit: {other}"),
                        };
                        for (warp_index, warp) in warp_nodes.into_iter().enumerate() {
                            if warp_markers.len() >= crate::project::MAX_PROJECT_WARP_MARKERS {
                                bail!("DAWproject contains too many audio warp markers");
                            }
                            let warp_time = required_number(&warp, "time")?;
                            let content_time = required_number(&warp, "contentTime")?;
                            if warp_time < 0.0 || content_time < 0.0 {
                                bail!("DAWproject warp marker is outside the audio clip");
                            }
                            let marker_timeline = timeline_position_to_samples(
                                warp_base + warp_time,
                                warp_time_unit,
                                sample_rate,
                                &tempo_events,
                                base_bpm,
                            )?;
                            let marker_source = match warp_content_time_unit {
                                "seconds" => content_time * f64::from(source_rate),
                                other => bail!("unsupported DAWproject warp content unit: {other}"),
                            }
                            .round();
                            if !marker_source.is_finite()
                                || marker_source < 0.0
                                || marker_source > u64::MAX as f64
                            {
                                bail!("DAWproject warp content position is out of range");
                            }
                            warp_markers.push(WarpMarkerContract {
                                marker_id: format!("dawproject:{id}:warp:{warp_index}"),
                                region_id: id,
                                source_sample: marker_source as u64,
                                timeline_sample: marker_timeline,
                                algorithm: "linear-anchor-map".into(),
                                pitch_semitones: 0.0,
                                transient: false,
                                analysis_generation: 0,
                                cache_generation: 0,
                            });
                        }
                        regions.push(ProjectRegion {
                            id,
                            track_id,
                            name: clip.attribute("name").unwrap_or("Audio Clip").to_owned(),
                            path: relative_media_path,
                            start: start_sample,
                            length: timeline_length,
                            source_length,
                            source_sample_rate: source_rate,
                            source_offset,
                            base_source_offset: source_offset,
                            base_length: source_frames,
                            muted: clip
                                .attribute("enable")
                                .and_then(parse_bool)
                                .is_some_and(|enabled| !enabled),
                            clip_gain: parse_clip_expression_value(clip, "gain", "linear")?
                                .map(|value| value.clamp(0.0, 2.0))
                                .unwrap_or(1.0),
                            fade_in_samples,
                            fade_out_samples,
                            warp_ratio: 1.0,
                            pitch_preserve_warp,
                            pitch_semitones: parse_clip_expression_value(
                                clip,
                                "transpose",
                                "semitones",
                            )?
                            .map(|value| value.clamp(-24.0, 24.0))
                            .unwrap_or(0.0),
                            reverse: false,
                            loop_count: 1,
                            sync_group: 0,
                            // DAWproject import has no processing-step concept;
                            // an empty history keeps the field honest.
                            processing_history: Vec::new(),
                        });
                    }
                }
                if let Some(notes) = clip.descendants().find(|node| node.has_tag_name("Notes")) {
                    let note_unit = notes.attribute("timeUnit").unwrap_or(clips_unit);
                    let clip_start = required_number(&clip, "time")?;
                    for note in notes.children().filter(|node| node.has_tag_name("Note")) {
                        if midi_notes.len() >= crate::project::MAX_PROJECT_REGIONS {
                            bail!("DAWproject contains too many MIDI notes");
                        }
                        let note_start = required_number(&note, "time")?;
                        let note_duration = required_number(&note, "duration")?;
                        if note_duration <= 0.0 {
                            bail!("DAWproject MIDI note duration must be positive");
                        }
                        let clip_start_sample = timeline_position_to_samples(
                            clip_start,
                            clips_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let clip_start_in_note_unit = match note_unit {
                            "beats" => samples_to_beats(
                                clip_start_sample,
                                sample_rate,
                                &tempo_events,
                                base_bpm,
                            )?,
                            "seconds" => clip_start_sample as f64 / sample_rate,
                            other => bail!("unsupported DAWproject MIDI note time unit: {other}"),
                        };
                        let note_start_sample = timeline_position_to_samples(
                            clip_start_in_note_unit + note_start,
                            note_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let note_end_sample = timeline_position_to_samples(
                            clip_start_in_note_unit + note_start + note_duration,
                            note_unit,
                            sample_rate,
                            &tempo_events,
                            base_bpm,
                        )?;
                        let start_sample = note_start_sample;
                        let end_sample = note_end_sample;
                        let velocity = optional_number(&note, "vel").unwrap_or(1.0).clamp(0.0, 1.0);
                        let channel = note
                            .attribute("channel")
                            .and_then(|value| value.parse::<u8>().ok())
                            .unwrap_or(0)
                            .min(15);
                        let pitch = note
                            .attribute("key")
                            .and_then(|value| value.parse::<u8>().ok())
                            .context("DAWproject MIDI note key is invalid")?;
                        let id_velocity = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                        midi_notes.push(MidiNoteContract {
                            region_id: 0,
                            track_id,
                            pitch,
                            midi_channel: channel,
                            articulation: 0,
                            velocity: id_velocity,
                            start_sample,
                            length_samples: end_sample.saturating_sub(start_sample).max(1),
                            lyric: String::new(),
                            phoneme: String::new(),
                            pitch_curve_cents: Vec::new(),
                            vibrato_depth_cents: 0,
                            vibrato_rate_millihz: 5_000,
                            portamento_samples: 0,
                            probability: 100,
                            repeat_count: 1,
                        });
                    }
                }
            }
        }
    }

    Ok(MediaImportOutcome { regions, warp_markers, midi_notes, embedded_media_count })
}
