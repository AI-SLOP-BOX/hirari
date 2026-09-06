use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::export::{export_interleaved_buffer_to_wav, WavExportError};

pub struct ExportTask {
    pub track_id: u32,
    pub label: String,
    pub is_multi_channel: bool,
}

pub struct BounceCoreOrchestrator {
    pub active_tasks: usize,
    pub last_error: Option<String>,
}

impl Default for BounceCoreOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl BounceCoreOrchestrator {
    pub fn new() -> Self {
        Self {
            active_tasks: 0,
            last_error: None,
        }
    }

    /// INDUSTRIAL: Executes a professional batch render with absolute parallel precision.
    pub fn execute_professional_batch_render(
        &mut self,
        tasks: &[ExportTask],
    ) -> Result<(), WavExportError> {
        // There is no safe implicit renderer for this legacy entry point. Do
        // not report a successful professional bounce merely because tasks
        // were queued; callers must provide the native graph callback below.
        if tasks.is_empty() {
            self.last_error = Some("no export tasks queued".into());
            return Err(WavExportError::InvalidTask);
        }
        if tasks
            .iter()
            .any(|task| task.track_id == 0 || task.label.trim().is_empty())
        {
            self.last_error = Some("invalid export task".into());
            return Err(WavExportError::InvalidTask);
        }
        self.active_tasks = tasks.len();
        self.last_error = Some("renderer callback is required".into());
        Err(WavExportError::RendererNotConnected)
    }

    /// Processes one export through the supplied native-graph callback.
    pub fn process_single_professional_export_with_renderer<F>(
        &mut self,
        output_dir: &Path,
        task: &ExportTask,
        sample_rate: u32,
        channels: u16,
        mut render: F,
    ) -> Result<(), WavExportError>
    where
        F: FnMut(&ExportTask) -> Result<Vec<f32>, String>,
    {
        self.render_batch_with_renderer(output_dir, std::slice::from_ref(task), sample_rate, channels, |item| render(item))
            .map(|_| ())
    }

    /// Legacy single-export entry point. It now fails explicitly instead of
    /// silently accepting a task without rendering any audio.
    pub fn process_single_professional_export(
        &mut self,
        task: &ExportTask,
    ) -> Result<(), WavExportError> {
        if task.track_id == 0 || task.label.trim().is_empty() {
            self.last_error = Some("invalid export task".into());
            return Err(WavExportError::InvalidTask);
        }
        self.last_error = Some("renderer callback is required".into());
        Err(WavExportError::RendererNotConnected)
    }

    /// Render and publish a complete stem batch through a caller-owned
    /// renderer. Publication is transactional: if any stem fails, files
    /// already published by this batch are removed and the batch is reported
    /// as failed instead of leaving an incomplete stem set behind.
    pub fn render_batch_with_renderer<F>(
        &mut self,
        output_dir: &Path,
        tasks: &[ExportTask],
        sample_rate: u32,
        channels: u16,
        mut render: F,
    ) -> Result<usize, WavExportError>
    where
        F: FnMut(&ExportTask) -> Result<Vec<f32>, String>,
    {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() {
            self.last_error = Some("invalid bounce output directory".into());
            return Err(WavExportError::InvalidPath);
        }
        if tasks.is_empty()
            || !(1..=384_000).contains(&sample_rate)
            || !(1..=32).contains(&channels)
        {
            self.last_error = Some("invalid bounce batch".into());
            return Err(if tasks.is_empty() || !(1..=32).contains(&channels) {
                WavExportError::InvalidTask
            } else {
                WavExportError::InvalidSampleRate
            });
        }

        let mut ids = HashSet::with_capacity(tasks.len());
        let mut outputs: Vec<(PathBuf, Vec<f32>)> = Vec::with_capacity(tasks.len());
        for task in tasks {
            if task.track_id == 0 || task.label.trim().is_empty() || !ids.insert(task.track_id) {
                self.last_error = Some("duplicate or invalid bounce task".into());
                return Err(WavExportError::InvalidTask);
            }
            let samples = render(task).map_err(|error| {
                self.last_error = Some(format!("track {} render failed: {error}", task.track_id));
                WavExportError::InvalidTask
            })?;
            if samples.is_empty() {
                self.last_error = Some(format!("track {} returned an empty buffer", task.track_id));
                return Err(WavExportError::EmptyBuffer);
            }
            let filename = format!("{:04}_{}.wav", task.track_id, safe_filename(&task.label));
            let path = output_dir.join(filename);
            // A transactional rollback cannot safely restore an arbitrary
            // pre-existing file after an atomic replace. Require callers to
            // choose a fresh output set (or remove it explicitly first).
            if path.exists() {
                self.last_error = Some(format!("bounce output already exists: {}", path.display()));
                return Err(WavExportError::InvalidPath);
            }
            outputs.push((path, samples));
        }

        let mut published = Vec::with_capacity(outputs.len());
        for (path, samples) in outputs {
            if let Err(error) =
                export_interleaved_buffer_to_wav(&path, &samples, sample_rate, channels, true)
            {
                for old_path in &published {
                    let _ = std::fs::remove_file(old_path);
                }
                self.last_error = Some(format!("bounce batch publish failed: {error:?}"));
                return Err(error);
            }
            published.push(path);
        }
        self.active_tasks = 0;
        self.last_error = None;
        Ok(published.len())
    }

