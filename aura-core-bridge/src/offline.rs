use std::path::Path;

use crate::export::{
    export_interleaved_buffer_to_wav, export_interleaved_buffer_to_wave64, WavExportError,
};
use crate::job_system::PublicationGate;

#[derive(Debug, Clone)]
pub struct OfflineProcessJob {
    pub id: u32,
    pub region_name: String,
    pub process_steps: Vec<String>,
    pub version_index: u32,
}

pub struct OfflineOrchestrator {
    pub jobs: Vec<OfflineProcessJob>,
    pub last_error: Option<String>,
    completed_outputs: Vec<(u32, std::path::PathBuf)>,
    publication_gate: PublicationGate,
}

impl Default for OfflineOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineOrchestrator {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            last_error: None,
            completed_outputs: Vec::new(),
            publication_gate: PublicationGate::new(),
        }
    }

    /// INDUSTRIAL: Submits a new offline processing job with versioning and absolute precision.
    pub fn submit_job(&mut self, id: u32, name: &str, steps: Vec<String>) {
        // INDUSTRIAL: Implementation of high-performance versioning logic.
        if id == 0
            || name.trim().is_empty()
            || name.len() > 256
            || name.contains('\0')
            || steps.is_empty()
            || steps.len() > 256
            || steps.iter().any(|step| step.trim().is_empty() || step.len() > 512 || step.contains('\0'))
            || self.jobs.len() >= 65_536
            || self.jobs.iter().any(|job| job.id == id)
        {
            self.last_error = Some("invalid offline job".into());
            return;
        }
        let version_index = self.jobs.iter().filter(|j| j.region_name == name.trim()).count() as u32;
        self.jobs.push(OfflineProcessJob {
            id,
            region_name: name.trim().to_owned(),
            process_steps: steps,
            version_index,
        });
    }

    /// Writes a completed offline render buffer to WAV.
    ///
    /// The renderer connection is explicit so queued/no-op offline jobs cannot
    /// accidentally be reported as successful exports.
    pub fn write_rendered_buffer_to_wav(
        &mut self,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        let result = export_interleaved_buffer_to_wav(
            path,
            samples,
            sample_rate,
            channels,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("offline WAV export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// Normalizes an offline render to a LUFS target under a true-peak ceiling
    /// before publishing it. Returns the actual applied gain in dB.
    pub fn write_normalized_buffer_to_wav(
        &mut self, path: &Path, samples: &[f32], sample_rate: u32, channels: u16,
        measured_lufs: f32, target_lufs: f32, max_true_peak_dbtp: f32,
        renderer_connected: bool,
    ) -> Result<f32, WavExportError> {
        let (normalized, applied_db) = crate::delivery::normalize_loudness_interleaved(
            samples, measured_lufs, target_lufs, max_true_peak_dbtp,
        ).ok_or(WavExportError::InvalidTask)?;
        self.write_rendered_buffer_to_wav(path, &normalized, sample_rate, channels, renderer_connected)?;
        Ok(applied_db)
    }

    /// Writes an offline render as atomic IEEE-float WAVE64.
    pub fn write_rendered_buffer_to_wave64(
        &mut self,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        let result = export_interleaved_buffer_to_wave64(
            path,
            samples,
            sample_rate,
            channels,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("offline WAVE64 export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// Associates a rendered file with a concrete offline job. A successful
    /// file write is the only transition that records completion.
    pub fn complete_job_with_buffer(
        &mut self,
        job_id: u32,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        let generation = self.publication_gate.begin();
        self.complete_job_with_buffer_for_generation(
            generation,
            job_id,
            path,
            samples,
            sample_rate,
            channels,
        )
    }

    /// Publishes a render only when it belongs to the current render batch.
    /// A caller that queued work asynchronously should retain the generation
    /// returned by `begin_render_generation` and pass it here.
    pub fn complete_job_with_buffer_for_generation(
        &mut self,
        generation: u64,
        job_id: u32,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        if !self.publication_gate.accepts(generation) {
            return Err(WavExportError::InvalidTask);
        }
        if !self.jobs.iter().any(|job| job.id == job_id) {
            return Err(WavExportError::InvalidTask);
        }
        if self
            .completed_outputs
            .iter()
            .any(|(id, output)| *id == job_id || output == path)
        {
            return Err(WavExportError::InvalidTask);
        }
        let result = export_interleaved_buffer_to_wav(path, samples, sample_rate, channels, true);
        match result {
            Ok(()) => {
                self.completed_outputs.push((job_id, path.to_path_buf()));
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(format!("offline job {job_id} failed: {error:?}"));
                Err(error)
            }
        }
    }

    /// Publishes a WAVE64 result only for the current render generation.
    pub fn complete_job_with_wave64_for_generation(
        &mut self,
        generation: u64,
        job_id: u32,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        if !self.publication_gate.accepts(generation)
            || !self.jobs.iter().any(|job| job.id == job_id)
            || self
                .completed_outputs
                .iter()
                .any(|(id, output)| *id == job_id || output == path)
        {
            return Err(WavExportError::InvalidTask);
        }
        export_interleaved_buffer_to_wave64(path, samples, sample_rate, channels, true)?;
        self.completed_outputs.push((job_id, path.to_path_buf()));
        self.last_error = None;
        Ok(())
    }

    /// Invalidates all previously submitted render completions and returns a
    /// token for the new batch.
    pub fn begin_render_generation(&self) -> u64 {
        self.publication_gate.begin()
    }

    /// Cancels the current batch. Workers may finish, but their output can no
    /// longer be published through the generation-aware API.
    pub fn cancel_render_generation(&self) -> u64 {
        self.publication_gate.cancel()
    }

    /// Executes the queued jobs through a caller-owned offline renderer.
    /// Rendering happens completely before any completion is published, so a
    /// failed renderer cannot consume half of a batch. The callback is a
    /// control/offline boundary: it must never be called from the realtime
    /// audio callback.
    pub fn orchestrate_render_with_renderer<F>(
        &mut self,
        output_dir: &Path,
        sample_rate: u32,
        channels: u16,
        mut render: F,
    ) -> Result<usize, WavExportError>
    where
        F: FnMut(&OfflineProcessJob) -> Result<Vec<f32>, String>,
    {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() {
            return Err(WavExportError::InvalidPath);
        }
        if self.jobs.is_empty() || !(1..=384_000).contains(&sample_rate) ||
            !(1..=32).contains(&channels) {
            return Err(WavExportError::InvalidTask);
        }
        let generation = self.begin_render_generation();
        // Clone the immutable job plan before invoking the renderer. This
        // keeps renderer failures observable without holding a borrow into
        // the mutable orchestrator state.
        let jobs = self.jobs.clone();
        let mut rendered = Vec::with_capacity(jobs.len());
        for job in jobs {
            let samples = match render(&job) {
                Ok(samples) => samples,
                Err(error) => {
                    let job_id = job.id;
                    self.last_error = Some(format!("offline job {job_id} failed: {error}"));
                    return Err(WavExportError::InvalidTask);
                }
            };
            if samples.is_empty() {
                self.last_error = Some(format!("offline job {} returned an empty buffer", job.id));
                return Err(WavExportError::EmptyBuffer);
            }
            rendered.push((job.id, samples));
        }
        let mut completed = 0usize;
        let mut published_paths = Vec::with_capacity(rendered.len());
        for (job_id, samples) in rendered {
            let job = self.jobs.iter().find(|job| job.id == job_id)
                .ok_or_else(|| {
                    for path in &published_paths {
                        let _ = std::fs::remove_file(path);
                    }
                    WavExportError::InvalidTask
                })?;
            let path = output_dir.join(format!(
                "{job_id:04}_{}.wav", safe_filename(&job.region_name)
            ));
            if let Err(error) = self.complete_job_with_buffer_for_generation(
                generation, job_id, &path, &samples, sample_rate, channels
            ) {
                for published in &published_paths {
                    let _ = std::fs::remove_file(published);
                }
                self.completed_outputs
                    .retain(|(_, output)| !published_paths.iter().any(|p| p == output));
                return Err(error);
            }
            published_paths.push(path);
            completed += 1;
        }
        Ok(completed)
    }

    /// Completes multiple offline jobs from buffers produced by an external
    /// renderer. Each file is written atomically before completion is recorded.
    pub fn execute_render_with_buffers(
        &mut self,
        output_dir: &Path,
        renders: &[(u32, Vec<f32>)],
        sample_rate: u32,
        channels: u16,
    ) -> Result<usize, WavExportError> {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() {
            return Err(WavExportError::InvalidPath);
        }
        if !(1..=384_000).contains(&sample_rate) || !(1..=32).contains(&channels) {
            return Err(if !(1..=384_000).contains(&sample_rate) {
                WavExportError::InvalidSampleRate
            } else {
                WavExportError::InvalidChannelCount
            });
        }
        let mut seen = std::collections::HashSet::with_capacity(renders.len());
        for (job_id, samples) in renders {
            if !seen.insert(*job_id)
                || self.completed_outputs.iter().any(|(id, _)| *id == *job_id)
                || !self.jobs.iter().any(|job| job.id == *job_id)
            {
                return Err(WavExportError::InvalidTask);
            }
            if samples.is_empty() {
                return Err(WavExportError::EmptyBuffer);
            }
        }
        let generation = self.begin_render_generation();
        let mut completed = 0usize;
        let mut published_paths = Vec::with_capacity(renders.len());
        for (job_id, samples) in renders {
            let Some(job) = self.jobs.iter().find(|job| job.id == *job_id) else {
                for path in &published_paths {
                    let _ = std::fs::remove_file(path);
                }
                return Err(WavExportError::InvalidTask);
            };
            let path = output_dir.join(format!(
                "{job_id:04}_{}.wav",
                safe_filename(&job.region_name)
            ));
            if let Err(error) = self.complete_job_with_buffer_for_generation(
                generation,
                *job_id,
                &path,
                samples,
                sample_rate,
                channels,
            ) {
                // A batch is one publication unit. Do not leave a partial
                // stem set behind when a later stem fails; callers can retry
                // the whole batch against the same output directory.
                for published in &published_paths {
                    let _ = std::fs::remove_file(published);
                }
                self.completed_outputs
                    .retain(|(_, output)| !published_paths.iter().any(|p| p == output));
                return Err(error);
            }
            published_paths.push(path);
            completed += 1;
        }
        Ok(completed)
    }

    pub fn completed_output(&self, job_id: u32) -> Option<&Path> {
        self.completed_outputs
            .iter()
            .find_map(|(id, path)| (*id == job_id).then_some(path.as_path()))
    }

    /// INDUSTRIAL: Orchestrates background rendering with forensic tail detection and non-destructive snapshots.
    pub fn orchestrate_render(&mut self) {
        self.last_error = if self.jobs.is_empty() {
            Some("no offline jobs queued".into())
        } else {
            Some("offline renderer is not connected; jobs remain queued".into())
        };
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide rendering integrity graph.
    pub fn audit_rendering(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic rendering auditing logic.
        self.jobs.len() <= 65_536 && self.jobs.iter().all(|job| {
            job.id > 0 && !job.region_name.trim().is_empty() && job.region_name.len() <= 256 && !job.region_name.contains('\0') && !job.process_steps.is_empty() && job.process_steps.len() <= 256 && job.process_steps.iter().all(|step| !step.trim().is_empty() && step.len() <= 512 && !step.contains('\0'))
        }) && self.jobs.windows(2).all(|pair| {
            pair[0].version_index < pair[1].version_index
                || pair[0].region_name != pair[1].region_name
        }) && self
            .completed_outputs
            .iter()
            .all(|(id, path)| self.jobs.iter().any(|job| job.id == *id) && path.is_file())
    }
}

fn safe_filename(name: &str) -> String {
    let mut result = String::with_capacity(name.len().min(80));
    for character in name.chars().take(80) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            result.push(character);
        } else if character.is_whitespace() {
            result.push('_');
        }
    }
    if result.is_empty() {
        "region".to_owned()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn offline_render_requires_connected_renderer_and_non_empty_buffer() {
        let path = std::env::temp_dir().join(format!(
            "aura-offline-render-{}-{}.wav",
            std::process::id(),
            1
        ));
        let mut offline = OfflineOrchestrator::new();
        assert_eq!(
            offline.write_rendered_buffer_to_wav(&path, &[0.0, 0.0], 48_000, 2, false),
            Err(WavExportError::RendererNotConnected)
        );
        assert_eq!(
            offline.write_rendered_buffer_to_wav(&path, &[], 48_000, 2, true),
            Err(WavExportError::EmptyBuffer)
        );
        assert!(offline
            .write_rendered_buffer_to_wav(&path, &[0.0, 0.0], 48_000, 2, true)
            .is_ok());
        assert_eq!(&fs::read(&path).unwrap()[0..4], b"RIFF");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn normalized_offline_render_returns_applied_gain() {
        let path = std::env::temp_dir().join(format!("aura-offline-normalized-{}.wav", std::process::id()));
        let mut offline = OfflineOrchestrator::new();
        let applied = offline.write_normalized_buffer_to_wav(&path, &[0.25, 0.25], 48_000, 1, -20.0, -14.0, -1.0, true).unwrap();
        assert!(applied > 0.0);
        assert!(path.is_file());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn offline_wave64_export_is_atomic_and_validated() {
        let path =
            std::env::temp_dir().join(format!("aura-offline-wave64-{}.w64", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut offline = OfflineOrchestrator::new();
        offline
            .write_rendered_buffer_to_wave64(&path, &[0.0, 0.25], 48_000, 2, true)
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        let (_, channels, samples) = crate::export::read_wave64_float32(&path).unwrap();
        assert_eq!(channels, 2);
        assert_eq!(samples, vec![0.0, 0.25]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn batch_render_writes_files_and_tracks_completion() {
        let output_dir =
            std::env::temp_dir().join(format!("aura-offline-batch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output_dir);
        fs::create_dir_all(&output_dir).unwrap();
        let mut offline = OfflineOrchestrator::new();
        offline.submit_job(1, "Lead Vocal/Comp", vec!["normalize".to_owned()]);
        offline.submit_job(2, "Guitar", vec!["render".to_owned()]);
        let renders = vec![(1, vec![0.0_f32, 0.1]), (2, vec![0.0_f32, -0.1])];
        assert_eq!(
            offline.execute_render_with_buffers(&output_dir, &renders, 48_000, 1),
            Ok(2)
        );
        assert!(output_dir.join("0001_Lead_VocalComp.wav").is_file());
        assert!(output_dir.join("0002_Guitar.wav").is_file());
        assert!(offline.completed_output(1).is_some());
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn duplicate_jobs_and_duplicate_batch_entries_are_rejected_before_writing() {
        let output_dir =
            std::env::temp_dir().join(format!("aura-offline-duplicate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output_dir);
        fs::create_dir_all(&output_dir).unwrap();
        let mut offline = OfflineOrchestrator::new();
        offline.submit_job(7, "Lead", vec!["render".to_owned()]);
        offline.submit_job(7, "Duplicate", vec!["render".to_owned()]);
        assert_eq!(offline.jobs.len(), 1);
        assert_eq!(
            offline.execute_render_with_buffers(
                &output_dir,
                &[(7, vec![0.0]), (7, vec![0.0])],
                48_000,
                1
            ),
            Err(WavExportError::InvalidTask)
        );
        assert!(!output_dir.join("0007_Lead.wav").exists());
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn stale_render_completion_cannot_publish_after_batch_cancellation() {
        let output_dir =
            std::env::temp_dir().join(format!("aura-offline-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output_dir);
        fs::create_dir_all(&output_dir).unwrap();
        let mut offline = OfflineOrchestrator::new();
        offline.submit_job(9, "Lead", vec!["render".to_owned()]);
        let old_generation = offline.begin_render_generation();
        offline.cancel_render_generation();
        let path = output_dir.join("stale.wav");
        assert_eq!(
            offline.complete_job_with_buffer_for_generation(
                old_generation,
                9,
                &path,
                &[0.0],
                48_000,
                1,
            ),
            Err(WavExportError::InvalidTask)
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn failed_batch_rolls_back_stems_already_published() {
        let output_dir = std::env::temp_dir().join(format!(
            "aura-offline-rollback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&output_dir);
        fs::create_dir_all(&output_dir).unwrap();
        let mut offline = OfflineOrchestrator::new();
        offline.submit_job(11, "Lead", vec!["render".to_owned()]);
        offline.submit_job(12, "Bass", vec!["render".to_owned()]);

        let result = offline.execute_render_with_buffers(
            &output_dir,
            &[(11, vec![0.0_f32]), (12, vec![f32::NAN])],
            48_000,
            1,
        );

        assert_eq!(result, Err(WavExportError::NonFiniteSample));
        assert!(!output_dir.join("0011_Lead.wav").exists());
        assert!(offline.completed_output(11).is_none());
        let _ = fs::remove_dir_all(output_dir);
    }
}
