use hirari_core_bridge::stable_api::{CoreApiV1, LoadedMixRenderRequest, WaveContainer};
use hirari_core_bridge::HirariCore;
use slint::ComponentHandle;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::slint_ui::{
    default_render_output_path, display_path, ui_error_message, unix_time_millis, AppWindow,
    RenderActions, UiErrorKind,
};
use crate::ui::operation_gate::{OperationGate, OperationKind};
use std::path::PathBuf;

/// A stems export is owned by core (`stem_batch_*`), which does the track
/// selection, the naming, the destination check, the per-stem WAV validation
/// and the rollback of a partly written batch. Each surface only renders the
/// stem core arms and reports the result back, so these variants are the
/// whole of what a surface has to understand.
#[derive(Debug)]
pub(crate) enum StemStep {
    /// A stem is armed: render `output` into `format`.
    Ready {
        output: PathBuf,
        total: u32,
        format: u32,
    },
    /// One stem published; `stem_batch_next` now reports the following target.
    StemDone { rendered: u32, total: u32 },
    /// The whole batch published.
    Complete { outputs: Vec<String>, format: u32 },
    /// Core rejected the batch or rolled it back; this is its reason.
    Failed(String),
}

/// Decodes one `stem_batch_*` document. Core owns every code; this only maps
/// a document onto what a surface has to do next.
pub(crate) fn stem_step(document: &str) -> StemStep {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(document) else {
        return StemStep::Failed("engine returned an unreadable stem export status".to_owned());
    };
    if value.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return StemStep::Failed(stem_failure_reason(&value));
    }
    let number = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default() as u32
    };
    match value.get("state").and_then(serde_json::Value::as_str) {
        Some("ready") => StemStep::Ready {
            output: value
                .get("output")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default(),
            total: number("total"),
            format: number("format"),
        },
        Some("stem_done") => StemStep::StemDone {
            rendered: number("index"),
            total: number("total"),
        },
        Some("complete") => StemStep::Complete {
            outputs: value
                .get("outputs")
                .and_then(serde_json::Value::as_array)
                .map(|outputs| {
                    outputs
                        .iter()
                        .filter_map(|output| output.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            format: number("format"),
        },
        _ => StemStep::Failed("stem export reported an unexpected state".to_owned()),
    }
}

/// Turns core's bare error document into the sentence the action log shows.
/// The codes stay core's; this is only the translation for a human.
pub(crate) fn stem_failure_reason(value: &serde_json::Value) -> String {
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let message = value.get("message").and_then(serde_json::Value::as_str);
    match (code, message) {
        ("stem_render_failed", Some(message)) => message.to_owned(),
        ("stem_output_exists", _) => value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("stem output already exists: {path}"))
            .unwrap_or_else(|| "stem output already exists".to_owned()),
        ("invalid_stem_track_selection", _) => "invalid stem track selection".to_owned(),
        ("stem_track_not_found", _) => "one of the selected tracks no longer exists".to_owned(),
        ("no_tracks_for_stem_export", _) => "there are no tracks to export".to_owned(),
        ("invalid_stem_output_directory", _) => "stem output folder is unavailable".to_owned(),
        ("unsupported_render_format", _) => "unsupported render format".to_owned(),
        ("engine_unavailable", _) => "engine is unavailable".to_owned(),
        ("project_transaction_busy", _) => "project is busy".to_owned(),
        (code, _) => format!("stem export could not be prepared ({code})"),
    }
}

/// `stem_batch_prepare` answers with `{"ok":true,...}` rather than a step, so
/// this is what tells a surface whether core accepted the batch, and produces
/// the sentence to show when it did not.
pub(crate) fn prepare_rejected(document: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(document) {
        Ok(value) if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) => None,
        Ok(value) => Some(stem_failure_reason(&value)),
        Err(_) => Some("engine returned an unreadable stem export status".to_owned()),
    }
}

