//! DAWproject 1.0 export for exchanging a Hirari arrangement with other DAWs.
//! Audio is embedded; tracks, audio clips, MIDI notes, tempo, meter, and
//! arrangement markers are represented in the portable project XML.

use crate::persistence::ProjectMetadata;
use crate::project::{ProjectDocument, ProjectRegion, ProjectTrack};
use crate::project_contracts::{
    AutomationPointContract, MarkerContract, MidiNoteContract, TempoEventContract,
    TimeSignatureEventContract, WarpMarkerContract,
};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MAX_MEDIA_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL_MEDIA_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DawProjectExportReport {
    pub track_count: usize,
    pub audio_clip_count: usize,
    pub midi_note_count: usize,
    pub embedded_media_count: usize,
    pub unsupported_plugin_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DawProjectImportReport {
    pub track_count: usize,
    pub audio_clip_count: usize,
    pub midi_note_count: usize,
    pub embedded_media_count: usize,
    pub unsupported_plugin_count: usize,
}

struct ImportAssetCleanup {
    path: PathBuf,
    keep: bool,
}

impl Drop for ImportAssetCleanup {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Import DAWproject 1.0 arrangement data and embedded audio media into a native
/// Hirari project. The generated media directory is unique and stored beside
/// the destination project so the resulting document remains relocatable as a
/// pair. Hirari built-in devices and their normalized parameters are restored;
/// unsupported third-party devices are skipped and counted in the report.
pub fn import_dawproject_file(
    source: &Path,
    destination: &Path,
    sample_rate: f64,
) -> Result<DawProjectImportReport> {
    if source.as_os_str().is_empty() || destination.as_os_str().is_empty() {
        bail!("DAWproject source and destination must not be empty");
    }
    if !sample_rate.is_finite()
        || sample_rate.fract() != 0.0
        || !(8_000.0..=384_000.0).contains(&sample_rate)
    {
        bail!("target project sample rate is invalid");
    }
    let destination_parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(destination_parent).with_context(|| {
        format!(
            "could not create project directory {}",
            destination_parent.display()
        )
    })?;
    let import_id = uuid::Uuid::new_v4().simple().to_string();
    let asset_dir_name = format!(
        "{}-media-{import_id}",
        destination
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
    );
    let asset_dir = destination_parent.join(&asset_dir_name);
    let mut asset_cleanup = ImportAssetCleanup {
        path: asset_dir.clone(),
        keep: false,
    };
    let input = File::open(source)
        .with_context(|| format!("could not open DAWproject {}", source.display()))?;
    let mut archive = ZipArchive::new(input).context("DAWproject is not a valid ZIP archive")?;
    if archive.len() > 65_536 {
        bail!("DAWproject contains too many archive entries");
    }
    let mut archive_names = std::collections::HashSet::with_capacity(archive.len());
    for name in archive.file_names() {
        if !archive_names.insert(name.to_owned()) {
            bail!("DAWproject contains a duplicate archive entry: {name}");
        }
    }
    let project_xml = read_zip_text(&mut archive, "project.xml", 64 * 1024 * 1024)?;
    let metadata_xml = read_zip_text_optional(&mut archive, "metadata.xml", 4 * 1024 * 1024)?;
    let title = metadata_xml
        .as_deref()
        .and_then(|xml| {
            let document = roxmltree::Document::parse(xml).ok()?;
            document
                .descendants()
                .find(|node| node.has_tag_name("Title"))
                .and_then(|node| node.text())
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            source
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Imported DAWproject")
                .to_owned()
        });
    let xml =
        roxmltree::Document::parse(&project_xml).context("DAWproject project.xml is malformed")?;
    let root = xml.root_element();
    if !root.has_tag_name("Project") || root.attribute("version") != Some("1.0") {
        bail!("only DAWproject version 1.0 is supported");
    }
    let transport = root.children().find(|node| node.has_tag_name("Transport"));
    let base_bpm = transport
        .and_then(|node| node.children().find(|child| child.has_tag_name("Tempo")))
        .and_then(|node| node.attribute("value"))
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(120.0);
    if !(20.0..=300.0).contains(&base_bpm) {
        bail!("DAWproject tempo is outside Hirari's supported range (20–300 BPM)");
    }
    let (base_numerator, base_denominator) = transport
        .and_then(|node| {
            node.children()
                .find(|child| child.has_tag_name("TimeSignature"))
        })
        .map(|node| {
            let numerator = node
                .attribute("numerator")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4);
            let denominator = node
                .attribute("denominator")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4);
            (numerator, denominator)
        })
        .unwrap_or((4, 4));
    let mut tempo_events = parse_tempo_events(root, base_bpm)?;
    let mut time_signature_events =
        parse_time_signature_events(root, base_numerator, base_denominator)?;
    let arrangement = root
        .descendants()
        .find(|node| node.has_tag_name("Arrangement"))
        .context("DAWproject arrangement is missing")?;
    let structure = root
        .children()
        .find(|node| node.has_tag_name("Structure"))
        .context("DAWproject track structure is missing")?;
    let external_tracks = structure
        .descendants()
        .filter(|node| node.has_tag_name("Track"))
        .collect::<Vec<_>>();
    if external_tracks.len() > crate::project::MAX_PROJECT_TRACKS {
        bail!("DAWproject contains too many tracks");
    }
    let mut track_ids = HashMap::<String, u32>::with_capacity(external_tracks.len());
    let mut tracks = Vec::with_capacity(external_tracks.len());
    let mut track_content = Vec::with_capacity(external_tracks.len());
    let mut plugin_parameter_refs_by_track =
        HashMap::<String, HashMap<String, (u32, u32)>>::with_capacity(external_tracks.len());
    let mut supported_plugin_count = 0usize;
    for (index, external_track) in external_tracks.iter().enumerate() {
        let external_id = external_track
            .attribute("id")
            .context("DAWproject track id is missing")?;
        if track_ids.contains_key(external_id) {
            bail!("DAWproject contains duplicate track id {external_id}");
        }
        let id = u32::try_from(index + 1).context("track id range exceeded")?;
        track_ids.insert(external_id.to_owned(), id);
        let channel = external_track
            .children()
            .find(|node| node.has_tag_name("Channel"));
        let parameter_value = |name: &str, default: f32| {
            channel
                .and_then(|node| node.children().find(|child| child.has_tag_name(name)))
                .and_then(|node| node.attribute("value"))
                .and_then(|value| value.parse::<f32>().ok())
                .filter(|value| value.is_finite())
                .unwrap_or(default)
        };
        let muted = channel
            .and_then(|node| node.children().find(|child| child.has_tag_name("Mute")))
            .and_then(|node| node.attribute("value"))
            .and_then(parse_bool)
            .unwrap_or(false);
        let normalized_pan = parameter_value("Pan", 0.5).clamp(0.0, 1.0);
        let content_type = external_track.attribute("contentType").unwrap_or("audio");
        let track_type = if content_type.split_whitespace().any(|kind| kind == "notes") {
            "Instrument"
        } else {
            "Audio"
        };
        let name = external_track
            .attribute("name")
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Track {}", index + 1));
        let mut plugin_types = Vec::new();
        let mut plugin_bypasses = Vec::new();
        let mut plugin_parameter_values = Vec::<Vec<f32>>::new();
        let mut plugin_parameter_refs = HashMap::<String, (u32, u32)>::new();
        if let Some(devices) =
            channel.and_then(|node| node.children().find(|child| child.has_tag_name("Devices")))
        {
            for device in devices
                .children()
                .filter(|node| matches!(node.tag_name().name(), "Device" | "BuiltinDevice"))
            {
                // Only restore Hirari's built-in IDs. A third-party device ID
                // is not a portable executable plugin identity, so treating it
                // as active could make the imported project fail to load.
                let plugin_type = device
                    .attribute("deviceID")
                    .or_else(|| device.attribute("id"))
                    .and_then(|id| id.strip_prefix("builtin:"))
                    .and_then(|value| value.parse::<u32>().ok())
                    .filter(|plugin_type| *plugin_type <= 10);
                let Some(plugin_type) = plugin_type else {
                    continue;
                };
                supported_plugin_count = supported_plugin_count.saturating_add(1);
                let slot_index = u32::try_from(plugin_types.len())
                    .context("DAWproject plugin slot range exceeded")?;
                plugin_types.push(plugin_type);
                plugin_bypasses.push(
                    device
                        .children()
                        .find(|child| child.has_tag_name("Enabled"))
                        .and_then(|enabled| enabled.attribute("value"))
                        .and_then(parse_bool)
                        .is_some_and(|enabled| !enabled),
                );
                let mut parameter_values = Vec::new();
                if let Some(parameters) = device
                    .children()
                    .find(|child| child.has_tag_name("Parameters"))
                {
                    for parameter in parameters.children().filter(|node| {
                        matches!(node.tag_name().name(), "RealParameter" | "BooleanParameter")
                    }) {
                        let Some(parameter_id) = parameter
                            .attribute("parameterID")
                            .and_then(|id| id.parse::<u32>().ok())
                            .filter(|id| *id < crate::project::MAX_PLUGIN_PARAMETERS as u32)
                        else {
                            continue;
                        };
                        let value = optional_number(&parameter, "value")
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0) as f32;
                        let required = parameter_id as usize + 1;
                        if parameter_values.len() < required {
                            parameter_values.resize(required, 0.0);
                        }
                        parameter_values[parameter_id as usize] = value;
                        if let Some(reference) = parameter.attribute("id") {
                            plugin_parameter_refs
                                .insert(reference.to_owned(), (slot_index, parameter_id));
                        }
                    }
                }
                plugin_parameter_values.push(parameter_values);
            }
        }
        plugin_parameter_refs_by_track.insert(external_id.to_owned(), plugin_parameter_refs);
        tracks.push(ProjectTrack {
            id,
            name,
            track_type: track_type.to_owned(),
            volume: parameter_value("Volume", 1.0).clamp(0.0, 2.0),
            pan: normalized_pan * 2.0 - 1.0,
            muted,
            solo: channel
                .and_then(|node| node.attribute("solo"))
                .and_then(parse_bool)
                .unwrap_or(false),
            record_armed: false,
            phase_invert: false,
            track_delay_samples: 0,
            volume_automation: Vec::new(),
            pan_automation: Vec::new(),
            track_delay_automation: Vec::new(),
            plugin_automation: Vec::new(),
            plugin_types: plugin_types.clone(),
            plugin_bypasses,
            plugin_parameter_values,
            plugin_states: vec![Vec::new(); plugin_types.len()],
            plugin_gui_states: vec![Vec::new(); plugin_types.len()],
            plugin_state_versions: vec![
                crate::project::PLUGIN_STATE_SCHEMA_VERSION;
                plugin_types.len()
            ],
            sandbox_plugin_paths: Vec::new(),
            sandbox_plugin_states: Vec::new(),
            sandbox_plugin_state_versions: Vec::new(),
            expression_map: Vec::new(),
            expression_map_pro: None,
        });
        track_content.push((external_id.to_owned(), track_type.to_owned()));
    }
    if tracks.is_empty() {
        bail!("DAWproject contains no tracks");
    }
    let unsupported_plugin_count = structure
        .descendants()
        .filter(|node| is_dawproject_plugin_device(*node))
        .count()
        .saturating_sub(supported_plugin_count);
    tempo_events = normalize_tempo_events(tempo_events, base_bpm)?;
    time_signature_events =
        normalize_time_signature_events(time_signature_events, base_numerator, base_denominator)?;
    let mut regions = Vec::new();
    let mut warp_markers = Vec::new();
    let mut midi_notes = Vec::new();
    let mut markers = Vec::new();
    let mut media_path_by_source = HashMap::<String, String>::new();
    let mut total_media_bytes = 0u64;
    let mut embedded_media_count = 0usize;
    let mut automation_point_count = 0usize;
    for (external_id, _) in &track_content {
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
                        curve: 0.0,
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
    for marker in arrangement
        .descendants()
        .filter(|node| node.has_tag_name("Marker"))
    {
        if markers.len() >= 65_536 {
            bail!("DAWproject contains too many arrangement markers");
        }
        let position = optional_number(&marker, "time").unwrap_or(0.0);
        let marker_unit = marker
            .parent()
            .and_then(|parent| parent.attribute("timeUnit"))
            .unwrap_or("beats");
        let beat = match marker_unit {
            "beats" => position,
            "seconds" if position >= 0.0 => {
                let sample = (position * sample_rate).round();
                if !sample.is_finite() || sample > u64::MAX as f64 {
                    bail!("DAWproject marker position exceeds the timeline range");
                }
                samples_to_beats(sample as u64, sample_rate, &tempo_events, base_bpm)?
            }
            other => bail!("unsupported DAWproject marker time unit: {other}"),
        };
        let label = marker.attribute("name").unwrap_or("Marker").trim();
        if !label.is_empty() {
            let label = bounded_utf8(label, 128);
            markers.push(MarkerContract {
                id: u32::try_from(markers.len() + 1).context("marker id range exceeded")?,
                label,
                beat: beat.max(0.0),
                color: String::new(),
            });
        }
    }
    warp_markers.sort_by_key(|marker| (marker.region_id, marker.timeline_sample));
    let document = ProjectDocument {
        schema_version: crate::project::PROJECT_SCHEMA_VERSION,
        contract_version: crate::project_contracts::PROJECT_CONTRACT_VERSION,
        project_id: uuid::Uuid::new_v4().to_string(),
        metadata: ProjectMetadata {
            name: title,
            version: crate::project::PROJECT_SCHEMA_VERSION,
            bpm: base_bpm as f32,
            tracks_count: tracks.len() as u32,
            key_root: 0,
            scale_type: 0,
        },
        sample_rate,
        master_gain: 1.0,
        cycle_start_sample: 0,
        cycle_end_sample: 0,
        cycle_enabled: false,
        metronome_enabled: false,
        tracks,
        aux_track_ids: Vec::new(),
        regions,
        plugin_instances: Vec::new(),
        midi_learn_mappings: Vec::new(),
        midi_notes,
        chord_track: Vec::new(),
        midi_events: Vec::new(),
        tempo_events,
        time_signature_events,
        macro_mappings: Vec::new(),
        warp_markers,
        render_targets: Vec::new(),
        freeze_artifacts: Vec::new(),
        sidechain_routes: Vec::new(),
        feedback_routes: Vec::new(),
        audio_routes: Vec::new(),
        audio_input_assignments: Vec::new(),
        step_sequencer_patterns: Vec::new(),
        openutau_vocals: Vec::new(),
        comp_takes: Vec::new(),
        comp_segments: Vec::new(),
        track_stacks: Vec::new(),
        markers,
        vca_groups: Vec::new(),
        hardware_inserts: Vec::new(),
        control_room: None,
    };
    document
        .validate()
        .context("imported DAWproject cannot be represented as a Hirari project")?;
    document
        .save_atomic(&destination.to_string_lossy())
        .context("could not save imported Hirari project")?;
    asset_cleanup.keep = true;
    Ok(DawProjectImportReport {
        track_count: document.tracks.len(),
        audio_clip_count: document.regions.len(),
        midi_note_count: document.midi_notes.len(),
        embedded_media_count,
        unsupported_plugin_count,
    })
}

