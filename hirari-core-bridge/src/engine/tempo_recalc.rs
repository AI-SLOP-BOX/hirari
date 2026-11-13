//! Tempo map read/commit helpers, split out of tempo.rs.
use super::*;

pub(crate) fn recalculate_tempo_events(events: &mut [NativeTempoMapEvent], sample_rate: f64) {
    let (mut current_beats, mut previous_sample, mut previous_bpm, mut previous_ramp) =
        (0.0_f64, 0_u64, 120.0_f64, false);
    for event in events {
        let step = event.sample_pos.saturating_sub(previous_sample);
        if step > 0
            && previous_bpm.is_finite()
            && previous_bpm > 0.0
            && event.bpm.is_finite()
            && event.bpm > 0.0
        {
            let average_bpm = if previous_ramp {
                (previous_bpm + event.bpm) * 0.5
            } else {
                previous_bpm
            };
            current_beats += (step as f64 / sample_rate) * (average_bpm / 60.0);
        }
        event.world_beats = current_beats;
        previous_sample = event.sample_pos;
        previous_bpm = event.bpm;
        previous_ramp = event.ramp;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_replace_events_at_beats(
    events: *mut std::ffi::c_void,
    event_count: usize,
    sample_rate: f64,
) -> bool {
    if events.is_null() || event_count == 0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return false;
    }
    let output = std::slice::from_raw_parts_mut(events.cast::<NativeTempoMapEvent>(), event_count);
    let mut candidate = output.to_vec();
    candidate.sort_by(|left, right| left.world_beats.total_cmp(&right.world_beats));
    if !candidate[0].world_beats.is_finite() || candidate[0].world_beats.abs() > 1.0e-9 {
        return false;
    }
    candidate[0].world_beats = 0.0;
    candidate[0].sample_pos = 0;
    for index in 0..candidate.len() {
        let event = candidate[index];
        if !event.world_beats.is_finite()
            || event.world_beats < 0.0
            || !event.bpm.is_finite()
            || !(20.0..=300.0).contains(&event.bpm)
            || (index > 0 && event.world_beats <= candidate[index - 1].world_beats)
        {
            return false;
        }
        if index == 0 {
            continue;
        }
        let previous = candidate[index - 1];
        let beat_delta = event.world_beats - previous.world_beats;
        let effective_bpm = if previous.ramp {
            (previous.bpm + event.bpm) * 0.5
        } else {
            previous.bpm
        };
        if !effective_bpm.is_finite() || effective_bpm <= 0.0 {
            return false;
        }
        let sample_delta = beat_delta * 60.0 / effective_bpm * sample_rate;
        let rounded_delta = (sample_delta + 0.5).floor();
        let maximum_delta = (u64::MAX - previous.sample_pos) as f64;
        if !rounded_delta.is_finite() || rounded_delta < 0.0 || rounded_delta > maximum_delta {
            return false;
        }
        let Some(sample_pos) = previous.sample_pos.checked_add(rounded_delta as u64) else {
            return false;
        };
        candidate[index].sample_pos = sample_pos;
    }
    output.copy_from_slice(&candidate);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_bpm_at_sample(
    events: *const std::ffi::c_void,
    event_count: usize,
    sample_pos: u64,
) -> f64 {
    if events.is_null() || event_count == 0 {
        return 120.0;
    }
    let events = std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count);
    if sample_pos < events[0].sample_pos {
        return events[0].bpm;
    }
    for (index, event) in events.iter().enumerate() {
        if event.sample_pos > sample_pos {
            break;
        }
        if event.ramp && index + 1 < events.len() && sample_pos < events[index + 1].sample_pos {
            let next = &events[index + 1];
            let span = (next.sample_pos - event.sample_pos) as f64;
            let t = if span > 0.0 {
                ((sample_pos - event.sample_pos) as f64 / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return event.bpm + (next.bpm - event.bpm) * t;
        }
    }
    events
        .iter()
        .rev()
        .find(|event| event.sample_pos <= sample_pos)
        .map(|event| event.bpm)
        .unwrap_or(events[0].bpm)
}

/// Tempo conversion used directly by the native engine's immutable event
/// snapshots. This ABI mirrors `TempoMap::Event`; it allocates nothing and
/// performs no synchronization on the audio thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_samples_to_beats(
    events: *const std::ffi::c_void,
    event_count: usize,
    samples: u64,
    sample_rate: f64,
) -> f64 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0.0;
    }
    if event_count == 0 {
        return (samples as f64 / sample_rate) * 2.0;
    }
    if events.is_null() {
        return 0.0;
    }
    let events = std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count);
    if samples < events[0].sample_pos {
        return (samples as f64 / sample_rate) * (events[0].bpm / 60.0);
    }
    let upper = events.partition_point(|event| event.sample_pos <= samples);
    let index = upper.saturating_sub(1).min(events.len() - 1);
    let event = events[index];
    let delta_seconds = (samples - event.sample_pos) as f64 / sample_rate;
    if event.ramp && index + 1 < events.len() {
        let next = events[index + 1];
        let segment_seconds = (next.sample_pos - event.sample_pos) as f64 / sample_rate;
        if segment_seconds > 0.0 {
            let t = (delta_seconds / segment_seconds).clamp(0.0, 1.0);
            let integrated =
                event.bpm * delta_seconds + 0.5 * (next.bpm - event.bpm) * segment_seconds * t * t;
            return event.world_beats + integrated / 60.0;
        }
    }
    event.world_beats + delta_seconds * (event.bpm / 60.0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_beats_to_samples(
    events: *const std::ffi::c_void,
    event_count: usize,
    beats: f64,
    sample_rate: f64,
) -> u64 {
    if !beats.is_finite() || !sample_rate.is_finite() || sample_rate <= 0.0 || beats <= 0.0 {
        return 0;
    }
    if event_count == 0 {
        return (beats * 60.0 / 120.0 * sample_rate) as u64;
    }
    if events.is_null() {
        return 0;
    }
    let events = std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count);
    let upper = events.partition_point(|event| event.world_beats <= beats);
    let index = upper.saturating_sub(1).min(events.len() - 1);
    let event = events[index];
    let delta_beats = beats - event.world_beats;
    if event.ramp && index + 1 < events.len() {
        let next = events[index + 1];
        let segment_seconds = (next.sample_pos - event.sample_pos) as f64 / sample_rate;
        if segment_seconds <= 0.0 || next.world_beats <= event.world_beats {
            return event.sample_pos;
        }
        let delta_bpm = next.bpm - event.bpm;
        let mut delta_seconds = (delta_beats * 60.0 / event.bpm).clamp(0.0, segment_seconds);
        for _ in 0..5 {
            let t = (delta_seconds / segment_seconds).clamp(0.0, 1.0);
            let residual = (event.bpm * delta_seconds + 0.5 * delta_bpm * segment_seconds * t * t)
                / 60.0
                - delta_beats;
            let derivative = (event.bpm + delta_bpm * t) / 60.0;
            if derivative.abs() < 1.0e-12 {
                break;
            }
            delta_seconds = (delta_seconds - residual / derivative).clamp(0.0, segment_seconds);
        }
        return safe_native_sample_offset(event.sample_pos, delta_seconds * sample_rate);
    }
    safe_native_sample_offset(
        event.sample_pos,
        delta_beats / (event.bpm / 60.0) * sample_rate,
    )
}
