use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Submitted,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub state: JobState,
    /// Normalized completion fraction exposed to CLI/API clients.
    pub progress: f32,
    pub error_code: Option<String>,
}

impl JobRecord {
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            state: JobState::Submitted,
            progress: 0.0,
            error_code: None,
        }
    }
    pub fn transition(
        &mut self,
        next: JobState,
        error_code: Option<String>,
    ) -> Result<(), &'static str> {
        let allowed = matches!(
            (self.state, next),
            (JobState::Submitted, JobState::Running)
                | (JobState::Submitted, JobState::Cancelled)
                | (JobState::Running, JobState::Completed)
                | (JobState::Running, JobState::Failed)
                | (JobState::Running, JobState::Cancelled)
        );
        if !allowed {
            return Err("invalid job state transition");
        }
        self.state = next;
        if matches!(next, JobState::Completed) {
            self.progress = 1.0;
        }
        self.error_code = error_code;
        Ok(())
    }
}

/// Shared publication gate for asynchronous render/export jobs. Keeping this
/// alias on the canonical generation gate prevents waveform, offline render,
/// and job-system completions from acquiring subtly different invalidation
/// semantics.
pub use crate::generation_gate::GenerationGate as PublicationGate;

/// Thread-safe control-plane registry for background jobs. Workers only need
/// to hold a clone of the registry and publish explicit lifecycle transitions;
/// audio processing never touches this lock.
#[derive(Clone, Default)]
pub struct JobRegistry {
    jobs: Arc<Mutex<HashMap<String, JobRecord>>>,
}

impl JobRegistry {
    pub fn submit(
        &self,
        id: impl Into<String>,
        kind: impl Into<String>,
    ) -> Result<JobRecord, &'static str> {
        let job = JobRecord::new(id, kind);
        let mut jobs = self.jobs.lock().map_err(|_| "job registry poisoned")?;
        if jobs.contains_key(&job.id) {
            return Err("job id already exists");
        }
        jobs.insert(job.id.clone(), job.clone());
        Ok(job)
    }
    pub fn transition(
        &self,
        id: &str,
        next: JobState,
        error_code: Option<String>,
    ) -> Result<JobRecord, &'static str> {
        let mut jobs = self.jobs.lock().map_err(|_| "job registry poisoned")?;
        let job = jobs.get_mut(id).ok_or("job not found")?;
        job.transition(next, error_code)?;
        Ok(job.clone())
    }
    pub fn get(&self, id: &str) -> Result<Option<JobRecord>, &'static str> {
        Ok(self
            .jobs
            .lock()
            .map_err(|_| "job registry poisoned")?
            .get(id)
            .cloned())
    }

    /// Publish worker progress without allowing terminal jobs to move back
    /// into an in-flight state. Values are deliberately normalized here so
    /// every transport reports the same range.
    pub fn set_progress(&self, id: &str, progress: f32) -> Result<JobRecord, &'static str> {
        if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
            return Err("job progress must be finite and between 0 and 1");
        }
        let mut jobs = self.jobs.lock().map_err(|_| "job registry poisoned")?;
        let job = jobs.get_mut(id).ok_or("job not found")?;
        if matches!(
            job.state,
            JobState::Completed | JobState::Failed | JobState::Cancelled
        ) {
            return Err("cannot update terminal job");
        }
        if progress < job.progress {
            return Err("job progress cannot move backwards");
        }
        job.progress = progress;
        Ok(job.clone())
    }

    /// Return a stable snapshot for polling clients. Sorting by id keeps
    /// responses deterministic across HashMap iteration orders.
    pub fn list(&self) -> Result<Vec<JobRecord>, &'static str> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| "job registry poisoned")?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        jobs.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(jobs)
    }
    pub fn cancel(&self, id: &str) -> Result<JobRecord, &'static str> {
        self.transition(id, JobState::Cancelled, Some("cancelled_by_client".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_lifecycle_is_explicit_and_terminal() {
        let mut job = JobRecord::new("job-1", "stem_export");
        job.transition(JobState::Running, None).unwrap();
        job.transition(JobState::Failed, Some("provider_timeout".into()))
            .unwrap();
        assert_eq!(job.state, JobState::Failed);
        assert!(job.transition(JobState::Running, None).is_err());
    }

    #[test]
    fn submitted_job_can_be_cancelled_before_worker_start() {
        let mut job = JobRecord::new("job-2", "render");
        job.transition(JobState::Cancelled, Some("superseded".into()))
            .unwrap();
        assert_eq!(job.state, JobState::Cancelled);
    }

    #[test]
    fn stale_publication_token_is_rejected_after_cancel() {
        let gate = PublicationGate::new();
        let old = gate.begin();
        assert!(gate.accepts(old));
        gate.cancel();
        assert!(!gate.accepts(old));
    }

    #[test]
    fn registry_submits_transitions_reads_and_cancels_jobs() {
        let registry = JobRegistry::default();
        registry.submit("job-3", "headless_render").unwrap();
        registry
            .transition("job-3", JobState::Running, None)
            .unwrap();
        registry.set_progress("job-3", 0.5).unwrap();
        assert_eq!(
            registry.get("job-3").unwrap().unwrap().state,
            JobState::Running
        );
        assert_eq!(registry.get("job-3").unwrap().unwrap().progress, 0.5);
        registry.cancel("job-3").unwrap();
        assert_eq!(
            registry.get("job-3").unwrap().unwrap().state,
            JobState::Cancelled
        );
        assert_eq!(registry.list().unwrap().len(), 1);
        assert!(registry.submit("job-3", "duplicate").is_err());
    }

    #[test]
    fn progress_is_monotonic_and_terminal_jobs_are_immutable() {
        let registry = JobRegistry::default();
        registry.submit("job-progress", "render").unwrap();
        registry
            .transition("job-progress", JobState::Running, None)
            .unwrap();
        assert!(registry.set_progress("job-progress", 0.7).is_ok());
        assert!(registry.set_progress("job-progress", 0.6).is_err());
        registry
            .transition("job-progress", JobState::Completed, None)
            .unwrap();
        assert_eq!(registry.get("job-progress").unwrap().unwrap().progress, 1.0);
        assert!(registry.set_progress("job-progress", 1.0).is_err());
        assert!(registry.set_progress("job-progress", f32::NAN).is_err());
    }
}