struct EmbeddedMedia {
    source: PathBuf,
    archive_path: String,
}

/// Export a native project document to a DAWproject 1.0 archive.
///
/// Supported audio media is embedded in the archive. Relative source paths resolve from
/// the directory containing `source_project`. Destination replacement is
/// atomic after the complete ZIP archive has been flushed and synced.
pub fn export_dawproject_file(
    document: &ProjectDocument,
    source_project: &Path,
    destination: &Path,
) -> Result<DawProjectExportReport> {
    document.validate().context("project cannot be exported")?;
    if destination.as_os_str().is_empty() {
        bail!("DAWproject destination must not be empty");
    }
    let source_root = source_project.parent().unwrap_or_else(|| Path::new("."));
    let (project_xml, media) = build_project_xml(document, source_root)?;
    let metadata_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MetaData><Title>{}</Title></MetaData>",
        escape_xml(&document.metadata.name)
    );
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("could not create export directory {}", parent.display()))?;
    let temporary = temporary_path(destination);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("could not create temporary export {}", temporary.display()))?;
    if let Err(error) = write_archive(&mut file, &project_xml, &metadata_xml, &media) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    file.flush().context("could not flush DAWproject archive")?;
    file.sync_all()
        .context("could not sync DAWproject archive")?;
    drop(file);
    if destination.exists() {
        let backup = temporary_path(destination).with_extension("dawproject.previous");
        fs::rename(destination, &backup)
            .with_context(|| format!("could not preserve {}", destination.display()))?;
        if let Err(error) = fs::rename(&temporary, destination) {
            let _ = fs::rename(&backup, destination);
            let _ = fs::remove_file(&temporary);
            return Err(error).context("could not publish DAWproject archive");
        }
        let _ = fs::remove_file(backup);
    } else {
        fs::rename(&temporary, destination)
            .with_context(|| format!("could not publish {}", destination.display()))?;
    }
    Ok(DawProjectExportReport {
        track_count: document.tracks.len(),
        audio_clip_count: document.regions.len(),
        midi_note_count: document.midi_notes.len(),
        embedded_media_count: media.len(),
        unsupported_plugin_count: document.plugin_instances.len(),
    })
}