/// Kicks the engine's own bounce thread for an armed stem. Arming already
/// happened inside `stem_batch_next`, which is the only thing that differs
/// from the CLI: `render stems` blocks on the engine's synchronous bounce
/// instead of letting its timer drive the queue.
pub(crate) fn kick_stem(core: &HirariCore, output: &PathBuf, format: u32) -> bool {
    core.start_render_async_to_format(output.to_string_lossy().as_ref(), format)
}

pub(crate) fn render_format_label(format: u32) -> &'static str {
    match format {
        1 => "WAVE64 Float32",
        2 => "Float32 WAV",
        3 => "PCM24 WAV",
        _ => "PCM16 WAV",
    }
}

pub(crate) fn render_tail_seconds(index: i32) -> f32 {
    [0.0, 1.0, 2.0, 5.0, 10.0, 30.0]
        .get(index.clamp(0, 5) as usize)
        .copied()
        .unwrap_or(2.0)
}

pub(crate) fn cancellation_status(accepted: bool) -> (&'static str, &'static str, &'static str) {
    if accepted {
        (
            "CANCEL_REQUESTED",
            "CANCELLATION REQUESTED",
            "RENDER CANCELLATION REQUESTED",
        )
    } else {
        (
            "CANCEL_REJECTED",
            "CANCELLATION REJECTED · RENDER CONTINUES",
            "RENDER CANCELLATION REJECTED",
        )
    }
}

