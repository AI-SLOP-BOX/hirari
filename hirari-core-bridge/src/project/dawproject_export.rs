//! DawProject archive writer and project XML builder, split out of dawproject.rs.
use super::*;

pub(in crate::dawproject) struct EmbeddedMedia {
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

pub(in crate::dawproject) fn build_project_xml(
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

pub(in crate::dawproject) fn ordered_tempo_events(
    document: &ProjectDocument,
) -> Vec<crate::project_contracts::TempoEventContract> {
    let mut events = document.tempo_events.clone();
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    events
}

pub(in crate::dawproject) fn ordered_time_signatures(
    document: &ProjectDocument,
) -> Vec<crate::project_contracts::TimeSignatureEventContract> {
    let mut events = document.time_signature_events.clone();
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    events
}

pub(in crate::dawproject) fn write_archive<W: Write + Seek>(
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

pub(in crate::dawproject) fn plugin_devices_xml(
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

pub(in crate::dawproject) fn plugin_automation_points_xml(
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

pub(in crate::dawproject) fn automation_points_xml(
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