fn build_project_xml(
    document: &ProjectDocument,
    source_root: &Path,
) -> Result<(String, HashMap<String, EmbeddedMedia>)> {
    let mut media = HashMap::new();
    let mut total_media_bytes = 0u64;
    let mut structure = String::new();
    let mut lanes = String::new();
    for track in &document.tracks {
        let track_id = format!("track-{}", track.id);
        let channel_id = format!("channel-{}", track.id);
        let regions = document
            .regions
            .iter()
            .filter(|region| region.track_id == track.id)
            .collect::<Vec<_>>();
        let notes = document
            .midi_notes
            .iter()
            .filter(|note| note.track_id == track.id)
            .collect::<Vec<_>>();
        let content_type = match (
            !regions.is_empty(),
            !notes.is_empty(),
            track.track_type.as_str(),
        ) {
            (true, true, _) => "audio notes",
            (true, false, _) => "audio",
            (false, true, _) => "notes",
            (false, false, "Instrument" | "Midi" | "MIDI") => "notes",
            _ => "audio",
        };
        let (device_xml, plugin_parameter_refs) = plugin_devices_xml(track, document);
        structure.push_str(&format!(
            "<Track id=\"{track_id}\" name=\"{}\" contentType=\"{content_type}\" loaded=\"true\"><Channel id=\"{channel_id}\" role=\"regular\" audioChannels=\"2\" solo=\"{}\"><Devices>{device_xml}</Devices><Mute id=\"mute-{}\" name=\"Mute\" value=\"{}\"/><Pan id=\"pan-{}\" name=\"Pan\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"{:.6}\"/><Volume id=\"volume-{}\" name=\"Volume\" unit=\"linear\" min=\"0\" max=\"2\" value=\"{:.6}\"/></Channel></Track>",
            escape_xml(&track.name),
            track.solo,
            track.id,
            track.muted,
            track.id,
            normalized_pan(track.pan),
            track.id,
            track.volume
        ));
        lanes.push_str(&format!(
            "<Lanes id=\"lanes-{}\" track=\"{track_id}\" timeUnit=\"beats\">",
            track.id
        ));
        for region in regions {
            let source = resolve_region_path(source_root, region)?;
            let size = fs::metadata(&source)
                .with_context(|| format!("audio is unavailable: {}", source.display()))?
                .len();
            if size > MAX_MEDIA_FILE_BYTES {
                bail!("audio file exceeds the DAWproject 2 GiB per-file limit");
            }
            total_media_bytes = total_media_bytes.saturating_add(size);
            if total_media_bytes > MAX_TOTAL_MEDIA_BYTES {
                bail!("embedded audio exceeds the DAWproject 8 GiB total limit");
            }
            let extension = supported_audio_extension(source.to_string_lossy().as_ref()).context(
                "DAWproject export supports WAV, AIFF, FLAC, MP3, Ogg, AAC, and M4A media",
            )?;
            let archive_path = format!(
                "audio/track-{}_region-{}.{}",
                track.id, region.id, extension
            );
            let (channels, sample_rate, source_frames) = read_audio_info(&source)?;
            media.insert(
                archive_path.clone(),
                EmbeddedMedia {
                    source,
                    archive_path: archive_path.clone(),
                },
            );
            let clip_start = samples_to_beats(
                region.start,
                document.sample_rate,
                &document.tempo_events,
                f64::from(document.metadata.bpm),
            )?;
            let clip_end = samples_to_beats(
                region.start.saturating_add(region.length),
                document.sample_rate,
                &document.tempo_events,
                f64::from(document.metadata.bpm),
            )?;
            let clip_duration = (clip_end - clip_start).max(0.000_001);
            let source_offset = region.source_offset as f64 / f64::from(sample_rate);
            let source_duration = source_frames as f64 / f64::from(sample_rate);
            let fade_in = region.fade_in_samples as f64 / document.sample_rate;
            let fade_out = region.fade_out_samples as f64 / document.sample_rate;
            let mut region_warps = document
                .warp_markers
                .iter()
                .filter(|marker| marker.region_id == region.id)
                .collect::<Vec<_>>();
            region_warps.sort_by_key(|marker| marker.timeline_sample);
            let mut warp_xml = String::new();
            for marker in region_warps {
                let time = samples_to_beats(
                    marker.timeline_sample,
                    document.sample_rate,
                    &document.tempo_events,
                    f64::from(document.metadata.bpm),
                )? - clip_start;
                let content_time = marker.source_sample as f64 / f64::from(sample_rate);
                warp_xml.push_str(&format!(
                    "<Warp time=\"{time:.9}\" contentTime=\"{content_time:.9}\"/>"
                ));
            }
            let audio_warps = format!(
                "<Warps id=\"warps-{}-{}\" timeUnit=\"beats\" contentTimeUnit=\"seconds\"><Audio id=\"audio-{}-{}\" timeUnit=\"seconds\" duration=\"{source_duration:.9}\" channels=\"{channels}\" sampleRate=\"{sample_rate}\"><File path=\"{archive_path}\"/></Audio>{warp_xml}</Warps>",
                track.id,
                region.id,
                track.id, region.id
            );
            let mut clip_lanes = String::new();
            if (region.clip_gain - 1.0).abs() > f32::EPSILON {
                clip_lanes.push_str(&format!(
                    "<Points id=\"clip-gain-{}-{}\" timeUnit=\"beats\" unit=\"linear\"><Target expression=\"gain\"/><RealPoint time=\"0\" value=\"{:.9}\" interpolation=\"hold\"/></Points>",
                    track.id, region.id, region.clip_gain
                ));
            }
            if region.pitch_semitones.abs() > f32::EPSILON {
                clip_lanes.push_str(&format!(
                    "<Points id=\"clip-transpose-{}-{}\" timeUnit=\"beats\" unit=\"semitones\"><Target expression=\"transpose\"/><RealPoint time=\"0\" value=\"{:.9}\" interpolation=\"hold\"/></Points>",
                    track.id, region.id, region.pitch_semitones
                ));
            }
            lanes.push_str(&format!(
                "<Clips id=\"clips-{}-{}\" timeUnit=\"beats\"><Clip name=\"{}\" time=\"{clip_start:.9}\" duration=\"{clip_duration:.9}\" contentTimeUnit=\"seconds\" playStart=\"{source_offset:.9}\" enable=\"{}\" fadeTimeUnit=\"seconds\" fadeInTime=\"{fade_in:.9}\" fadeOutTime=\"{fade_out:.9}\">{audio_warps}<Lanes id=\"clip-lanes-{}-{}\" timeUnit=\"beats\">{clip_lanes}</Lanes></Clip></Clips>",
                track.id,
                region.id,
                escape_xml(&region.name),
                !region.muted,
                track.id,
                region.id
            ));
        }
        if !notes.is_empty() {
            let mut positioned = Vec::with_capacity(notes.len());
            for note in notes {
                let end_sample = note
                    .start_sample
                    .checked_add(note.length_samples)
                    .context("MIDI note end sample overflows")?;
                positioned.push((
                    samples_to_beats(
                        note.start_sample,
                        document.sample_rate,
                        &document.tempo_events,
                        f64::from(document.metadata.bpm),
                    )?,
                    samples_to_beats(
                        end_sample,
                        document.sample_rate,
                        &document.tempo_events,
                        f64::from(document.metadata.bpm),
                    )?,
                    note,
                ));
            }
            let clip_start = positioned
                .iter()
                .map(|(start, _, _)| *start)
                .fold(f64::INFINITY, f64::min);
            let clip_end = positioned
                .iter()
                .map(|(_, end, _)| *end)
                .fold(f64::NEG_INFINITY, f64::max);
            lanes.push_str(&format!(
                "<Clips id=\"notes-clips-{}\" timeUnit=\"beats\"><Clip time=\"{clip_start:.9}\" duration=\"{:.9}\" contentTimeUnit=\"beats\"><Notes id=\"notes-{}\" timeUnit=\"beats\">",
                track.id,
                (clip_end - clip_start).max(0.000_001),
                track.id
            ));
            for (start, end, note) in positioned {
                lanes.push_str(&format!(
                    "<Note time=\"{:.9}\" duration=\"{:.9}\" channel=\"{}\" key=\"{}\" vel=\"{:.6}\" rel=\"0.5\"/>",
                    start - clip_start,
                    (end - start).max(0.000_001),
                    note.midi_channel,
                    note.pitch,
                    f64::from(note.velocity) / 127.0
                ));
            }
            lanes.push_str("</Notes></Clip></Clips>");
        }
        if !track.volume_automation.is_empty() {
            lanes.push_str(&format!(
                "<Points id=\"volume-automation-{}\" track=\"{track_id}\" timeUnit=\"beats\"><Target parameter=\"volume-{}\"/>{}</Points>",
                track.id,
                track.id,
                automation_points_xml(
                    &track.volume_automation,
                    document.sample_rate,
                    &document.tempo_events,
                    f64::from(document.metadata.bpm),
                    |value| f64::from(value),
                )?
            ));
        }
        if !track.pan_automation.is_empty() {
            lanes.push_str(&format!(
                "<Points id=\"pan-automation-{}\" track=\"{track_id}\" timeUnit=\"beats\"><Target parameter=\"pan-{}\"/>{}</Points>",
                track.id,
                track.id,
                automation_points_xml(
                    &track.pan_automation,
                    document.sample_rate,
                    &document.tempo_events,
                    f64::from(document.metadata.bpm),
                    |value| f64::from(normalized_pan(value)),
                )?
            ));
        }
        for lane in &track.plugin_automation {
            let Some(parameter_ref) =
                plugin_parameter_refs.get(&(lane.plugin_index, lane.parameter_id))
            else {
                continue;
            };
            let points = plugin_automation_points_xml(
                &lane.points,
                document.sample_rate,
                &document.tempo_events,
                f64::from(document.metadata.bpm),
            )?;
            lanes.push_str(&format!(
                "<Points id=\"plugin-automation-{}-{}-{}\" track=\"{track_id}\" timeUnit=\"beats\" unit=\"normalized\"><Target parameter=\"{parameter_ref}\"/>{points}</Points>",
                track.id, lane.plugin_index, lane.parameter_id
            ));
        }
        lanes.push_str("</Lanes>");
    }

    let tempo_events = ordered_tempo_events(document);
    let base_tempo = tempo_events
        .iter()
        .find(|event| event.beat == 0.0)
        .map_or(f64::from(document.metadata.bpm), |event| event.bpm);
    let signatures = ordered_time_signatures(document);
    let base_signature = signatures.iter().find(|event| event.beat == 0.0);
    let (numerator, denominator) = base_signature
        .map(|event| (event.numerator, event.denominator))
        .unwrap_or((4, 4));
    let mut tempo_points = String::new();
    if tempo_events.is_empty() {
        tempo_points.push_str(&format!(
            "<RealPoint time=\"0\" value=\"{:.9}\" interpolation=\"hold\"/>",
            f64::from(document.metadata.bpm)
        ));
    } else {
        for (index, event) in tempo_events.iter().enumerate() {
            if index == 0 && event.beat > 0.0 {
                tempo_points.push_str(&format!(
                    "<RealPoint time=\"0\" value=\"{:.9}\" interpolation=\"hold\"/>",
                    f64::from(document.metadata.bpm)
                ));
            }
            tempo_points.push_str(&format!(
                "<RealPoint time=\"{:.9}\" value=\"{:.9}\" interpolation=\"{}\"/>",
                event.beat,
                event.bpm,
                if event.ramp { "linear" } else { "hold" }
            ));
        }
    }
    let mut signature_points = String::new();
    if signatures.is_empty() {
        signature_points
            .push_str("<TimeSignaturePoint time=\"0\" numerator=\"4\" denominator=\"4\"/>");
    } else {
        for event in signatures {
            signature_points.push_str(&format!(
                "<TimeSignaturePoint time=\"{:.9}\" numerator=\"{}\" denominator=\"{}\"/>",
                event.beat, event.numerator, event.denominator
            ));
        }
    }
    let markers = document
        .markers
        .iter()
        .map(|marker| {
            format!(
                "<Marker id=\"marker-{}\" time=\"{:.9}\" name=\"{}\"/>",
                marker.id,
                marker.beat,
                escape_xml(&marker.label)
            )
        })
        .collect::<String>();
    let markers_xml = if markers.is_empty() {
        String::new()
    } else {
        format!("<Markers id=\"markers\" timeUnit=\"beats\">{markers}</Markers>")
    };
    let project_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Project version=\"1.0\"><Application name=\"Hirari\" version=\"{}\"/><Transport><Tempo id=\"tempo-base\" name=\"Tempo\" unit=\"bpm\" min=\"20\" max=\"666\" value=\"{base_tempo:.9}\"/><TimeSignature id=\"time-signature-base\" name=\"Time Signature\" numerator=\"{numerator}\" denominator=\"{denominator}\"/></Transport><Structure>{structure}</Structure><Arrangement id=\"arrangement\"><Lanes id=\"arrangement-lanes\" timeUnit=\"beats\">{lanes}</Lanes>{markers_xml}<TempoAutomation id=\"tempo-automation\" timeUnit=\"beats\"><Target parameter=\"tempo-base\"/>{tempo_points}</TempoAutomation><TimeSignatureAutomation id=\"time-signature-automation\" timeUnit=\"beats\"><Target parameter=\"time-signature-base\"/>{signature_points}</TimeSignatureAutomation></Arrangement></Project>",
        env!("CARGO_PKG_VERSION")
    );
    Ok((project_xml, media))
}

