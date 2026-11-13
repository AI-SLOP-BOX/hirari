//! Track construction phase of the DAWproject import, split out of dawproject_import.rs.
use super::*;

pub(in crate::dawproject) struct TrackBuildOutcome {
    pub track_ids: HashMap<String, u32>,
    pub tracks: Vec<ProjectTrack>,
    pub track_content: Vec<(String, String)>,
    pub plugin_parameter_refs_by_track: HashMap<String, HashMap<String, (u32, u32)>>,
    pub unsupported_plugin_count: usize,
    pub tempo_events: Vec<TempoEventContract>,
    pub time_signature_events: Vec<TimeSignatureEventContract>,
}

pub(in crate::dawproject) fn build_tracks(
    external_tracks: &[roxmltree::Node<'_, '_>],
    structure: roxmltree::Node<'_, '_>,
    base_bpm: f64,
    base_numerator: u8,
    base_denominator: u8,
    mut tempo_events: Vec<TempoEventContract>,
    mut time_signature_events: Vec<TimeSignatureEventContract>,
) -> Result<TrackBuildOutcome> {
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

    Ok(TrackBuildOutcome {
        track_ids,
        tracks,
        track_content,
        plugin_parameter_refs_by_track,
        unsupported_plugin_count,
        tempo_events,
        time_signature_events,
    })
}
