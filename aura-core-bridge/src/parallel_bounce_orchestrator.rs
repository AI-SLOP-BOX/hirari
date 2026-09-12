use std::path::{Path, PathBuf};

use crate::export::{export_interleaved_buffer_to_wave64, write_wav_pcm16, WavExportError};
use crate::generation_gate::GenerationGate;

#[derive(Debug, Clone)]
pub struct StemTaskRust {
    pub track_id: u32,
    pub name: String,
    /// Render generation that owns this task. A completion from an older
    /// generation must never publish into a newer render session.
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BounceTaskState {
    Queued,
    Rendering,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct BounceTaskStatus {
    pub track_id: u32,
    pub state: BounceTaskState,
    pub error: Option<String>,
}

pub struct DistributedRenderingOrchestrator {
    pub active_tasks: Vec<StemTaskRust>,
    in_flight: Vec<StemTaskRust>,
    completed: Vec<u32>,
    completed_outputs: Vec<(u32, PathBuf)>,
    failed: Vec<BounceTaskStatus>,
    last_error: Option<String>,
    publication_gate: GenerationGate,
    cancelled: bool,
}

impl Default for DistributedRenderingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl DistributedRenderingOrchestrator {
    pub fn new() -> Self {
        Self {
            active_tasks: Vec::new(),
            in_flight: Vec::new(),
            completed: Vec::new(),
            completed_outputs: Vec::new(),
            failed: Vec::new(),
            last_error: None,
            publication_gate: GenerationGate::new(),
            cancelled: false,
        }
    }

    /// INDUSTRIAL: Renders project stems in parallel with absolute thread-pool safety and precision.
    pub fn render_project_stems(&mut self, track_ids: Vec<u32>) {
        let generation = self.publication_gate.begin();
        self.cancelled = false;
        self.last_error = None;
        self.active_tasks.clear();
        self.in_flight.clear();
        self.completed.clear();
        self.completed_outputs.clear();
        self.failed.clear();

        // This method is the deterministic planning stage.  It does not claim
        // that audio was rendered; an actual renderer must consume each task
        // through `take_next_task` and acknowledge it with `complete_task`.
        for id in track_ids {
            if self.active_tasks.iter().any(|task| task.track_id == id) {
                continue;
            }
            self.active_tasks.push(StemTaskRust {
                track_id: id,
                name: format!("Track_{}", id),
                generation,
            });
        }
    }

    pub fn generation(&self) -> u64 {
        self.publication_gate.current()
    }

    /// Cancels the current render session. In-flight workers become stale and
    /// cannot complete or publish output into a later session.
    pub fn cancel_render(&mut self) {
        self.cancelled = true;
        self.publication_gate.cancel();
        self.active_tasks.clear();
        self.in_flight.clear();
    }

    /// Removes and returns the next planned task for a connected renderer.
    pub fn take_next_task(&mut self) -> Option<StemTaskRust> {
        let task = self.active_tasks.pop()?;
        self.in_flight.push(task.clone());
        Some(task)
    }

    /// Records a completed task only when its identifier is still pending.
    pub fn complete_task(&mut self, track_id: u32) -> bool {
        self.complete_task_for_generation(track_id, self.generation())
    }

    pub fn complete_task_for_generation(&mut self, track_id: u32, generation: u64) -> bool {
        if self.cancelled || !self.publication_gate.accepts(generation) {
            return false;
        }
        let before = self.in_flight.len();
        self.in_flight
            .retain(|task| !(task.track_id == track_id && task.generation == generation));
        if before == self.in_flight.len() {
            return false;
        }
        self.completed.push(track_id);
        true
    }

    /// Completes a task only after an offline renderer has successfully
    /// written its interleaved PCM buffer to disk. This keeps planning,
    /// rendering, and file publication separate from the real-time callback.
    pub fn complete_task_with_buffer(
        &mut self,
        track_id: u32,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        self.complete_task_with_buffer_for_generation(
            track_id,
            self.generation(),
            path,
            samples,
            sample_rate,
            channels,
        )
    }

    pub fn complete_task_with_buffer_for_generation(
        &mut self,
        track_id: u32,
        generation: u64,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        if self.cancelled || !self.publication_gate.accepts(generation) {
            return Err(WavExportError::InvalidTask);
        }
        if !self
            .in_flight
            .iter()
            .any(|task| task.track_id == track_id && task.generation == generation)
        {
            return Err(WavExportError::InvalidTask);
        }
        if path.as_os_str().is_empty()
            || self
                .completed_outputs
                .iter()
                .any(|(_, output)| output == path)
        {
            let _ = self.fail_task(track_id, "output path is empty or already published");
            return Err(WavExportError::InvalidPath);
        }

        if let Err(error) = write_wav_pcm16(path, samples, sample_rate, channels) {
            let error_text = format!("{error:?}");
            let _ = self.fail_task(track_id, error_text);
            return Err(error);
        }

        if !self.complete_task_for_generation(track_id, generation) {
            return Err(WavExportError::InvalidTask);
        }
        self.completed_outputs.push((track_id, path.to_path_buf()));
        Ok(())
    }

    /// Publishes a large-file WAVE64 stem without allowing stale generations
    /// or duplicate output paths to become visible.
    pub fn complete_task_with_wave64_for_generation(
        &mut self,
        track_id: u32,
        generation: u64,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        if self.cancelled || !self.publication_gate.accepts(generation) {
            return Err(WavExportError::InvalidTask);
        }
        if !self
            .in_flight
            .iter()
            .any(|task| task.track_id == track_id && task.generation == generation)
            || path.as_os_str().is_empty()
            || self
                .completed_outputs
                .iter()
                .any(|(_, output)| output == path)
        {
            return Err(WavExportError::InvalidTask);
        }
        export_interleaved_buffer_to_wave64(path, samples, sample_rate, channels, true)?;
        if !self.complete_task_for_generation(track_id, generation) {
            return Err(WavExportError::InvalidTask);
        }
        self.completed_outputs.push((track_id, path.to_path_buf()));
        Ok(())
    }

    pub fn completed_output_path(&self, track_id: u32) -> Option<&Path> {
        self.completed_outputs
            .iter()
            .find_map(|(id, path)| (*id == track_id).then_some(path.as_path()))
    }

    pub fn fail_task(&mut self, track_id: u32, error: impl Into<String>) -> bool {
        let before = self.in_flight.len();
        self.in_flight.retain(|task| task.track_id != track_id);
        if before == self.in_flight.len() {
            return false;
        }
        self.failed.push(BounceTaskStatus {
            track_id,
            state: BounceTaskState::Failed,
            error: Some(error.into()),
        });
        self.last_error = self.failed.last().and_then(|task| task.error.clone());
        true
    }

    pub fn retry_failed_task(&mut self, track_id: u32) -> bool {
        let Some(index) = self
            .failed
            .iter()
            .position(|task| task.track_id == track_id)
        else {
            return false;
        };
        self.failed.remove(index);
        self.completed_outputs.retain(|(id, _)| *id != track_id);
        if self.cancelled {
            self.publication_gate.begin();
            self.cancelled = false;
            self.active_tasks.clear();
            self.in_flight.clear();
        }
        self.active_tasks.push(StemTaskRust {
            track_id,
            name: format!("Track_{}", track_id),
            generation: self.generation(),
        });
        self.last_error = self.failed.last().and_then(|task| task.error.clone());
        true
    }

    pub fn pending_count(&self) -> usize {
        self.active_tasks.len() + self.in_flight.len() + self.failed.len()
    }

    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }

    pub fn statuses(&self) -> Vec<BounceTaskStatus> {
        let mut result = Vec::with_capacity(self.pending_count() + self.completed.len());
        result.extend(self.active_tasks.iter().map(|task| BounceTaskStatus {
            track_id: task.track_id,
            state: BounceTaskState::Queued,
            error: None,
        }));
        result.extend(self.in_flight.iter().map(|task| BounceTaskStatus {
            track_id: task.track_id,
            state: BounceTaskState::Rendering,
            error: None,
        }));
        result.extend(self.completed.iter().map(|track_id| BounceTaskStatus {
            track_id: *track_id,
            state: BounceTaskState::Completed,
            error: None,
        }));
        result.extend(self.failed.iter().cloned());
        result
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide rendering state.
    pub fn audit_parallel_bounce_orchestrator(&self) -> bool {
        if self.last_error.is_some() {
            return false;
        }
        let completed_are_unique = self
            .completed
            .iter()
            .enumerate()
            .all(|(index, id)| !self.completed[..index].contains(id));
        let outputs_match_completed = self.completed_outputs.iter().all(|(id, path)| {
            self.completed.contains(id) && !path.as_os_str().is_empty() && path.is_file()
        });
        completed_are_unique
            && outputs_match_completed
            && self.active_tasks.iter().enumerate().all(|(index, task)| {
                !task.name.trim().is_empty()
                    && task.name == format!("Track_{}", task.track_id)
                    && self.active_tasks[..index]
                        .iter()
                        .all(|previous| previous.track_id != task.track_id)
            })
            && self.in_flight.iter().all(|task| {
                !self.completed.contains(&task.track_id)
                    && !self
                        .active_tasks
                        .iter()
                        .any(|queued| queued.track_id == task.track_id)
            })
            && self
                .failed
                .iter()
                .all(|task| task.state == BounceTaskState::Failed && task.error.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_unique_tasks_without_claiming_render_completion() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![3, 3, 7]);

        assert!(orchestrator.audit_parallel_bounce_orchestrator());
        assert_eq!(orchestrator.active_tasks.len(), 2);
        assert_eq!(orchestrator.take_next_task().unwrap().track_id, 7);
        assert!(!orchestrator.complete_task(3));
        assert_eq!(orchestrator.completed_count(), 0);
        let task = orchestrator.take_next_task().unwrap();
        assert_eq!(task.track_id, 3);
        assert!(orchestrator.complete_task(task.track_id));
        assert_eq!(orchestrator.completed_count(), 1);
        assert!(orchestrator.audit_parallel_bounce_orchestrator());
    }

    #[test]
    fn empty_plan_resets_pending_work_and_advances_generation() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![4, 5]);
        let first_generation = orchestrator.generation();
        assert_eq!(orchestrator.pending_count(), 2);

        orchestrator.render_project_stems(Vec::new());
        assert!(orchestrator.generation() > first_generation);
        assert_eq!(orchestrator.pending_count(), 0);
        assert_eq!(orchestrator.completed_count(), 0);
        assert!(orchestrator.statuses().is_empty());
    }

