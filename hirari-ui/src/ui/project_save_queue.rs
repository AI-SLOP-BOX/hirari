//! Serial background publication for immutable project snapshots.

use crate::slint_ui::{self, PreparedUiProjectSave, Z_Track};
use hirari_core_bridge::HirariCore;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

use super::operation_gate::OperationLease;

#[derive(Clone, Debug)]
pub(crate) enum SaveIntent {
    Auto,
    Manual {
        success_prefix: String,
        failure_label: String,
    },
    RecoveryAs {
        success_prefix: String,
    },
}

struct SaveJob {
    id: u64,
    save: PreparedUiProjectSave,
}

struct WorkerCompletion {
    id: u64,
    success: bool,
}

struct LayoutBuildJob {
    id: u64,
    name: String,
    bpm: f32,
    sample_rate: f64,
    aux_track_ids: String,
    layout_json: String,
    _worker_permit: crate::ui::worker_budget::WorkerPermit,
}

struct LayoutBuildCompletion {
    id: u64,
    result: Result<(String, hirari_core_bridge::project::ProjectDocument), String>,
}

struct PendingLayoutBuild {
    id: u64,
    name: String,
    bpm: f32,
    native_revision: u64,
    project_revision: u64,
    aux_track_ids: String,
    audio_input_assignments: Vec<hirari_core_bridge::project::TrackAudioInputAssignment>,
    step_sequencer_patterns_json: String,
}

struct PendingSave {
    id: u64,
    path: String,
    fingerprint: u64,
    intent: SaveIntent,
    after_completion: Option<Box<dyn FnOnce(bool)>>,
    native_snapshot: Option<PendingNativeSnapshot>,
    layout_build: Option<PendingLayoutBuild>,
    _lease: OperationLease,
}

struct PendingNativeSnapshot {
    request_id: u64,
    name: String,
    bpm: f32,
    project_revision: u64,
    sample_rate: f64,
    aux_track_ids: String,
    cancel_requested: bool,
    audio_input_assignments: Vec<hirari_core_bridge::project::TrackAudioInputAssignment>,
    step_sequencer_patterns_json: String,
    _worker_permit: crate::ui::worker_budget::WorkerPermit,
}

pub(crate) struct ProjectSaveCompletion {
    pub(crate) path: String,
    pub(crate) fingerprint: u64,
    pub(crate) intent: SaveIntent,
    pub(crate) success: bool,
    pub(crate) after_completion: Option<Box<dyn FnOnce(bool)>>,
}

/// UI-thread coordinator. Native serialization runs on the engine-owned
/// worker, layout normalization/decoding on a bounded Rust worker, and Core
/// metadata integration plus final publication on their respective control
/// and publication paths.
pub(crate) struct ProjectSaveQueue {
    sender: SyncSender<SaveJob>,
    completions: Receiver<WorkerCompletion>,
    layout_sender: SyncSender<LayoutBuildJob>,
    layout_completions: Receiver<LayoutBuildCompletion>,
    pending: RefCell<Option<PendingSave>>,
    next_id: AtomicU64,
}

