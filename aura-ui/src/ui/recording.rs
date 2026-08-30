use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

use crate::slint_ui::{
    clamp_selection_index, sync_tracks_from_engine, ui_error_message, AppWindow, CompingActions,
    RecordingActions, TransportActions, UiErrorKind, Z_Track,
};

fn sync_recording_status(ui: &AppWindow, core: &AuraCore) {
    ui.set_recording_lifecycle(core.recording_lifecycle_label().into());
    ui.set_recording_take_count(core.recording_take_count() as i32);
    ui.set_active_recording_take(core.active_recording_take() as i32);
}

/// Recording lifecycle bindings. The engine remains the source of truth;
/// this module only coordinates armed-track selection and UI feedback.
pub fn install(
    ui: &AppWindow,
    core: Rc<AuraCore>,
    tracks: Rc<VecModel<Z_Track>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
) {
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
        move |index| {
            if index < 0 {
                return;
            }
            let take_id = index as u32 + 1;
            let message = if core.select_comp_take(take_id) {
                format!("COMP TAKE {}", index + 1)
            } else {
                format!("COMP TAKE {} UNAVAILABLE", index + 1)
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(message.into());
            }
        }
    });
    ui.global::<CompingActions>().on_crossfade_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |value| {
            let message = if core.set_comp_crossfade_normalized(value) {
                format!("COMP CROSSFADE {:.0}%", value * 100.0)
            } else {
                "COMP CROSSFADE REJECTED".to_owned()
            };
            if let Some(ui) = weak.upgrade() {
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
        move || {
            if let Some(ui) = weak.upgrade() {
                if ui.get_is_rec() {
                    let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
                    let target_row = if tracks
                        .row_data(selected)
                        .map(|track| track.armed)
                        .unwrap_or(false)
                    {
                        Some(selected)
                    } else {
                        (0..tracks.row_count()).find(|&row| {
                            tracks
                                .row_data(row)
                                .map(|track| track.armed)
                                .unwrap_or(false)
                        })
                    };
                    let Some(target_row) = target_row else {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::AudioDevice,
                                "record stop failed: no armed track",
                            )
                            .into(),
                        );
                        return;
                    };
                    let Some(track) = tracks.row_data(target_row) else {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::AudioDevice,
                                "record stop failed: no track selected",
                            )
                            .into(),
                        );
                        return;
                    };
                    let project_path = last_saved_path.borrow().clone();
                    match core.commit_recording_capture_to_track(
                        track.id.max(0) as u32,
                        project_path.as_deref(),
                    ) {
                        Ok(frame_count) => {
                            ui.set_is_rec(false);
                            sync_recording_status(&ui, &core);
                            // Promote every committed capture into the Core
                            // comping registry.  The Arrange view can now use
                            // the same take identity as the recorder instead
                            // of maintaining a second UI-only take list.
                            let take_index = core.recording_take_count();
                            let take_id = take_index.max(1) as u32;
                            let _ = core.register_comp_take(
                                take_id,
                                &format!("Take {take_index}"),
                                0,
                                frame_count as u64,
                            );
                            sync_tracks_from_engine(&tracks, &core);
                            ui.set_last_action(
                                format!("RECORDED: {} frames on {}", frame_count, track.name)
                                    .into(),
                            );
                        }
                        Err(error) => {
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

                if !core.is_audio_device_ready() {
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::AudioDevice, "device not ready").into(),
                    );
                    return;
                }
                let armed = (0..tracks.row_count()).any(|row| {
                    tracks
                        .row_data(row)
                        .map(|track| track.armed)
                        .unwrap_or(false)
                });
                if !armed {
                    ui.set_last_action("RECORD BLOCKED: ARM A TRACK FIRST".into());
                    return;
                }
                let sample_rate = core.get_sample_rate();
                // The disk-backed StreamingRecordingWriter is authoritative
                // for the take. Keep only a bounded preview in memory for the
                // live waveform instead of reserving five minutes of PCM.
                const PREVIEW_SECONDS: f64 = 30.0;
                let max_frames = if sample_rate.is_finite() && sample_rate > 0.0 {
                    (sample_rate * PREVIEW_SECONDS).clamp(1.0, 4_194_304.0) as usize
                } else {
                    0
                };
                let start_sample = core.get_playhead();
                let arm_result = core.arm_recording_capture(sample_rate as f32, 2, max_frames);
                let capture_result = arm_result.and_then(|()| {
                    if ui.get_punch_enabled() {
                        let bpm = core.get_tempo();
                        let beats_to_samples = |beats: f32| -> Option<u64> {
                            if !beats.is_finite() || beats < 0.0 || !bpm.is_finite() || bpm <= 0.0 {
                                return None;
                            }
                            let samples =
                                (beats as f64 * 60.0 * sample_rate as f64 / bpm as f64).round();
                            if !samples.is_finite() || samples < 0.0 || samples > u64::MAX as f64 {
                                None
                            } else {
                                Some(samples as u64)
                            }
                        };
                        let punch_in = beats_to_samples(ui.get_punch_in());
                        let punch_out = beats_to_samples(ui.get_punch_out());
                        match (punch_in, punch_out) {
                            (Some(punch_in), Some(punch_out)) => core
                                .start_recording_capture_with_punch(
                                    sample_rate as f32,
                                    2,
                                    max_frames,
                                    start_sample,
                                    punch_in,
                                    punch_out,
                                ),
                            _ => Err(anyhow::anyhow!("invalid punch range")),
                        }
                    } else {
                        core.start_recording_capture(
                            sample_rate as f32,
                            2,
                            max_frames,
                            start_sample,
                        )
                    }
                });
                match capture_result {
                    Ok(()) => {
                        ui.set_is_rec(true);
                        sync_recording_status(&ui, &core);
                        ui.set_last_action("RECORDING".into());
                    }
                    Err(error) => {
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
