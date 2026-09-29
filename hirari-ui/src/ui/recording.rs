use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::slint_ui::{
    sync_tracks_from_engine, ui_error_message, AppWindow, CompingActions, RecordingActions,
    TransportActions, UiErrorKind, Z_CompRange, Z_CompTake, Z_Track,
};
use crate::ui::midi_input::{MidiRecordingRequest, SharedMidiRecordingRequest};

fn sync_recording_status(ui: &AppWindow, core: &HirariCore) {
    ui.set_recording_lifecycle(core.recording_lifecycle_label().into());
    ui.set_recording_take_count(core.recording_take_count() as i32);
    ui.set_active_recording_take(core.active_recording_take() as i32);
    let snapshot = serde_json::from_str::<serde_json::Value>(&core.comping_snapshot_json())
        .unwrap_or_default();
    let takes = snapshot
        .get("takes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|take| {
            Some(Z_CompTake {
                id: i32::try_from(take.get("id")?.as_i64()?).ok()?,
                name: take.get("name")?.as_str()?.into(),
                wave: slint::ModelRc::new(VecModel::from(Vec::<f32>::new())),
            })
        })
        .collect::<Vec<_>>();
    ui.set_comp_takes(slint::ModelRc::new(VecModel::from(takes)));
    let bounds = snapshot
        .get("takes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|take| {
            Some((
                take.get("id")?.as_i64()? as i32,
                take.get("start_sample")?.as_u64()?,
                take.get("end_sample")?.as_u64()?,
            ))
        })
        .collect::<Vec<_>>();
    let ranges = snapshot
        .get("current_comp")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|segment| {
            let take_id = segment.get("take_id")?.as_i64()? as i32;
            let start = segment.get("start")?.as_u64()?;
            let end = start.checked_add(segment.get("len")?.as_u64()?)?;
            let (_, take_start, take_end) = bounds.iter().find(|(id, _, _)| *id == take_id)?;
            let span = take_end.saturating_sub(*take_start);
            if span == 0 {
                return None;
            }
            Some(Z_CompRange {
                take_id,
                start: (start.saturating_sub(*take_start) as f64 / span as f64).clamp(0.0, 1.0)
                    as f32,
                end: (end.saturating_sub(*take_start) as f64 / span as f64).clamp(0.0, 1.0) as f32,
            })
        })
        .collect::<Vec<_>>();
    ui.set_comp_ranges(slint::ModelRc::new(VecModel::from(ranges)));
    let sample_rate = core.get_sample_rate().max(1.0);
    let fade_seconds = snapshot
        .get("current_comp")
        .and_then(serde_json::Value::as_array)
        .and_then(|segments| segments.first())
        .and_then(|segment| segment.get("crossfade_samples"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0.01, |samples| samples as f32 / sample_rate as f32)
        .clamp(0.0, 0.25);
    ui.set_comp_crossfade(fade_seconds);
}

/// Copies already-decoded arrangement peaks into comp lanes. Waveform decoding
/// remains on the engine's existing worker queue; this function only combines
/// resident peak vectors and runs on the existing 128 ms analysis cadence.
pub(crate) fn sync_comping_take_waveforms(
    ui: &AppWindow,
    core: &HirariCore,
    tracks: &VecModel<Z_Track>,
) {
    let Ok(snapshot) = serde_json::from_str::<serde_json::Value>(&core.comping_snapshot_json())
    else {
        return;
    };
    let takes = snapshot
        .get("takes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|take| {
            let id = i32::try_from(take.get("id")?.as_i64()?).ok()?;
            let mut wave = vec![0.0f32; 128];
            let mut has_wave = false;
            for region in take
                .get("regions")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(track_id), Some(region_id)) = (
                    region.get("track_id").and_then(serde_json::Value::as_i64),
                    region.get("region_id").and_then(serde_json::Value::as_i64),
                ) else {
                    continue;
                };
                let Some(track) = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .find(|track| track.id == track_id as i32)
                else {
                    continue;
                };
                let Some(clip) = track.clips.iter().find(|clip| clip.id == region_id as i32) else {
                    continue;
                };
                let count = clip.points.row_count();
                if count == 0 {
                    continue;
                }
                has_wave = true;
                for index in 0..count {
                    let Some(peak) = clip.points.row_data(index) else {
                        continue;
                    };
                    let bin = index.saturating_mul(wave.len()) / count;
                    if let Some(target) = wave.get_mut(bin) {
                        *target = target.max(if peak.is_finite() {
                            peak.abs().min(1.0)
                        } else {
                            0.0
                        });
                    }
                }
            }
            Some(Z_CompTake {
                id,
                name: take.get("name")?.as_str()?.into(),
                wave: slint::ModelRc::new(VecModel::from(if has_wave { wave } else { Vec::new() })),
            })
        })
        .collect::<Vec<_>>();
    ui.set_comp_takes(slint::ModelRc::new(VecModel::from(takes)));
}

