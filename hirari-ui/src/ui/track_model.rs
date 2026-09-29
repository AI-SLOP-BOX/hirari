include!("track_model_parts/track_model_core.rs");
include!("track_model_parts/track_model_sync.rs");

pub(crate) fn parse_recording_input_channels(input: &str) -> Option<Vec<u16>> {
    let channels = input
        .strip_prefix("IN ")?
        .split('/')
        .map(|channel| channel.parse::<u16>().ok()?.checked_sub(1))
        .collect::<Option<Vec<_>>>()?;
    if !(1..=32).contains(&channels.len())
        || channels.iter().any(|channel| *channel >= 32)
        || channels
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != channels.len()
    {
        return None;
    }
    Some(channels)
}

pub(crate) fn format_recording_input_channels(channels: &[u16]) -> Option<String> {
    if !(1..=32).contains(&channels.len())
        || channels.iter().any(|channel| *channel >= 32)
        || channels
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != channels.len()
    {
        return None;
    }
    Some(format!(
        "IN {}",
        channels
            .iter()
            .map(|channel| (channel + 1).to_string())
            .collect::<Vec<_>>()
            .join("/")
    ))
}

pub(crate) fn recording_input_bus_label(channels: &[u16]) -> Option<String> {
    if channels.is_empty() || channels.len() > 32 || channels.iter().any(|channel| *channel >= 32) {
        return None;
    }
    let mut ordered = channels.to_vec();
    ordered.sort_unstable();
    if ordered.windows(2).any(|pair| pair[1] != pair[0] + 1) {
        return Some(format!(
            "Input {}",
            ordered
                .iter()
                .map(|channel| (channel + 1).to_string())
                .collect::<Vec<_>>()
                .join("+")
        ));
    }
    let first = *ordered.first()? + 1;
    let last = *ordered.last()? + 1;
    let layout = match channels.len() {
        1 => "Mono",
        2 => "Stereo",
        4 => "Quad",
        6 => "5.1",
        8 => "7.1",
        _ => "",
    };
    let layout = if layout.is_empty() {
        format!("{} ch", channels.len())
    } else {
        layout.to_owned()
    };
    Some(if first == last {
        format!("{layout} In {first}")
    } else {
        format!("{layout} In {first}–{last}")
    })
}

pub(crate) fn recording_input_bus_label_with_names(
    channels: &[u16],
    channel_names: &[String],
) -> Option<String> {
    let base = recording_input_bus_label(channels)?;
    let names = channels
        .iter()
        .map(|channel| channel_names.get(*channel as usize).cloned())
        .collect::<Option<Vec<_>>>()?;
    if names.iter().any(|name| name.trim().is_empty()) {
        return Some(base);
    }
    Some(format!("{base} · {}", names.join(" / ")))
}

pub(crate) fn recording_input_bus_label_from_assignment(input: &str) -> String {
    parse_recording_input_channels(input)
        .and_then(|channels| recording_input_bus_label(&channels))
        .unwrap_or_else(|| "Input".to_owned())
}