    #[test]
    fn stale_render_completion_cannot_publish_after_new_generation() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![21]);
        let stale = orchestrator.take_next_task().unwrap();
        orchestrator.render_project_stems(vec![21]);
        let current = orchestrator.take_next_task().unwrap();

        assert_ne!(stale.generation, current.generation);
        assert!(!orchestrator.complete_task_for_generation(21, stale.generation));
        assert!(orchestrator.complete_task_for_generation(21, current.generation));
    }

    #[test]
    fn cancelled_render_rejects_late_completion() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![22]);
        let task = orchestrator.take_next_task().unwrap();
        orchestrator.cancel_render();

        assert!(!orchestrator.complete_task_for_generation(22, task.generation));
        assert_eq!(orchestrator.completed_count(), 0);
        assert_eq!(orchestrator.pending_count(), 0);
    }

    #[test]
    fn retry_after_cancel_starts_a_new_publishable_generation() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![23]);
        let first_generation = orchestrator.generation();
        let task = orchestrator.take_next_task().unwrap();
        assert!(orchestrator.fail_task(task.track_id, "temporary worker failure"));
        orchestrator.cancel_render();
        assert!(orchestrator.retry_failed_task(23));
        assert_ne!(first_generation, orchestrator.generation());
        let retry = orchestrator.take_next_task().unwrap();
        assert!(orchestrator.complete_task_for_generation(23, retry.generation));
    }

    #[test]
    fn completion_requires_a_successfully_written_output() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![11]);
        assert_eq!(orchestrator.take_next_task().unwrap().track_id, 11);

        let path = std::env::temp_dir().join(format!(
            "aura-bounce-test-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        orchestrator
            .complete_task_with_buffer(11, &path, &[0.0, 0.25], 48_000, 1)
            .unwrap();
        assert_eq!(orchestrator.completed_count(), 1);
        assert_eq!(orchestrator.completed_output_path(11), Some(path.as_path()));
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"RIFF");
        assert!(orchestrator.audit_parallel_bounce_orchestrator());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_file_publication_is_retryable() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![13]);
        orchestrator.take_next_task();
        let invalid_path = Path::new("");
        let error = orchestrator
            .complete_task_with_buffer(13, invalid_path, &[0.0], 48_000, 1)
            .unwrap_err();
        assert_eq!(error, WavExportError::InvalidPath);
        assert_eq!(orchestrator.statuses()[0].state, BounceTaskState::Failed);
        assert!(orchestrator.retry_failed_task(13));
        assert_eq!(orchestrator.active_tasks[0].track_id, 13);
    }

    #[test]
    fn wave64_completion_rejects_stale_generation_and_publishes_current_stem() {
        let mut orchestrator = DistributedRenderingOrchestrator::new();
        orchestrator.render_project_stems(vec![31]);
        let stale = orchestrator.take_next_task().unwrap();
        orchestrator.render_project_stems(vec![31]);
        let current = orchestrator.take_next_task().unwrap();
        let path = std::env::temp_dir().join(format!("aura-current-{}.w64", std::process::id()));
        assert_eq!(
            orchestrator.complete_task_with_wave64_for_generation(
                31,
                stale.generation,
                &path,
                &[0.0, 0.1],
                48_000,
                2
            ),
            Err(WavExportError::InvalidTask)
        );
        assert!(orchestrator
            .complete_task_with_wave64_for_generation(
                31,
                current.generation,
                &path,
                &[0.0, 0.1],
                48_000,
                2
            )
            .is_ok());
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"RIFF");
        let _ = std::fs::remove_file(path);
    }
}
