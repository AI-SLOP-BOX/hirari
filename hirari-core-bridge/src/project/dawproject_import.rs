//! DawProject archive reader importing a project into Hirari, split out of dawproject.rs.
use super::*;

pub(in crate::dawproject) struct ImportAssetCleanup {
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
        || !crate::dsp_limits::is_supported_sample_rate(sample_rate)
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
    let tempo_events = parse_tempo_events(root, base_bpm)?;
    let time_signature_events =
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
#[path = "dawproject_import_tracks.rs"]
mod tracks_;
#[path = "dawproject_import_media.rs"]
mod media_;

    let TrackBuildOutcome {
        track_ids,
        mut tracks,
        track_content,
        plugin_parameter_refs_by_track,
        unsupported_plugin_count,
        tempo_events,
        time_signature_events,
    } = build_tracks(
        &external_tracks,
        structure,
        base_bpm,
        base_numerator,
        base_denominator,
        tempo_events,
        time_signature_events,
    )?;

    let MediaImportOutcome { regions, mut warp_markers, midi_notes, embedded_media_count } =
        import_content(
            &mut archive,
            arrangement,
            &external_tracks,
            &track_ids,
            &track_content,
            &mut tracks,
            &plugin_parameter_refs_by_track,
            &tempo_events,
            base_bpm,
            sample_rate,
            &asset_dir,
            &asset_dir_name,
            &destination_parent,
        )?;

    use tracks_::{build_tracks, TrackBuildOutcome};
    use media_::{import_content, MediaImportOutcome};

    let mut markers = Vec::new();
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