/// Recording lifecycle bindings. The engine remains the source of truth;
/// this module only coordinates armed-track selection and UI feedback.
pub fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    recording_target: Rc<RefCell<Option<Vec<(u32, Vec<u16>)>>>>,
    midi_recording_request: SharedMidiRecordingRequest,
) {
    sync_recording_status(ui, &core);
    let weak = ui.as_weak();
    ui.global::<RecordingActions>().on_select_recording_take({
        let weak = weak.clone();
        let core = core.clone();
        move |index| {
            if index < 0 || !core.select_recording_take(index as usize) {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                sync_recording_status(&ui, &core);
                let waveform = core.recording_capture_waveform(96);
                if !waveform.is_empty() {
                    ui.set_recording_waveform(slint::ModelRc::new(slint::VecModel::from(waveform)));
                }
                ui.set_last_action(format!("SELECT TAKE {}", index + 1).into());
            }
        }
    });
    ui.global::<CompingActions>().on_select_take({
        let weak = weak.clone();
        let core = core.clone();
        move |take_id| {
            if take_id <= 0 {
                return;
            }
            let message = if core.select_comp_take(take_id as u32) {
                format!("COMP TAKE {take_id}")
            } else {
                format!("COMP TAKE {take_id} UNAVAILABLE")
            };
            if let Some(ui) = weak.upgrade() {
                sync_recording_status(&ui, &core);
                ui.set_last_action(message.into());
            }
        }
    });
    ui.global::<CompingActions>().on_swipe_take({
        let weak = weak.clone();
        let core = core.clone();
        move |take_id, start, end| {
            if take_id <= 0 || !start.is_finite() || !end.is_finite() {
                return;
            }
            let snapshot = serde_json::from_str::<serde_json::Value>(&core.comping_snapshot_json())
                .unwrap_or_default();
            let take = snapshot
                .get("takes")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .find(|take| {
                    take.get("id").and_then(serde_json::Value::as_i64) == Some(i64::from(take_id))
                });
            let bounds = take.and_then(|take| {
                Some((
                    take.get("start_sample")?.as_u64()?,
                    take.get("end_sample")?.as_u64()?,
                ))
            });
            let ok = bounds.is_some_and(|(take_start, take_end)| {
                let span = take_end.saturating_sub(take_start);
                let swipe_start = take_start
                    .saturating_add((span as f64 * start.clamp(0.0, 1.0) as f64).round() as u64);
                let swipe_end = take_start
                    .saturating_add((span as f64 * end.clamp(0.0, 1.0) as f64).round() as u64);
                let fade = (core.get_sample_rate().max(1.0) * 0.01)
                    .round()
                    .min(u32::MAX as f64) as u32;
                core.swipe_comp_take_region(take_id as u32, swipe_start, swipe_end, fade)
            });
            if let Some(ui) = weak.upgrade() {
                sync_recording_status(&ui, &core);
                ui.set_last_action(
                    if ok {
                        format!("QUICK SWIPE TAKE {take_id}")
                    } else {
                        format!("QUICK SWIPE TAKE {take_id} REJECTED")
                    }
                    .into(),
                );
            }
        }
    });
    ui.global::<CompingActions>().on_crossfade_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |seconds| {
            let samples = (seconds.clamp(0.0, 0.25) as f64 * core.get_sample_rate().max(1.0))
                .round()
                .min(u32::MAX as f64) as u32;
            let message = if core.set_comp_crossfade_samples(samples) {
                format!("COMP CROSSFADE {:.0} MS", seconds * 1000.0)
            } else {
                "COMP CROSSFADE REJECTED".to_owned()
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_comp_crossfade(seconds.clamp(0.0, 0.25));
                ui.set_last_action(message.into());
            }
        }
    });
    ui.global::<CompingActions>().on_preview({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            let (take_id, fade) = core.resolve_comp_at(core.get_playhead());
            let message = if take_id == 0 {
                "COMP PREVIEW: NO ACTIVE TAKE".to_owned()
            } else {
                format!("COMP PREVIEW: TAKE {} / FADE {}", take_id, fade)
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(message.into());
            }
        }
    });
    ui.global::<TransportActions>().on_toggle_record({
        let weak = weak.clone();
        let recording_target = recording_target.clone();
        let midi_recording_request = midi_recording_request.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_is_rec() {
                    if let Some(request) = midi_recording_request.borrow_mut().as_mut() {
                        request.active = false;
                        request.stop_host_time = Some(core.midi_host_time_now());
                    }
                    let targets = recording_target.borrow().clone();
                    if targets.as_ref().is_none_or(Vec::is_empty) {
                        ui.set_is_rec(false);
                        ui.set_recording_lifecycle("Finalizing".into());
                        ui.set_last_action("STOPPING MIDI TAKE".into());
                        return;
                    }
                    let Some(targets) = targets else {
                        return;
                    };
                    let target_tracks = targets
                        .iter()
                        .filter_map(|(id, _)| {
                            (0..tracks.row_count())
                                .filter_map(|row| tracks.row_data(row))
                                .find(|track| track.id.max(0) as u32 == *id)
                        })
                        .collect::<Vec<_>>();
                    if target_tracks.len() != targets.len() {
                        ui.set_is_rec(false);
                        *recording_target.borrow_mut() = None;
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::AudioDevice,
                                "record stop failed: an armed track no longer exists",
                            )
                            .into(),
                        );
                        return;
                    }
                    let target_names = target_tracks
                        .iter()
                        .map(|track| track.name.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let project_path = last_saved_path.borrow().clone();
                    let commit =
                        core.commit_recording_capture_to_tracks(&targets, project_path.as_deref());
                    match commit {
                        Ok(frame_count) => {
                            ui.set_is_rec(false);
                            *recording_target.borrow_mut() = None;
                            sync_recording_status(&ui, &core);
                            // Promote every committed capture into the Core
                            // comping registry.  The Arrange view can now use
                            // the same take identity as the recorder instead
                            // of maintaining a second UI-only take list.
                            sync_tracks_from_engine(&tracks, &core);
                            ui.set_last_action(
                                format!("RECORDED: {} frames on {}", frame_count, target_names)
                                    .into(),
                            );
                        }
                        Err(error) => {
                            // A failed commit (for example an empty input
                            // take) still terminates the capture session.
                            // Keep the UI transport and settings controls from
                            // remaining latched in recording mode.
                            ui.set_is_rec(false);
                            *recording_target.borrow_mut() = None;
                            sync_recording_status(&ui, &core);
                            ui.set_last_action(
                                ui_error_message(
                                    UiErrorKind::AudioDevice,
                                    &format!("record stop failed: {error}"),
                                )
                                .into(),
                            );
                        }
                    }
                    return;
                }

                if midi_recording_request.borrow().is_some() {
                    ui.set_last_action("RECORD BLOCKED: MIDI TAKE IS STILL FINALIZING".into());
                    return;
                }

                let armed_tracks = (0..tracks.row_count())
                    .filter_map(|row| tracks.row_data(row))
                    .filter(|track| track.armed)
                    .collect::<Vec<_>>();
                if armed_tracks.is_empty() || armed_tracks.len() > 32 {
                    ui.set_last_action("RECORD BLOCKED: ARM 1–32 AUDIO OR MIDI TRACKS".into());
                    return;
                }
                if armed_tracks.iter().any(|track| {
                    !matches!(
                        track.r#type.as_str(),
                        "Audio" | "Vocal" | "Midi" | "MIDI" | "Instrument"
                    )
                }) {
                    ui.set_last_action("RECORD BLOCKED: UNSUPPORTED ARMED TRACK TYPE".into());
                    return;
                }
                let audio_tracks = armed_tracks
                    .iter()
                    .filter(|track| matches!(track.r#type.as_str(), "Audio" | "Vocal"))
                    .cloned()
                    .collect::<Vec<_>>();
                let midi_tracks = armed_tracks
                    .iter()
                    .filter(|track| matches!(track.r#type.as_str(), "Midi" | "MIDI" | "Instrument"))
                    .collect::<Vec<_>>();
                if !audio_tracks.is_empty() && !core.is_audio_device_ready() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::AudioDevice, "device not ready").into(),
                    );
                    return;
                }
                let active_input_uid = core.audio_input_device_uid();
                if let Some(track) = audio_tracks.iter().find(|track| {
                    !track.input_endpoint_uid.is_empty()
                        && track.input_endpoint_uid.as_str() != active_input_uid
                }) {
                    let endpoint = if track.input_endpoint_name.is_empty() {
                        "the project's audio input device".to_owned()
                    } else {
                        track.input_endpoint_name.to_string()
                    };
                    ui.set_last_action(format!("RECORD BLOCKED: RECONNECT {endpoint}").into());
                    return;
                }
                if !midi_tracks.is_empty() && !core.start_midi_input() {
                    ui.set_last_action("RECORD BLOCKED: MIDI INPUT UNAVAILABLE".into());
                    return;
                }
                let channels = if audio_tracks.is_empty() {
                    0
                } else {
                    core.get_audio_input_channel_count()
                };
                if !audio_tracks.is_empty() && (channels == 0 || channels > 32) {
                    if !midi_tracks.is_empty() {
                        core.stop_midi_input();
                    }
                    ui.set_last_action("RECORD BLOCKED: NO SUPPORTED AUDIO INPUT CHANNELS".into());
                    return;
                }
                let targets = audio_tracks
                    .iter()
                    .filter_map(|track| {
                        crate::ui::track_model::parse_recording_input_channels(track.input.as_str())
                            .map(|input_channels| (track.id.max(0) as u32, input_channels))
                    })
                    .collect::<Vec<_>>();
                if targets.len() != audio_tracks.len()
                    || targets
                        .iter()
                        .any(|(_, inputs)| inputs.iter().any(|channel| *channel >= channels))
                {
                    if !midi_tracks.is_empty() {
                        core.stop_midi_input();
                    }
                    ui.set_last_action(
                        "RECORD BLOCKED: CHECK ARMED TRACK INPUT ASSIGNMENTS".into(),
                    );
                    return;
                }
                let sample_rate = core.get_sample_rate();
                // The disk-backed StreamingRecordingWriter is authoritative
                // for the take. Keep only a bounded preview in memory for the
                // live waveform instead of reserving five minutes of PCM.
                const PREVIEW_SECONDS: f64 = 30.0;
                const PREVIEW_SAMPLE_BUDGET: usize = 8_388_608;
                let max_preview_frames = if channels == 0 {
                    0
                } else {
                    PREVIEW_SAMPLE_BUDGET / channels as usize
                };
                let max_frames = if audio_tracks.is_empty() {
                    0
                } else if sample_rate.is_finite() && sample_rate > 0.0 {
                    (sample_rate * PREVIEW_SECONDS).clamp(1.0, max_preview_frames as f64) as usize
                } else {
                    0
                };
                let started_transport = !core.is_playing();
                if started_transport && !core.try_set_playing(true) {
                    if !midi_tracks.is_empty() {
                        core.stop_midi_input();
                    }
                    ui.set_last_action("RECORD BLOCKED: AUDIO TRANSPORT NOT READY".into());
                    return;
                }
                if started_transport {
                    ui.set_is_ply(true);
                }
                let loop_range = if core.is_loop_enabled() {
                    let start = core.cycle_start_sample();
                    let end = core.cycle_end_sample();
                    (end > start).then_some((start, end))
                } else {
                    None
                };
                let mut start_sample = core.get_playhead();
                if let Some((cycle_start, cycle_end)) = loop_range {
                    // The playback callback wraps a cursor at or beyond the
                    // right locator to the cycle start before rendering its
                    // next block. Align the capture and MIDI origins with
                    // that first recorded sample instead of the stale cursor.
                    if start_sample >= cycle_end {
                        start_sample = cycle_start;
                        core.set_playhead(cycle_start);
                    }
                }
                let punch_range = (|| -> anyhow::Result<Option<(u64, u64)>> {
                    if ui.get_punch_enabled() {
                        let beats_to_samples = |beats: f32| -> Option<u64> {
                            if !beats.is_finite() || beats < 0.0 {
                                return None;
                            }
                            // Punch points are absolute timeline beats. Use
                            // the engine's tempo map so changes and ramps map
                            // to the same sample positions as playback.
                            Some(core.beats_to_samples(beats as f64))
                        };
                        let (Some(punch_in), Some(punch_out)) = (
                            beats_to_samples(ui.get_punch_in()),
                            beats_to_samples(ui.get_punch_out()),
                        ) else {
                            anyhow::bail!("invalid punch range");
                        };
                        if punch_out <= punch_in {
                            anyhow::bail!("invalid punch range");
                        }
                        if start_sample >= punch_out {
                            anyhow::bail!("punch range has already ended");
                        }
                        Ok(Some((punch_in, punch_out)))
                    } else {
                        Ok(None)
                    }
                })();
                let midi_punch_range = punch_range.as_ref().ok().copied().flatten();
                let capture_result = punch_range.and_then(|punch_range| {
                    if audio_tracks.is_empty() {
                        return Ok(());
                    }
                    core.arm_recording_capture(sample_rate as f32, channels, max_frames)?;
                    if !core.set_recording_capture_cycle_range(loop_range) {
                        anyhow::bail!("recording cycle range could not be stored");
                    }
                    let started = match punch_range {
                        Some((punch_in, punch_out)) => core.start_recording_capture_with_punch(
                            sample_rate as f32,
                            channels,
                            max_frames,
                            start_sample,
                            punch_in,
                            punch_out,
                        ),
                        None => core.start_recording_capture(
                            sample_rate as f32,
                            channels,
                            max_frames,
                            start_sample,
                        ),
                    };
                    started?;
                    Ok(())
                });
                match capture_result {
                    Ok(()) => {
                        *recording_target.borrow_mut() = if targets.is_empty() {
                            None
                        } else {
                            Some(targets.clone())
                        };
                        if !midi_tracks.is_empty() {
                            *midi_recording_request.borrow_mut() = Some(MidiRecordingRequest {
                                active: true,
                                audio_capture: !audio_tracks.is_empty(),
                                target_track_ids: midi_tracks
                                    .iter()
                                    .map(|track| track.id.max(0) as u32)
                                    .collect(),
                                start_sample,
                                start_host_time: core.midi_host_time_now(),
                                dropped_events_at_start: core.midi_dropped_input_events(),
                                stop_host_time: None,
                                punch_range: midi_punch_range,
                                loop_range,
                            });
                        }
                        ui.set_is_rec(true);
                        sync_recording_status(&ui, &core);
                        if audio_tracks.is_empty() && !midi_tracks.is_empty() {
                            ui.set_recording_lifecycle("Recording".into());
                        }
                        let routing = targets
                            .iter()
                            .zip(audio_tracks.iter())
                            .map(|((_, inputs), track)| {
                                let input =
                                    crate::ui::track_model::format_recording_input_channels(inputs)
                                        .unwrap_or_else(|| "IN ?".to_owned());
                                format!("{} ← {}", track.name, input)
                            })
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let midi_routing = midi_tracks
                            .iter()
                            .map(|track| track.name.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        let routing = match (routing.is_empty(), midi_routing.is_empty()) {
                            (false, false) => {
                                format!("RECORDING AUDIO: {routing} · MIDI: {midi_routing}")
                            }
                            (false, true) => format!("RECORDING AUDIO: {routing}"),
                            (true, false) => format!("RECORDING MIDI: {midi_routing}"),
                            (true, true) => "RECORDING".to_owned(),
                        };
                        ui.set_last_action(routing.into());
                    }
                    Err(error) => {
                        if started_transport {
                            let _ = core.try_set_playing(false);
                            ui.set_is_ply(core.is_playing());
                        }
                        if !midi_tracks.is_empty() {
                            core.stop_midi_input();
                        }
                        *midi_recording_request.borrow_mut() = None;
                        sync_recording_status(&ui, &core);
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::AudioDevice,
                                &format!("record start failed: {error}"),
                            )
                            .into(),
                        );
                    }
                }
            }
        }
    });
}
