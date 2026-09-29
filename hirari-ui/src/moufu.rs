//! Hirari's project integration with the Moufu Hub.

use hirari_core_bridge::HirariCore;
use serde_json::Value;
use slint::Model;

pub use hirari_moufu_adapter::{
    EntityDescriptor, LinkInfo, LinkStatus, MoufuEvent, MoufuPublisher, PayloadTransport,
    HIRARI_PROJECT_CONTRACT, HIRARI_PROJECT_DATA_TYPE, HIRARI_PROJECT_ENTITY,
};

fn parse_snapshot_array(label: &str, json: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(json) {
        Ok(value) if value.is_array() => Some(value),
        Ok(_) => {
            log::warn!("Hirari skipped Moufu publication: {label} is not an array");
            None
        }
        Err(error) => {
            log::warn!("Hirari skipped Moufu publication: invalid {label} JSON: {error}");
            None
        }
    }
}

/// Publish the native project snapshot with UI-owned input choices. The small
/// selection list is captured here; the adapter merges it into project JSON on
/// its worker so the UI avoids a second full-layout parse and serialization.
pub(crate) fn publish_layout(
    publisher: &MoufuPublisher,
    core: &HirariCore,
    tracks: &slint::VecModel<crate::slint_ui::Z_Track>,
    is_transient: bool,
) {
    let recording_input_channels = (0..tracks.row_count())
        .filter_map(|row| tracks.row_data(row))
        .filter(|track| track.r#type == "Audio" || track.r#type == "Vocal")
        .filter_map(|track| {
            let channels =
                crate::ui::track_model::parse_recording_input_channels(track.input.as_str())?;
            let track_id = u32::try_from(track.id).ok()?;
            Some((
                track_id,
                channels
                    .into_iter()
                    .map(|channel| channel.saturating_add(1))
                    .collect(),
            ))
        })
        .collect();
    let Some(tracks) = parse_snapshot_array("project layout", &core.get_project_layout_json())
    else {
        publisher.discard_latest_layout();
        return;
    };
    let Some(midi_notes) = parse_snapshot_array("MIDI notes", &core.midi_notes_json()) else {
        publisher.discard_latest_layout();
        return;
    };
    let Some(markers) = parse_snapshot_array("markers", &core.markers_json()) else {
        publisher.discard_latest_layout();
        return;
    };
    let sample_rate = core.get_sample_rate();
    let tempo_bpm = core.get_tempo();
    if !sample_rate.is_finite() || sample_rate <= 0.0 || !tempo_bpm.is_finite() || tempo_bpm <= 0.0
    {
        log::warn!("Hirari skipped Moufu publication: invalid sample rate or tempo");
        publisher.discard_latest_layout();
        return;
    }
    let raw_tempo_events = core.get_tempo_events();
    let raw_time_signature_events = core.get_time_signature_events();
    if raw_tempo_events.len() % 3 != 0
        || raw_time_signature_events.len() % 3 != 0
        || raw_tempo_events.iter().any(|value| !value.is_finite())
        || raw_time_signature_events
            .iter()
            .any(|value| !value.is_finite())
        || raw_tempo_events.chunks_exact(3).any(|event| {
            event[0] < 0.0 || !(20.0..=300.0).contains(&event[1]) || ![0.0, 1.0].contains(&event[2])
        })
        || raw_time_signature_events.chunks_exact(3).any(|event| {
            event[0] < 0.0
                || event[1].fract() != 0.0
                || !(1.0..=32.0).contains(&event[1])
                || event[2].fract() != 0.0
                || ![1.0, 2.0, 4.0, 8.0, 16.0, 32.0].contains(&event[2])
        })
        || raw_tempo_events
            .chunks_exact(3)
            .zip(raw_tempo_events.chunks_exact(3).skip(1))
            .any(|(previous, next)| previous[0] >= next[0])
        || raw_time_signature_events
            .chunks_exact(3)
            .zip(raw_time_signature_events.chunks_exact(3).skip(1))
            .any(|(previous, next)| previous[0] >= next[0])
    {
        log::warn!("Hirari skipped Moufu publication: malformed tempo or meter map");
        publisher.discard_latest_layout();
        return;
    }
    let tempo_events = raw_tempo_events
        .chunks_exact(3)
        .map(|event| {
            serde_json::json!({
                "beat": event[0],
                "bpm": event[1],
                "ramp": event[2] != 0.0,
            })
        })
        .collect::<Vec<_>>();
    let time_signature_events = raw_time_signature_events
        .chunks_exact(3)
        .map(|event| {
            serde_json::json!({
                "beat": event[0],
                "numerator": event[1] as u8,
                "denominator": event[2] as u8,
            })
        })
        .collect::<Vec<_>>();
    let snapshot = serde_json::json!({
        "schema": HIRARI_PROJECT_CONTRACT,
        "schema_version": 2,
        "sample_rate": sample_rate,
        "tempo_bpm": tempo_bpm,
        "tempo_events": tempo_events,
        "time_signature_events": time_signature_events,
        "markers": markers,
        "tracks": tracks,
        "midi_notes": midi_notes,
    });
    let Ok(snapshot_json) = serde_json::to_string(&snapshot) else {
        log::warn!("Hirari skipped Moufu publication: project snapshot serialization failed");
        publisher.discard_latest_layout();
        return;
    };
    publisher.publish_layout_with_recording_inputs_owned(
        snapshot_json,
        recording_input_channels,
        is_transient,
    );
}