fn ordered_tempo_events(
    document: &ProjectDocument,
) -> Vec<crate::project_contracts::TempoEventContract> {
    let mut events = document.tempo_events.clone();
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    events
}

fn ordered_time_signatures(
    document: &ProjectDocument,
) -> Vec<crate::project_contracts::TimeSignatureEventContract> {
    let mut events = document.time_signature_events.clone();
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    events
}

fn resolve_region_path(source_root: &Path, region: &ProjectRegion) -> Result<PathBuf> {
    let path = Path::new(&region.path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_root.join(path)
    };
    if !resolved.is_file() {
        bail!("audio media is missing for region {}", region.id);
    }
    supported_audio_extension(resolved.to_string_lossy().as_ref())
        .context("DAWproject audio format is unsupported")?;
    Ok(resolved)
}

fn supported_audio_extension(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?;
    match extension.to_ascii_lowercase().as_str() {
        "wav" | "rf64" | "aif" | "aiff" | "flac" | "mp3" | "ogg" | "aac" | "m4a" => {
            // Return a stable lowercase extension so extracted assets keep a
            // decoder-recognized suffix on every platform.
            Some(match extension.to_ascii_lowercase().as_str() {
                "aif" => "aif",
                "aiff" => "aiff",
                "rf64" => "rf64",
                "flac" => "flac",
                "mp3" => "mp3",
                "ogg" => "ogg",
                "aac" => "aac",
                "m4a" => "m4a",
                _ => "wav",
            })
        }
        _ => None,
    }
}

