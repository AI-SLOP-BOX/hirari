//! Read-only linked-project preview helpers for the Moufu integration.
//! Shared by the Slint dock and the AmeUI Moufu card so both surfaces render
//! received state the same way (and without remote-bidi leaking into rows).

use hirari_moufu_adapter::PayloadTransport;
use std::collections::HashMap;

pub(crate) fn linked_project_preview(
    source: &str,
    version: u64,
    transient: bool,
    payload: &PayloadTransport,
) -> (String, String) {
    let state = match payload {
        PayloadTransport::Inline(value) => value,
        PayloadTransport::SharedMemory(descriptor) => {
            return (
                source.to_owned(),
                format!(
                    "{} snapshot · v{version} · shared memory payload ({} bytes)",
                    if transient {
                        "Live preview"
                    } else {
                        "Committed"
                    },
                    descriptor.size_bytes
                ),
            );
        }
    };
    let tracks = state
        .as_array()
        .or_else(|| state.get("tracks").and_then(serde_json::Value::as_array));
    let Some(tracks) = tracks else {
        return (
            source.to_owned(),
            format!(
                "{} snapshot · v{version} · payload received, but no track list is available",
                if transient {
                    "Live preview"
                } else {
                    "Committed"
                }
            ),
        );
    };
    let mut lines = vec![format!(
        "{} snapshot · v{version} · {} tracks",
        if transient {
            "Live preview"
        } else {
            "Committed"
        },
        tracks.len()
    )];
    if let Some(object) = state.as_object() {
        let mut details = Vec::new();
        if let Some(bpm) = object.get("tempo_bpm").and_then(serde_json::Value::as_f64) {
            if bpm.is_finite() {
                details.push(format!("{bpm:.2} BPM"));
            }
        }
        for (key, label) in [
            ("midi_notes", "MIDI notes"),
            ("markers", "markers"),
            ("time_signature_events", "meter changes"),
        ] {
            if let Some(items) = object.get(key).and_then(serde_json::Value::as_array) {
                details.push(format!("{} {label}", items.len()));
            }
        }
        if let Some(events) = object
            .get("tempo_events")
            .and_then(serde_json::Value::as_array)
        {
            details.push(format!("{} tempo points", events.len()));
        }
        if !details.is_empty() {
            lines[0].push_str(" · ");
            lines[0].push_str(&details.join(" · "));
        }
    }
    for track in tracks.iter().take(8) {
        let name = track
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(one_line_remote_preview_text)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Unnamed track".to_owned());
        let region_count = track
            .get("regions")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        let input_channels = track
            .get("recording_input_channels")
            .and_then(serde_json::Value::as_array)
            .map(|channels| {
                channels
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .map(|channel| channel.to_string())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|channels| !channels.is_empty());
        if let Some(input_channels) = input_channels {
            lines.push(format!(
                "{name} · input {input_channels} · {region_count} regions"
            ));
        } else {
            lines.push(format!("{name} · {region_count} regions"));
        }
    }
    if tracks.len() > 8 {
        lines.push(format!("…and {} more", tracks.len() - 8));
    }
    (source.to_owned(), lines.join("\n"))
}

/// Remote project names are user supplied and must not be able to inject
/// extra preview rows, reorder text with bidi controls, or flood the UI.
pub(crate) fn one_line_remote_preview_text(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_space = true;
    let mut truncated = false;
    for character in value.chars() {
        let bidi_control = matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        );
        if bidi_control {
            continue;
        }
        let character = if character.is_control() || character.is_whitespace() {
            ' '
        } else {
            character
        };
        if character == ' ' {
            if previous_was_space {
                continue;
            }
            previous_was_space = true;
        } else {
            previous_was_space = false;
        }
        if output.chars().count() >= 120 {
            truncated = true;
            break;
        }
        output.push(character);
    }
    let output = output.trim_end().to_owned();
    if truncated {
        format!("{output}…")
    } else {
        output
    }
}

pub(crate) fn accept_moufu_version(
    versions: &mut HashMap<String, (u64, bool)>,
    source: &str,
    version: u64,
    transient: bool,
) -> bool {
    if versions
        .get(source)
        .is_some_and(|(current_version, current_transient)| {
            version < *current_version
                || (version == *current_version && (!*current_transient || transient))
        })
    {
        return false;
    }
    versions.insert(source.to_owned(), (version, transient));
    true
}
