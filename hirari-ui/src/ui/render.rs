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

pub(crate) use crate::ui_shared::render_ops::{
    cancellation_status, kick_stem, prepare_rejected, render_format_label, render_tail_seconds,
    stem_step, StemStep,
};

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