/// Read the authoritative media properties through Symphonia so DAWproject
/// interchange can preserve common compressed and lossless source formats.
/// WAV falls back to Hirari's bounded parser for variants Symphonia rejects.
fn read_audio_info(path: &Path) -> Result<(u16, u32, u64)> {
    let decoded = (|| -> Result<(u16, u32, u64)> {
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::errors::Error as SymphoniaError;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let file =
            File::open(path).with_context(|| format!("could not read {}", path.display()))?;
        let source = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .context("unsupported or invalid audio file")?;
        let mut format = probed.format;
        let track = format
            .default_track()
            .context("audio file has no default track")?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let sample_rate = params
            .sample_rate
            .context("audio file has no sample rate")?;
        let channels = params
            .channels
            .context("audio file has no channel layout")?
            .count();
        if !(1..=384_000).contains(&sample_rate) || !(1..=32).contains(&channels) {
            bail!("audio sample rate or channel count is unsupported");
        }
        let mut decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .context("audio codec is unsupported")?;
        let mut frames = 0u64;
        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break
                }
                Err(SymphoniaError::ResetRequired) => {
                    bail!("chained audio streams are unsupported in DAWproject media")
                }
                Err(error) => return Err(error).context("audio packet read failed"),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = decoder
                .decode(&packet)
                .context("audio packet decode failed")?;
            frames = frames
                .checked_add(decoded.frames() as u64)
                .context("audio frame count overflow")?;
        }
        Ok((channels as u16, sample_rate, frames))
    })();
    match decoded {
        Ok(info) => Ok(info),
        Err(error) if supported_audio_extension(path.to_string_lossy().as_ref()) == Some("wav") => {
            read_wav_info(path).map_err(|fallback| {
                anyhow::anyhow!("{error}; WAV metadata fallback failed: {fallback}")
            })
        }
        Err(error) => Err(error),
    }
}

fn write_archive<W: Write + Seek>(
    output: W,
    project_xml: &str,
    metadata_xml: &str,
    media: &HashMap<String, EmbeddedMedia>,
) -> Result<W> {
    let mut archive = ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file("project.xml", options)?;
    archive.write_all(project_xml.as_bytes())?;
    archive.start_file("metadata.xml", options)?;
    archive.write_all(metadata_xml.as_bytes())?;
    let mut paths = media.keys().collect::<Vec<_>>();
    paths.sort();
    let mut buffer = vec![0u8; 1024 * 1024];
    for archive_path in paths {
        let entry = media.get(archive_path).context("media map changed")?;
        if archive_path != &entry.archive_path {
            bail!("DAWproject media path mismatch");
        }
        archive.start_file(archive_path, options)?;
        let mut input = File::open(&entry.source)
            .with_context(|| format!("could not read media {}", entry.source.display()))?;
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            archive.write_all(&buffer[..count])?;
        }
    }
    archive.finish().context("could not finish DAWproject ZIP")
}