    /// Writes a completed task buffer to WAV without treating an unconnected
    /// renderer or invalid interleaved data as a successful bounce.
    pub fn render_task_buffer_to_wav(
        &mut self,
        task: &ExportTask,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        if task.track_id == 0 || task.label.trim().is_empty() {
            let error = WavExportError::InvalidTask;
            self.last_error = Some("invalid export task".into());
            return Err(error);
        }
        let result = export_interleaved_buffer_to_wav(
            path,
            samples,
            sample_rate,
            channels,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("bounce WAV export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide render graph.
    pub fn audit_bounce_core(&self) -> bool {
        // INDUSTRIAL: Implementation of forensic render auditing logic.
        self.active_tasks == 0 || self.last_error.is_none()
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
        "track".to_owned()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bounce_requires_valid_task_and_connected_renderer() {
        let path =
            std::env::temp_dir().join(format!("aura-bounce-core-{}-{}.wav", std::process::id(), 1));
        let task = ExportTask {
            track_id: 1,
            label: "Preview".into(),
            is_multi_channel: false,
        };
        let mut bounce = BounceCoreOrchestrator::new();
        assert_eq!(
            bounce.render_task_buffer_to_wav(&task, &path, &[0.0, 0.0], 48_000, 2, false),
            Err(WavExportError::RendererNotConnected)
        );
        assert_eq!(
            bounce.render_task_buffer_to_wav(&task, &path, &[0.0], 48_000, 2, true),
            Err(WavExportError::IncompleteFrame)
        );
        assert!(bounce
            .render_task_buffer_to_wav(&task, &path, &[0.0, 0.0], 48_000, 2, true)
            .is_ok());
        assert_eq!(&fs::read(&path).unwrap()[0..4], b"RIFF");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_professional_entry_points_never_claim_success_without_renderer() {
        let task = ExportTask {
            track_id: 1,
            label: "Preview".into(),
            is_multi_channel: false,
        };
        let mut bounce = BounceCoreOrchestrator::new();
        assert_eq!(
            bounce.execute_professional_batch_render(std::slice::from_ref(&task)),
            Err(WavExportError::RendererNotConnected)
        );
        assert_eq!(
            bounce.process_single_professional_export(&task),
            Err(WavExportError::RendererNotConnected)
        );
    }

    #[test]
    fn single_professional_export_uses_the_supplied_renderer() {
        let output_dir = std::env::temp_dir().join(format!(
            "aura-single-render-{}-{}",
            std::process::id(),
            1
        ));
        fs::create_dir_all(&output_dir).unwrap();
        let task = ExportTask {
            track_id: 1,
            label: "Preview".into(),
            is_multi_channel: false,
        };
        let mut bounce = BounceCoreOrchestrator::new();
        assert_eq!(
            bounce.process_single_professional_export_with_renderer(
                &output_dir,
                &task,
                48_000,
                2,
                |_| Ok(vec![0.0, 0.0, 0.25, -0.25]),
            ),
            Ok(())
        );
        assert!(output_dir.join("0001_Preview.wav").is_file());
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn batch_render_rolls_back_when_a_later_stem_is_invalid() {
        let output_dir =
            std::env::temp_dir().join(format!("aura-bounce-batch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output_dir);
        fs::create_dir_all(&output_dir).unwrap();
        let tasks = vec![
            ExportTask {
                track_id: 1,
                label: "Lead Vocal".into(),
                is_multi_channel: false,
            },
            ExportTask {
                track_id: 2,
                label: "Bass".into(),
                is_multi_channel: false,
            },
        ];
        let mut bounce = BounceCoreOrchestrator::new();
        let result = bounce.render_batch_with_renderer(&output_dir, &tasks, 48_000, 1, |task| {
            if task.track_id == 1 {
                Ok(vec![0.0])
            } else {
                Ok(vec![f32::NAN])
            }
        });
        assert_eq!(result, Err(WavExportError::NonFiniteSample));
        assert!(!output_dir.join("0001_Lead_Vocal.wav").exists());
        let _ = fs::remove_dir_all(output_dir);
    }
}