impl ProjectSaveQueue {
    pub(crate) fn start() -> Rc<Self> {
        let (sender, receiver) = mpsc::sync_channel::<SaveJob>(1);
        let (completion_sender, completions) = mpsc::channel::<WorkerCompletion>();
        let worker = std::thread::Builder::new()
            .name("hirari-project-save".to_owned())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let success = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        slint_ui::publish_prepared_ui_project_save(job.save)
                    }))
                    .unwrap_or_else(|_| {
                        log::error!("Hirari project save worker panicked during publication");
                        false
                    });
                    if completion_sender
                        .send(WorkerCompletion {
                            id: job.id,
                            success,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        if let Err(error) = worker {
            log::error!("Could not start Hirari project save worker: {error}");
        }
        let (layout_sender, layout_receiver) = mpsc::sync_channel::<LayoutBuildJob>(1);
        let (layout_completion_sender, layout_completions) =
            mpsc::channel::<LayoutBuildCompletion>();
        let layout_worker = std::thread::Builder::new()
            .name("hirari-project-layout-decode".to_owned())
            .spawn(move || {
                while let Ok(job) = layout_receiver.recv() {
                    let _worker_permit = job._worker_permit;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        HirariCore::project_layout_document_seed(
                            job.layout_json,
                            &job.aux_track_ids,
                            &job.name,
                            job.bpm,
                            job.sample_rate,
                        )
                    }))
                    .unwrap_or_else(|_| {
                        Err(anyhow::anyhow!("project layout decode worker panicked"))
                    })
                    .map_err(|error| format!("{error:#}"));
                    if layout_completion_sender
                        .send(LayoutBuildCompletion { id: job.id, result })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        if let Err(error) = layout_worker {
            log::error!("Could not start Hirari project layout worker: {error}");
        }
        Rc::new(Self {
            sender,
            completions,
            layout_sender,
            layout_completions,
            pending: RefCell::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.pending.borrow().is_some()
    }

    pub(crate) fn enqueue(
        &self,
        path: &str,
        tracks: &slint::VecModel<Z_Track>,
        core: &HirariCore,
        step_sequencer_patterns_json: &str,
        fingerprint: u64,
        intent: SaveIntent,
        lease: OperationLease,
    ) -> Result<(), String> {
        self.enqueue_with_completion(
            path,
            tracks,
            core,
            step_sequencer_patterns_json,
            fingerprint,
            intent,
            lease,
            None,
        )
    }

    pub(crate) fn enqueue_with_completion(
        &self,
        path: &str,
        tracks: &slint::VecModel<Z_Track>,
        core: &HirariCore,
        step_sequencer_patterns_json: &str,
        fingerprint: u64,
        intent: SaveIntent,
        lease: OperationLease,
        mut after_completion: Option<Box<dyn FnOnce(bool)>>,
    ) -> Result<(), String> {
        if self.pending.borrow().is_some() {
            if let Some(callback) = after_completion.take() {
                callback(false);
            }
            return Err("another project save is still running".to_owned());
        }
        let Some(audio_input_assignments) = slint_ui::snapshot_audio_input_assignments(tracks)
        else {
            if let Some(callback) = after_completion.take() {
                callback(false);
            }
            return Err("could not capture audio input assignments".to_owned());
        };
        let project_path = std::path::Path::new(path);
        let name = project_path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Hirari Project")
            .to_owned();
        let Some(worker_permit) = crate::ui::worker_budget::WorkerPermit::try_acquire() else {
            if let Some(callback) = after_completion.take() {
                callback(false);
            }
            return Err("background worker budget is busy".to_owned());
        };
        let project_revision = core.project_state_revision();
        let request_id = core.request_project_layout_snapshot();
        if request_id == 0 {
            if let Some(callback) = after_completion.take() {
                callback(false);
            }
            return Err("native project snapshot worker is busy or unavailable".to_owned());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        *self.pending.borrow_mut() = Some(PendingSave {
            id,
            path: path.to_owned(),
            fingerprint,
            intent,
            after_completion,
            native_snapshot: Some(PendingNativeSnapshot {
                request_id,
                name,
                bpm: core.get_tempo(),
                project_revision,
                sample_rate: core.get_sample_rate(),
                aux_track_ids: core.aux_track_ids_json(),
                cancel_requested: false,
                audio_input_assignments,
                step_sequencer_patterns_json: step_sequencer_patterns_json.to_owned(),
                _worker_permit: worker_permit,
            }),
            layout_build: None,
            _lease: lease,
        });
        Ok(())
    }

    /// Polls once per UI tick. Only one job can be active, which also
    /// serializes generation backup rotation for a given project.
    pub(crate) fn poll_completion(&self, core: &HirariCore) -> Option<ProjectSaveCompletion> {
        let snapshot_request = self
            .pending
            .borrow_mut()
            .as_mut()
            .and_then(|pending| pending.native_snapshot.take());
        if let Some(mut snapshot_request) = snapshot_request {
            if !snapshot_request.cancel_requested
                && core.project_state_revision() != snapshot_request.project_revision
                && core.cancel_project_layout_snapshot(snapshot_request.request_id)
            {
                snapshot_request.cancel_requested = true;
                if let Some(pending) = self.pending.borrow_mut().as_mut() {
                    pending.native_snapshot = Some(snapshot_request);
                }
                return None;
            }
            let response = core.poll_project_layout_snapshot(snapshot_request.request_id);
            if response.is_empty() {
                if let Some(pending) = self.pending.borrow_mut().as_mut() {
                    pending.native_snapshot = Some(snapshot_request);
                }
                return None;
            }
            let Some((native_revision, layout_json)) = parse_native_snapshot_response(&response)
            else {
                return self.finish_pending(false);
            };
            let id = self.pending.borrow().as_ref()?.id;
            let layout_job = LayoutBuildJob {
                id,
                name: snapshot_request.name.clone(),
                bpm: snapshot_request.bpm,
                sample_rate: snapshot_request.sample_rate,
                aux_track_ids: snapshot_request.aux_track_ids.clone(),
                layout_json,
                _worker_permit: snapshot_request._worker_permit,
            };
            let pending_layout = PendingLayoutBuild {
                id,
                name: snapshot_request.name,
                bpm: snapshot_request.bpm,
                native_revision,
                project_revision: snapshot_request.project_revision,
                aux_track_ids: snapshot_request.aux_track_ids,
                audio_input_assignments: snapshot_request.audio_input_assignments,
                step_sequencer_patterns_json: snapshot_request.step_sequencer_patterns_json,
            };
            if self.layout_sender.try_send(layout_job).is_err() {
                return self.finish_pending(false);
            }
            if let Some(pending) = self.pending.borrow_mut().as_mut() {
                pending.layout_build = Some(pending_layout);
            }
        }

        let layout_pending = self
            .pending
            .borrow()
            .as_ref()
            .and_then(|pending| pending.layout_build.as_ref())
            .is_some();
        if layout_pending {
            let completion = match self.layout_completions.try_recv() {
                Ok(completion) => completion,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return self.finish_pending(false),
            };
            let pending_layout = self
                .pending
                .borrow_mut()
                .as_mut()
                .and_then(|pending| pending.layout_build.take())?;
            if completion.id != pending_layout.id {
                log::error!("Hirari project layout worker returned an out-of-order completion");
                return self.finish_pending(false);
            }
            let Ok((layout, document_seed)) = completion.result else {
                return self.finish_pending(false);
            };
            let document = core.project_document_snapshot_v2_from_layout_seed(
                &pending_layout.name,
                pending_layout.bpm,
                pending_layout.native_revision,
                pending_layout.project_revision,
                layout,
                pending_layout.aux_track_ids,
                document_seed,
            );
            let Some(document) = document.ok() else {
                return self.finish_pending(false);
            };
            let path = self.pending.borrow().as_ref()?.path.clone();
            let Some(save) = slint_ui::prepare_ui_project_save_from_captured_state(
                &path,
                pending_layout.audio_input_assignments,
                &pending_layout.step_sequencer_patterns_json,
                document,
            ) else {
                return self.finish_pending(false);
            };
            match self.sender.try_send(SaveJob {
                id: pending_layout.id,
                save,
            }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    return self.finish_pending(false);
                }
            }
        }

        let worker_result = match self.completions.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        let mut pending = self.pending.borrow_mut();
        let Some(mut pending_save) = pending.take() else {
            if worker_result.is_some() {
                log::error!("Hirari save worker completed without a pending request");
            }
            return None;
        };
        if worker_result
            .as_ref()
            .is_some_and(|result| pending_save.id != result.id)
        {
            log::error!("Hirari save worker returned an out-of-order completion");
        } else if worker_result.is_none() {
            log::error!("Hirari project save worker disconnected before completing a request");
        }
        Some(ProjectSaveCompletion {
            path: pending_save.path.clone(),
            fingerprint: pending_save.fingerprint,
            intent: pending_save.intent.clone(),
            after_completion: pending_save.after_completion.take(),
            success: worker_result
                .is_some_and(|result| result.id == pending_save.id && result.success),
        })
    }

    fn finish_pending(&self, success: bool) -> Option<ProjectSaveCompletion> {
        let mut pending = self.pending.borrow_mut();
        let pending_save = pending.take()?;
        Some(ProjectSaveCompletion {
            path: pending_save.path,
            fingerprint: pending_save.fingerprint,
            intent: pending_save.intent,
            after_completion: pending_save.after_completion,
            success,
        })
    }
}

fn parse_native_snapshot_response(response: &str) -> Option<(u64, String)> {
    let response = response.strip_prefix("READY ")?;
    let (revision, layout) = response.split_once('\n')?;
    let revision = revision.parse::<u64>().ok()?;
    (!layout.is_empty()).then(|| (revision, layout.to_owned()))
}