fn samples_to_beats(
    samples: u64,
    sample_rate: f64,
    events: &[crate::project_contracts::TempoEventContract],
    base_bpm: f64,
) -> Result<f64> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 || !base_bpm.is_finite() || base_bpm <= 0.0 {
        bail!("project sample rate or base tempo is invalid");
    }
    let mut ordered = events.to_vec();
    ordered.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    if ordered.is_empty() || ordered.first().is_some_and(|event| event.beat > 0.0) {
        ordered.insert(
            0,
            crate::project_contracts::TempoEventContract {
                beat: 0.0,
                bpm: base_bpm,
                ramp: false,
            },
        );
    }
    let mut elapsed_samples = 0.0f64;
    for index in 0..ordered.len() {
        let current = &ordered[index];
        let Some(next) = ordered.get(index + 1) else {
            let seconds = (samples as f64 - elapsed_samples).max(0.0) / sample_rate;
            return Ok(current.beat + seconds * current.bpm / 60.0);
        };
        let beat_span = next.beat - current.beat;
        if beat_span < 0.0 || !current.bpm.is_finite() || current.bpm <= 0.0 {
            bail!("project tempo map is invalid");
        }
        let duration_seconds = if current.ramp {
            120.0 * beat_span / (current.bpm + next.bpm)
        } else {
            60.0 * beat_span / current.bpm
        };
        let segment_samples = duration_seconds * sample_rate;
        if samples as f64 <= elapsed_samples + segment_samples {
            let offset_seconds = (samples as f64 - elapsed_samples).max(0.0) / sample_rate;
            let beat_offset = if current.ramp && duration_seconds > 0.0 {
                let alpha = (next.bpm - current.bpm) / duration_seconds;
                if alpha.abs() < 1e-12 {
                    offset_seconds * current.bpm / 60.0
                } else {
                    (current.bpm * offset_seconds + 0.5 * alpha * offset_seconds.powi(2)) / 60.0
                }
            } else {
                offset_seconds * current.bpm / 60.0
            };
            return Ok(current.beat + beat_offset.clamp(0.0, beat_span));
        }
        elapsed_samples += segment_samples;
    }
    bail!("could not convert sample position to beats")
}

