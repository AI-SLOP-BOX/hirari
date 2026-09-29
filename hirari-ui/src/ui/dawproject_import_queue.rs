//! Background import of exchange-format DAWproject files.

use hirari_core_bridge::dawproject::{self, DawProjectImportReport};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;

use super::operation_gate::OperationLease;

struct ImportJob {
    id: u64,
    source: String,
    destination: String,
    sample_rate: f64,
}

struct WorkerCompletion {
    id: u64,
    result: Result<DawProjectImportReport, String>,
}

struct PendingImport {
    id: u64,
    destination: String,
    _lease: OperationLease,
}

pub(crate) struct DawProjectImportCompletion {
    pub(crate) destination: String,
    pub(crate) result: Result<DawProjectImportReport, String>,
    _lease: OperationLease,
}

/// UI-thread coordinator. Archive parsing and media extraction run on one
/// worker; project hydration stays on the UI thread after completion.
pub(crate) struct DawProjectImportQueue {
    sender: SyncSender<ImportJob>,
    completions: Receiver<WorkerCompletion>,
    pending: RefCell<Option<PendingImport>>,
    next_id: AtomicU64,
}

impl DawProjectImportQueue {
    pub(crate) fn start() -> Arc<Self> {
        let (sender, receiver) = mpsc::sync_channel::<ImportJob>(1);
        let (completion_sender, completions) = mpsc::channel::<WorkerCompletion>();
        let worker = std::thread::Builder::new()
            .name("hirari-dawproject-import".to_owned())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let _worker_permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        dawproject::import_dawproject_file(
                            std::path::Path::new(&job.source),
                            std::path::Path::new(&job.destination),
                            job.sample_rate,
                        )
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("DAWproject import worker panicked")))
                    .map_err(|error| format!("{error:#}"));
                    if completion_sender
                        .send(WorkerCompletion { id: job.id, result })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        if let Err(error) = worker {
            log::error!("Could not start DAWproject import worker: {error}");
        }
        Arc::new(Self {
            sender,
            completions,
            pending: RefCell::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    pub(crate) fn enqueue(
        &self,
        source: &str,
        destination: &str,
        sample_rate: f64,
        lease: OperationLease,
    ) -> Result<(), String> {
        if self.pending.borrow().is_some() {
            return Err("another DAWproject import is still running".to_owned());
        }
        if source.trim().is_empty() || destination.trim().is_empty() {
            return Err("DAWproject import path is empty".to_owned());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let job = ImportJob {
            id,
            source: source.to_owned(),
            destination: destination.to_owned(),
            sample_rate,
        };
        match self.sender.try_send(job) {
            Ok(()) => {
                *self.pending.borrow_mut() = Some(PendingImport {
                    id,
                    destination: destination.to_owned(),
                    _lease: lease,
                });
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err("DAWproject import worker is busy".to_owned()),
            Err(TrySendError::Disconnected(_)) => {
                Err("DAWproject import worker is unavailable".to_owned())
            }
        }
    }

    pub(crate) fn poll_completion(&self) -> Option<DawProjectImportCompletion> {
        let worker_result = match self.completions.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                let pending = self.pending.borrow_mut().take()?;
                return Some(DawProjectImportCompletion {
                    destination: pending.destination,
                    result: Err("DAWproject import worker disconnected".to_owned()),
                    _lease: pending._lease,
                });
            }
        };
        let Some(pending) = self.pending.borrow_mut().take() else {
            log::error!("DAWproject import worker completed without a pending request");
            return None;
        };
        if pending.id != worker_result.id {
            log::error!("DAWproject import completion id did not match the pending request");
            return Some(DawProjectImportCompletion {
                destination: pending.destination,
                result: Err("DAWproject import completion did not match the request".to_owned()),
                _lease: pending._lease,
            });
        }
        Some(DawProjectImportCompletion {
            destination: pending.destination,
            result: worker_result.result,
            _lease: pending._lease,
        })
    }
}