/// Render queue and cancellation bindings. Progress telemetry is handled by
/// the existing timer; this module owns only commands that change render state.
pub(crate) fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    started: Arc<AtomicU64>,
    output_path: Arc<Mutex<std::path::PathBuf>>,
    operation_gate: OperationGate,
    render_lease: Arc<Mutex<Option<crate::ui::operation_gate::OperationLease>>>,
) {
    let render_api = Rc::new(CoreApiV1::from_shared_core(core.clone()));
    let weak = ui.as_weak();
    ui.global::<RenderActions>().on_start_render({
        let weak = weak.clone();
        let render_api = render_api.clone();
        let core = core.clone();
        let started = started.clone();
        let output_path = output_path.clone();
        let operation_gate = operation_gate.clone();
        let render_lease = render_lease.clone();
        move |requested_path, requested_format| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_is_rendering() {
                ui.set_last_action("RENDER ALREADY RUNNING".into());
                return;
            }
            let Some(lease) = operation_gate.try_enter(OperationKind::Render) else {
                ui.set_last_action("PROJECT BUSY: RENDER IGNORED".into());
                return;
            };
            let output = if requested_path.is_empty() {
                default_render_output_path()
            } else {
                std::path::PathBuf::from(requested_path.as_str())
            };
            if let Ok(mut current) = output_path.lock() {
                *current = output.clone();
            }
            let container = match requested_format {
                1 => WaveContainer::WavPcm24,
                2 => WaveContainer::WavFloat32,
                3 => WaveContainer::Wave64,
                _ => WaveContainer::Wav,
            };
            core.set_offline_render_tail_seconds(render_tail_seconds(
                ui.global::<RenderActions>().get_render_tail_index(),
            ));
            if render_api
                .queue_loaded_mix(LoadedMixRenderRequest {
                    output_path: output.clone(),
                    container,
                })
                .is_ok()
            {
                if core.stem_batch_progress().is_some() {
                    // A parked stems batch must not outlive into a mix
                    // render; core owns it, so core is what clears it.
                    core.stem_batch_abort();
                }
                if let Ok(mut active) = render_lease.lock() {
                    *active = Some(lease);
                }
                started.store(unix_time_millis(), Ordering::Release);
                ui.set_is_rendering(true);
                ui.global::<RenderActions>().set_stem_export_mode(false);
                ui.set_render_progress(0.0);
                ui.set_render_progress_available(false);
                ui.set_render_progress_indeterminate(true);
                ui.set_render_elapsed_seconds(0);
                ui.set_render_state("QUEUED".into());
                ui.set_render_error("WAITING FOR ENGINE STATUS".into());
                ui.set_export_open(false);
                ui.set_last_action(format!("RENDER QUEUED: {}", display_path(&output)).into());
            } else {
                core.set_offline_render_tail_seconds(0.0);
                started.store(0, Ordering::Release);
                ui.set_is_rendering(false);
                ui.set_render_state("FAILED_TO_QUEUE".into());
                ui.set_render_error(
                    ui_error_message(UiErrorKind::Render, "engine busy or render could not start")
                        .into(),
                );
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "could not queue render").into(),
                );
            }
        }
    });

    ui.global::<RenderActions>().on_start_stems({
        let weak = weak.clone();
        let core = core.clone();
        let started = started.clone();
        let output_path = output_path.clone();
        let operation_gate = operation_gate.clone();
        let render_lease = render_lease.clone();
        move |requested_dir, jobs_json| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_is_rendering() {
                ui.set_last_action("RENDER ALREADY RUNNING".into());
                return;
            }
            let output_dir = PathBuf::from(requested_dir.as_str());
            if !output_dir.is_dir() {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "stem output folder is unavailable")
                        .into(),
                );
                return;
            }
            // Only the ids matter: core derives every filename through the
            // one policy shared with `render stems`, from the same project
            // snapshot, so the two surfaces cannot disagree about names.
            let Ok(requested) = serde_json::from_str::<Vec<(u32, String)>>(jobs_json.as_str())
            else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "there are no tracks to export").into(),
                );
                return;
            };
            let track_ids: Vec<u32> = requested
                .into_iter()
                .map(|(track_id, _)| track_id)
                .collect();
            let Some(lease) = operation_gate.try_enter(OperationKind::Render) else {
                ui.set_last_action("PROJECT BUSY: STEM EXPORT IGNORED".into());
                return;
            };
            let selected_format = ui.global::<RenderActions>().get_render_format();
            // Engine container numbering, which does not follow the order the
            // options are listed in the UI; never pass an index through raw.
            let format = match selected_format {
                1 => 3,
                2 => 2,
                3 => 1,
                _ => 0,
            };
            // `prepare` validates the whole batch, names it, and rejects a
            // destination that already holds a stem *before* anything is
            // rendered. The trailing `false, true` is pre-fader off and
            // inserts in, the same settings `render stems` passes.
            let prepared = core.stem_batch_prepare(
                output_dir.to_string_lossy().as_ref(),
                format,
                &track_ids,
                render_tail_seconds(ui.global::<RenderActions>().get_render_tail_index()),
                false,
                true,
            );
            if let Some(reason) = prepare_rejected(&prepared) {
                // Dropping the lease here releases the gate again.
                ui.set_last_action(ui_error_message(UiErrorKind::Render, &reason).into());
                return;
            }
            let (first, track_count, first_format) = match stem_step(&core.stem_batch_next()) {
                StemStep::Ready {
                    output,
                    total,
                    format,
                    ..
                } => (output, total, format),
                StemStep::Failed(reason) => {
                    ui.set_last_action(ui_error_message(UiErrorKind::Render, &reason).into());
                    return;
                }
                // `prepare` just parked a non-empty queue, so `next` cannot
                // report anything other than its first target.
                _ => {
                    core.stem_batch_abort();
                    ui.set_last_action(
                        ui_error_message(UiErrorKind::Render, "stem export queue is invalid")
                            .into(),
                    );
                    return;
                }
            };
            if !kick_stem(&core, &first, first_format) {
                core.stem_batch_abort();
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Render, "stem render could not be queued").into(),
                );
                return;
            }
            if let Ok(mut active) = render_lease.lock() {
                *active = Some(lease);
            }
            if let Ok(mut current) = output_path.lock() {
                *current = output_dir.clone();
            }
            started.store(unix_time_millis(), Ordering::Release);
            ui.set_is_rendering(true);
            ui.global::<RenderActions>().set_stem_export_mode(true);
            ui.set_render_progress(0.0);
            ui.set_render_progress_available(true);
            ui.set_render_progress_indeterminate(false);
            ui.set_render_elapsed_seconds(0);
            ui.set_render_state("QUEUED".into());
            ui.set_render_error("PREPARING STEM EXPORT".into());
            ui.set_export_open(false);
            ui.set_last_action(
                format!(
                    "STEM EXPORT QUEUED: {track_count} tracks · {}",
                    display_path(&output_dir)
                )
                .into(),
            );
        }
    });

    ui.global::<RenderActions>().on_cancel_render({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let (state, error, action) = cancellation_status(core.cancel_render());
                ui.set_render_state(state.into());
                ui.set_render_error(error.into());
                ui.set_last_action(action.into());
                if state == "CANCEL_REJECTED" {
                    // Keep the render state active: a rejected cancellation
                    // must never make the UI claim that rendering stopped.
                    ui.set_is_rendering(true);
                    ui.set_render_progress_indeterminate(true);
                }
            }
        }
    });

    ui.global::<RenderActions>().on_pause_render({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                if core.pause_render() {
                    ui.set_render_state("PAUSED".into());
                    ui.set_render_error("RENDER PAUSED · CHECKPOINT RETAINED".into());
                    ui.set_last_action("RENDER PAUSED".into());
                } else {
                    ui.set_last_action("PAUSE REJECTED · RENDER NOT ACTIVE".into());
                }
            }
        }
    });

    ui.global::<RenderActions>().on_resume_render({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                if core.resume_render() {
                    ui.set_render_state("RENDERING".into());
                    ui.set_render_error("RENDER RESUMED".into());
                    ui.set_last_action("RENDER RESUMED".into());
                } else {
                    ui.set_last_action("RESUME REJECTED · RENDER NOT PAUSED".into());
                }
            }
        }
    });

    ui.global::<RenderActions>().on_dismiss_render({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_is_rendering(false);
                ui.set_render_state("IDLE".into());
                ui.set_last_action("RENDER PANEL DISMISSED".into());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{cancellation_status, prepare_rejected, stem_step, StemStep};
    use hirari_core_bridge::HirariCore;

    /// Builds an offline core with the two tracks a stems batch needs, and a
    /// private output folder for it to publish into.
    fn batch_fixture(tag: &str) -> (HirariCore, u32, u32, std::path::PathBuf) {
        let core = HirariCore::new_offline().expect("offline core initializes");
        let first = core.add_audio_track();
        let second = core.add_audio_track();
        assert_ne!(first, 0, "first track must be valid");
        assert_ne!(second, 0, "second track must be valid");
        let output_dir = std::env::temp_dir().join(format!(
            "hirari-stem-batch-{tag}-{}-{}",
            std::process::id(),
            crate::slint_ui::unix_time_millis()
        ));
        std::fs::create_dir_all(&output_dir).expect("stem output dir must be created");
        (core, first, second, output_dir)
    }

    /// Stands in for the engine publishing the stem it was just asked for.
    /// `settle(true)` validates this through the same `validate_render_output`
    /// the CLI uses, so the batch only advances over a structurally valid WAV.
    fn publish(output: &std::path::Path) {
        hirari_core_bridge::export::write_wav_float32(output, &[0.25, -0.5], 48_000, 1)
            .expect("simulated stem must be writable");
    }

    #[test]
    fn cancellation_status_preserves_running_state_when_rejected() {
        assert_eq!(
            cancellation_status(true),
            (
                "CANCEL_REQUESTED",
                "CANCELLATION REQUESTED",
                "RENDER CANCELLATION REQUESTED"
            )
        );
        assert_eq!(
            cancellation_status(false),
            (
                "CANCEL_REJECTED",
                "CANCELLATION REJECTED · RENDER CONTINUES",
                "RENDER CANCELLATION REJECTED"
            )
        );
    }

    /// The GUI walks a stems export one step at a time from its telemetry
    /// timer, while `render stems` steps the same queue inside a single
    /// blocking call. The blocking call is covered in core; this observes the
    /// *stepwise* drive a surface outside core performs, which is where a
    /// document the GUI cannot decode would strand an export mid-batch.
    #[test]
    fn a_surface_can_step_the_core_owned_stem_batch_to_completion() {
        let (core, first, second, output_dir) = batch_fixture("walk");
        let dir_text = output_dir.to_string_lossy().into_owned();

        let prepared = core.stem_batch_prepare(&dir_text, 0, &[first, second], 2.0, false, true);
        assert_eq!(
            prepare_rejected(&prepared),
            None,
            "core must accept a two-track batch, got {prepared}"
        );

        let mut published = Vec::new();
        let outputs = loop {
            assert!(
                published.len() <= 2,
                "the batch must not take more steps than it has tracks"
            );
            let armed = stem_step(&core.stem_batch_next());
            let StemStep::Ready {
                output,
                total,
                format,
            } = armed
            else {
                panic!("core must arm a stem, got {armed:?}");
            };
            assert_eq!(total, 2, "both tracks belong to this batch");
            assert_eq!(format, 0, "the prepared format is reported back");
            assert!(
                !published.contains(&output.to_string_lossy().into_owned()),
                "core must arm each track once, but {} was armed twice",
                output.display()
            );

            publish(&output);
            published.push(output.to_string_lossy().into_owned());

            match stem_step(&core.stem_batch_settle(true)) {
                StemStep::StemDone { rendered, total } => {
                    assert_eq!(total, 2, "the batch still holds both tracks");
                    assert_eq!(
                        rendered as usize,
                        published.len(),
                        "the queue only advances when a stem was published"
                    );
                }
                StemStep::Complete { outputs, format } => {
                    assert_eq!(format, 0, "completion reports the format that ran");
                    break outputs;
                }
                other => panic!("settle must advance or complete, got {other:?}"),
            }
        };

        assert_eq!(
            outputs, published,
            "completion must report exactly the stems the engine was asked to render, in order"
        );
        assert_eq!(
            core.stem_batch_progress(),
            None,
            "a completed batch is handed back so the next export can start"
        );
        for output in &outputs {
            assert!(
                std::path::Path::new(output).exists(),
                "a published stem must survive completion: {output}"
            );
        }

        std::fs::remove_dir_all(&output_dir).expect("stem output cleanup must succeed");
    }

    /// A stem that fails takes the whole batch with it. The GUI used to keep
    /// its own list of started files for this rollback, which is exactly the
    /// second copy core now owns: if the two disagreed, a failed GUI export
    /// would leave stems the CLI's rollback would have removed.
    #[test]
    fn a_failed_stem_rolls_back_everything_the_batch_published() {
        let (core, first, second, output_dir) = batch_fixture("rollback");
        let dir_text = output_dir.to_string_lossy().into_owned();

        let prepared = core.stem_batch_prepare(&dir_text, 0, &[first, second], 2.0, false, true);
        assert_eq!(
            prepare_rejected(&prepared),
            None,
            "core must accept a two-track batch, got {prepared}"
        );

        let StemStep::Ready {
            output: first_out, ..
        } = stem_step(&core.stem_batch_next())
        else {
            panic!("core must arm the first stem");
        };
        publish(&first_out);
        assert!(
            matches!(
                stem_step(&core.stem_batch_settle(true)),
                StemStep::StemDone { .. }
            ),
            "the first stem must publish"
        );

        // The second render reports failure and never produces a file.
        let StemStep::Ready {
            output: second_out, ..
        } = stem_step(&core.stem_batch_next())
        else {
            panic!("core must arm the second stem");
        };
        let failed = stem_step(&core.stem_batch_settle(false));
        let StemStep::Failed(reason) = failed else {
            panic!("a failed render must be reported as a failure, got {failed:?}");
        };
        assert!(
            reason.contains("stem render failed for track"),
            "core must name the stem that failed, got {reason}"
        );

        assert!(
            !first_out.exists(),
            "the published stem must be rolled back, but {} survives",
            first_out.display()
        );
        assert!(
            !second_out.exists(),
            "no output may be left for a failed stem"
        );
        assert_eq!(
            core.stem_batch_progress(),
            None,
            "a rolled back batch is handed back so it cannot be resumed half-way"
        );
        assert_eq!(
            std::fs::read_dir(&output_dir)
                .expect("output dir must remain readable")
                .count(),
            0,
            "a failed export must leave no partial set of stems"
        );

        std::fs::remove_dir_all(&output_dir).expect("stem output cleanup must succeed");
    }
}