fn plugin_devices_xml(
    track: &ProjectTrack,
    document: &ProjectDocument,
) -> (String, HashMap<(u32, u32), String>) {
    let mut instances = document
        .plugin_instances
        .iter()
        .filter(|instance| instance.track_id == track.id)
        .collect::<Vec<_>>();
    instances.sort_by_key(|instance| instance.slot_index);
    let mut devices = String::new();
    let mut parameter_refs = HashMap::new();
    for instance in instances {
        let (tag, format_name) = match &instance.format {
            crate::project_contracts::PluginFormat::Clap => ("ClapPlugin", "CLAP"),
            crate::project_contracts::PluginFormat::Vst3 => ("Vst3Plugin", "VST3"),
            crate::project_contracts::PluginFormat::AudioUnit => ("AuPlugin", "AU"),
            crate::project_contracts::PluginFormat::BuiltIn => ("Device", "Built-in"),
        };
        let device_id = format!("plugin-device-{}-{}", track.id, instance.slot_index);
        let device_role = if instance.slot_index == 0
            && matches!(track.track_type.as_str(), "Instrument" | "Midi" | "MIDI")
        {
            "instrument"
        } else {
            "audioFX"
        };
        let display_name = if instance.plugin_id.trim().is_empty() {
            format!("{format_name} Plugin")
        } else {
            instance.plugin_id.clone()
        };
        devices.push_str(&format!(
            "<{tag} id=\"{device_id}\" deviceID=\"{}\" deviceName=\"{}\" deviceRole=\"{device_role}\" loaded=\"{}\" name=\"{}\">",
            escape_xml(&instance.plugin_id),
            escape_xml(&display_name),
            !instance.offline && !instance.quarantined,
            escape_xml(&display_name)
        ));
        let slot_values = track
            .plugin_parameter_values
            .get(instance.slot_index as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut parameter_values = std::collections::BTreeMap::<u32, f32>::new();
        if instance.parameter_ids.is_empty() {
            for (parameter_id, value) in slot_values.iter().copied().enumerate() {
                if let Ok(parameter_id) = u32::try_from(parameter_id) {
                    parameter_values.insert(parameter_id, value);
                }
            }
        } else {
            for (index, id) in instance.parameter_ids.iter().enumerate() {
                let Some(parameter_id) = id.parse::<u32>().ok() else {
                    continue;
                };
                if let Some(value) = instance
                    .parameter_values
                    .get(index)
                    .copied()
                    .or_else(|| slot_values.get(index).copied())
                {
                    parameter_values.insert(parameter_id, value);
                }
            }
        }
        for lane in track
            .plugin_automation
            .iter()
            .filter(|lane| lane.plugin_index == instance.slot_index)
        {
            parameter_values
                .entry(lane.parameter_id)
                .or_insert_with(|| {
                    lane.points
                        .first()
                        .map(|point| point.normalized as f32)
                        .unwrap_or(0.0)
                });
        }
        devices.push_str("<Parameters>");
        for (parameter_id, value) in parameter_values {
            if i32::try_from(parameter_id).is_err() {
                continue;
            }
            let reference = format!(
                "plugin-parameter-{}-{}-{}",
                track.id, instance.slot_index, parameter_id
            );
            devices.push_str(&format!(
                "<RealParameter id=\"{reference}\" name=\"Parameter {parameter_id}\" parameterID=\"{parameter_id}\" unit=\"normalized\" min=\"0\" max=\"1\" value=\"{:.9}\"/>",
                value.clamp(0.0, 1.0)
            ));
            parameter_refs.insert((instance.slot_index, parameter_id), reference);
        }
        devices.push_str("</Parameters>");
        devices.push_str(&format!(
            "<Enabled id=\"plugin-enabled-{}-{}\" name=\"Enabled\" value=\"{}\"/></{tag}>",
            track.id, instance.slot_index, !instance.bypassed
        ));
    }
    (devices, parameter_refs)
}

fn is_dawproject_plugin_device(node: roxmltree::Node<'_, '_>) -> bool {
    matches!(
        node.tag_name().name(),
        "Vst2Plugin"
            | "Vst3Plugin"
            | "ClapPlugin"
            | "AuPlugin"
            | "Device"
            | "BuiltinDevice"
            | "Equalizer"
            | "Compressor"
            | "NoiseGate"
            | "Limiter"
    )
}

fn plugin_automation_points_xml(
    points: &[crate::plugin_parameters::PluginAutomationPoint],
    sample_rate: f64,
    tempo_events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<String> {
    let mut xml = String::with_capacity(points.len().saturating_mul(96));
    let mut flattened = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        if flattened.last().is_none() {
            flattened.push((point.sample, point.normalized));
        }
        let Some(next) = points.get(index + 1) else {
            continue;
        };
        if point.curve != 0.0 && next.sample.saturating_sub(point.sample) > 1 {
            const CURVE_SUBDIVISIONS: u64 = 8;
            for subdivision in 1..CURVE_SUBDIVISIONS {
                let sample = point.sample.saturating_add(
                    ((next.sample - point.sample) as f64 * subdivision as f64
                        / CURVE_SUBDIVISIONS as f64)
                        .round() as u64,
                );
                if sample <= point.sample || sample >= next.sample {
                    continue;
                }
                let amount = (sample - point.sample) as f64 / (next.sample - point.sample) as f64;
                let shaped = (amount
                    + point.curve * amount * (1.0 - amount) * (1.0 - 2.0 * amount))
                    .clamp(0.0, 1.0);
                flattened.push((
                    sample,
                    point.normalized + (next.normalized - point.normalized) * shaped,
                ));
            }
        }
        flattened.push((next.sample, next.normalized));
        if flattened.len() > crate::project::MAX_PROJECT_REGIONS {
            bail!("flattened plug-in automation exceeds the DAWproject point limit");
        }
    }
    for (sample, value) in flattened {
        let beat = samples_to_beats(sample, sample_rate, tempo_events, base_bpm)?;
        xml.push_str(&format!(
            "<RealPoint time=\"{beat:.9}\" value=\"{:.9}\" interpolation=\"linear\"/>",
            value
        ));
    }
    Ok(xml)
}

fn automation_points_xml(
    points: &[AutomationPointContract],
    sample_rate: f64,
    tempo_events: &[crate::project_contracts::TempoEventContract],
    base_bpm: f64,
    normalize: impl Fn(f32) -> f64,
) -> Result<String> {
    let mut xml = String::with_capacity(points.len().saturating_mul(96));
    for point in points {
        if point.time > u64::MAX as f64 {
            bail!("automation point time exceeds the project timeline range");
        }
        let beat = samples_to_beats(point.time as u64, sample_rate, tempo_events, base_bpm)?;
        xml.push_str(&format!(
            "<RealPoint time=\"{beat:.9}\" value=\"{:.9}\" interpolation=\"linear\"/>",
            normalize(point.value)
        ));
    }
    Ok(xml)
}

/// Recover a clip-level expression only when it is constant across the clip.
/// Hirari's region model stores gain and pitch as static values, so flattening
/// a changing DAWproject expression to one point would silently destroy edits.
fn parse_clip_expression_value(
    clip: roxmltree::Node<'_, '_>,
    expression: &str,
    target_unit: &str,
) -> Result<Option<f32>> {
    for points in clip
        .descendants()
        .filter(|node| node.has_tag_name("Points"))
    {
        let target = points.children().find(|node| node.has_tag_name("Target"));
        if target.and_then(|node| node.attribute("expression")) != Some(expression) {
            continue;
        }
        let unit = points.attribute("unit").unwrap_or("linear");
        if unit != target_unit && !(expression == "gain" && unit == "decibel") {
            continue;
        }
        let mut constant_value = None::<f32>;
        for point in points
            .children()
            .filter(|node| node.has_tag_name("RealPoint"))
        {
            let mut value = required_number(&point, "value")? as f32;
            if !value.is_finite() {
                bail!("DAWproject clip expression value is not finite");
            }
            if expression == "gain" && unit == "decibel" {
                value = 10.0f32.powf(value / 20.0);
            }
            if let Some(previous) = constant_value {
                if (previous - value).abs() > 1e-5 {
                    return Ok(None);
                }
            } else {
                constant_value = Some(value);
            }
        }
        if constant_value.is_some() {
            return Ok(constant_value);
        }
    }
    Ok(None)
}

fn normalized_pan(pan: f32) -> f32 {
    ((pan.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0)
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn read_wav_info(path: &Path) -> Result<(u16, u32, u64)> {
    let mut file =
        File::open(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header)
        .context("WAV header is truncated")?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        bail!("DAWproject export currently requires RIFF WAV audio");
    }
    let (mut channels, mut sample_rate, mut block_align, mut data_bytes) = (None, None, None, None);
    loop {
        let mut chunk = [0u8; 8];
        match file.read_exact(&mut chunk) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error).context("could not inspect WAV chunks"),
        }
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;
        match &chunk[0..4] {
            b"fmt " if size >= 16 && size <= 4096 => {
                let mut fmt = vec![0; size as usize];
                file.read_exact(&mut fmt)
                    .context("WAV format chunk is truncated")?;
                let format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                if !matches!(format, 1 | 3 | 0xfffe) {
                    bail!("WAV encoding is unsupported for DAWproject export");
                }
                channels = Some(u16::from_le_bytes(fmt[2..4].try_into().unwrap()));
                sample_rate = Some(u32::from_le_bytes(fmt[4..8].try_into().unwrap()));
                block_align = Some(u16::from_le_bytes(fmt[12..14].try_into().unwrap()));
                if size % 2 != 0 {
                    file.seek(std::io::SeekFrom::Current(1))?;
                }
            }
            b"data" => {
                data_bytes = Some(size);
                file.seek(std::io::SeekFrom::Current(
                    i64::try_from(size).unwrap_or(i64::MAX),
                ))?;
                if size % 2 != 0 {
                    file.seek(std::io::SeekFrom::Current(1))?;
                }
            }
            _ => {
                file.seek(std::io::SeekFrom::Current(
                    i64::try_from(size).unwrap_or(i64::MAX),
                ))?;
                if size % 2 != 0 {
                    file.seek(std::io::SeekFrom::Current(1))?;
                }
            }
        }
    }
    let channels = channels.context("WAV format chunk is missing")?;
    let sample_rate = sample_rate.context("WAV sample rate is missing")?;
    let block_align = block_align.context("WAV block alignment is missing")?;
    let data_bytes = data_bytes.context("WAV data chunk is missing")?;
    if channels == 0 || sample_rate == 0 || block_align == 0 {
        bail!("WAV format metadata is invalid");
    }
    Ok((channels, sample_rate, data_bytes / u64::from(block_align)))
}

fn temporary_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    destination.with_file_name(name)
}

fn read_zip_text<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<String> {
    let entry = archive
        .by_name(name)
        .with_context(|| format!("DAWproject is missing {name}"))?;
    if entry.size() > limit {
        bail!("DAWproject {name} exceeds the supported size limit");
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("DAWproject {name} exceeds the supported size limit");
    }
    String::from_utf8(bytes).with_context(|| format!("DAWproject {name} is not UTF-8"))
}

fn read_zip_text_optional<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Option<String>> {
    if !archive.file_names().any(|entry| entry == name) {
        return Ok(None);
    }
    read_zip_text(archive, name, limit).map(Some)
}

fn validate_archive_media_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || path.len() > 4096 || path.contains('\0') || path.contains('\\') {
        bail!("DAWproject media path is invalid");
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("DAWproject media path escapes the archive");
    }
    Ok(())
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn bounded_utf8(value: &str, max_bytes: usize) -> String {
    let end = value
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= max_bytes)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

fn optional_number(node: &roxmltree::Node<'_, '_>, attribute: &str) -> Option<f64> {
    node.attribute(attribute)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

fn required_number(node: &roxmltree::Node<'_, '_>, attribute: &str) -> Result<f64> {
    optional_number(node, attribute).with_context(|| {
        format!(
            "DAWproject element {} has invalid {attribute}",
            node.tag_name().name()
        )
    })
}

fn parse_tempo_events(
    root: roxmltree::Node<'_, '_>,
    _base_bpm: f64,
) -> Result<Vec<TempoEventContract>> {
    let Some(automation) = root
        .descendants()
        .find(|node| node.has_tag_name("TempoAutomation"))
    else {
        return Ok(Vec::new());
    };
    if automation
        .children()
        .filter(|node| node.has_tag_name("RealPoint"))
        .count()
        > 65_536
    {
        bail!("DAWproject contains too many tempo automation points");
    }
    automation
        .children()
        .filter(|node| node.has_tag_name("RealPoint"))
        .map(|point| {
            let beat = required_number(&point, "time")?;
            let bpm = required_number(&point, "value")?;
            let interpolation = point.attribute("interpolation").unwrap_or("hold");
            if !matches!(interpolation, "hold" | "linear") {
                bail!("DAWproject tempo interpolation is unsupported: {interpolation}");
            }
            Ok(TempoEventContract {
                beat,
                bpm,
                ramp: interpolation == "linear",
            })
        })
        .collect()
}

fn normalize_tempo_events(
    mut events: Vec<TempoEventContract>,
    base_bpm: f64,
) -> Result<Vec<TempoEventContract>> {
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    let mut normalized = Vec::<TempoEventContract>::with_capacity(events.len() + 1);
    for event in events {
        event.validate()?;
        if let Some(previous) = normalized.last_mut() {
            if (previous.beat - event.beat).abs() <= f64::EPSILON {
                *previous = event;
                continue;
            }
        }
        normalized.push(event);
    }
    if normalized.first().is_none_or(|event| event.beat > 0.0) {
        normalized.insert(
            0,
            TempoEventContract {
                beat: 0.0,
                bpm: base_bpm,
                ramp: false,
            },
        );
    }
    if normalized.is_empty() {
        normalized.push(TempoEventContract {
            beat: 0.0,
            bpm: base_bpm,
            ramp: false,
        });
    }
    Ok(normalized)
}

fn parse_time_signature_events(
    root: roxmltree::Node<'_, '_>,
    base_numerator: u8,
    base_denominator: u8,
) -> Result<Vec<TimeSignatureEventContract>> {
    let Some(automation) = root
        .descendants()
        .find(|node| node.has_tag_name("TimeSignatureAutomation"))
    else {
        return Ok(Vec::new());
    };
    if automation
        .children()
        .filter(|node| node.has_tag_name("TimeSignaturePoint"))
        .count()
        > 65_536
    {
        bail!("DAWproject contains too many time signature points");
    }
    let mut events = Vec::new();
    for point in automation
        .children()
        .filter(|node| node.has_tag_name("TimeSignaturePoint"))
    {
        let event = TimeSignatureEventContract {
            beat: required_number(&point, "time")?,
            numerator: point
                .attribute("numerator")
                .and_then(|value| value.parse().ok())
                .context("time signature numerator is invalid")?,
            denominator: point
                .attribute("denominator")
                .and_then(|value| value.parse().ok())
                .context("time signature denominator is invalid")?,
        };
        event.validate()?;
        events.push(event);
    }
    normalize_time_signature_events(events, base_numerator, base_denominator)
}

fn normalize_time_signature_events(
    mut events: Vec<TimeSignatureEventContract>,
    base_numerator: u8,
    base_denominator: u8,
) -> Result<Vec<TimeSignatureEventContract>> {
    let base = TimeSignatureEventContract {
        beat: 0.0,
        numerator: base_numerator,
        denominator: base_denominator,
    };
    base.validate()?;
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    let mut normalized = Vec::<TimeSignatureEventContract>::with_capacity(events.len() + 1);
    for event in events {
        event.validate()?;
        if let Some(previous) = normalized.last_mut() {
            if (previous.beat - event.beat).abs() <= f64::EPSILON {
                *previous = event;
                continue;
            }
        }
        normalized.push(event);
    }
    if normalized.first().is_none_or(|event| event.beat > 0.0) {
        normalized.insert(0, base.clone());
    }
    if normalized.is_empty() {
        normalized.push(base);
    }
    Ok(normalized)
}

fn timeline_position_to_samples(
    value: f64,
    unit: &str,
    sample_rate: f64,
    tempo_events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<u64> {
    if !value.is_finite() || value < 0.0 {
        bail!("DAWproject timeline position is invalid");
    }
    let samples = match unit {
        "beats" => beats_to_samples(value, sample_rate, tempo_events, base_bpm)?,
        "seconds" => value * sample_rate,
        other => bail!("unsupported DAWproject timeline unit: {other}"),
    };
    if !samples.is_finite() || samples < 0.0 || samples > u64::MAX as f64 {
        bail!("DAWproject timeline position exceeds Hirari's sample range");
    }
    Ok(samples.round() as u64)
}

fn duration_to_samples(
    start: f64,
    duration: f64,
    unit: &str,
    sample_rate: f64,
    tempo_events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<u64> {
    if !duration.is_finite() || duration < 0.0 {
        bail!("DAWproject fade duration is invalid");
    }
    let start_samples =
        timeline_position_to_samples(start, unit, sample_rate, tempo_events, base_bpm)?;
    let end_samples =
        timeline_position_to_samples(start + duration, unit, sample_rate, tempo_events, base_bpm)?;
    Ok(end_samples.saturating_sub(start_samples))
}

fn beats_to_samples(
    beats: f64,
    sample_rate: f64,
    events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<f64> {
    if !beats.is_finite() || beats < 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        bail!("tempo conversion input is invalid");
    }
    let ordered = normalize_tempo_events(events.to_vec(), base_bpm)?;
    let mut elapsed_seconds = 0.0;
    for (index, current) in ordered.iter().enumerate() {
        let Some(next) = ordered.get(index + 1) else {
            if beats <= current.beat {
                return Ok(elapsed_seconds * sample_rate);
            }
            return Ok(
                (elapsed_seconds + (beats - current.beat) * 60.0 / current.bpm) * sample_rate,
            );
        };
        let beat_span = next.beat - current.beat;
        if beat_span < 0.0 || current.bpm <= 0.0 || next.bpm <= 0.0 {
            bail!("DAWproject tempo map is invalid");
        }
        let duration_seconds = if current.ramp {
            120.0 * beat_span / (current.bpm + next.bpm)
        } else {
            60.0 * beat_span / current.bpm
        };
        if beats <= next.beat {
            let beat_offset = (beats - current.beat).max(0.0).min(beat_span);
            let seconds_offset = if current.ramp && duration_seconds > 0.0 {
                let alpha = (next.bpm - current.bpm) / duration_seconds;
                if alpha.abs() < 1e-12 {
                    beat_offset * 60.0 / current.bpm
                } else {
                    let discriminant = current.bpm.powi(2) + 120.0 * alpha * beat_offset;
                    if discriminant < 0.0 {
                        bail!("tempo ramp cannot be integrated");
                    }
                    (-current.bpm + discriminant.sqrt()) / alpha
                }
            } else {
                beat_offset * 60.0 / current.bpm
            };
            return Ok(
                (elapsed_seconds + seconds_offset.clamp(0.0, duration_seconds)) * sample_rate,
            );
        }
        elapsed_seconds += duration_seconds;
    }
    bail!("could not convert beats to samples")
}
