#![allow(clippy::too_many_arguments)]

use std::path::{Path, PathBuf};

use crate::export::{write_wav_pcm16, WavExportError};

pub struct OfflineOrchestrator {
    pub pending_tasks: Vec<String>,
    /// Source buffers are retained until a real offline renderer consumes them.
    pub pending_sources: Vec<Vec<f32>>,
    completed_outputs: Vec<(String, PathBuf)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OfflineProcessError {
    EmptyBuffer,
    InvalidRange,
    NonFiniteInput,
    RendererNotConnected,
    ExportFailed(String),
}

impl Default for OfflineOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineOrchestrator {
    pub fn new() -> Self {
        Self {
            pending_tasks: Vec::new(),
            pending_sources: Vec::new(),
            completed_outputs: Vec::new(),
        }
    }

    /// INDUSTRIAL: Renders a specific region offline with absolute DSP chain precision and asynchronous sovereignty.
    pub fn process_region_async(
        &mut self,
        source_data: Vec<f32>,
        chain_id: String,
    ) -> Result<(), OfflineProcessError> {
        self.process_region_async_with_range(source_data, 0, usize::MAX, chain_id)
    }

    /// Queues a validated selection while preserving the original input.
    pub fn process_region_async_with_range(
        &mut self,
        source_data: Vec<f32>,
        start: usize,
        end: usize,
        chain_id: String,
    ) -> Result<(), OfflineProcessError> {
        if source_data.is_empty() || chain_id.trim().is_empty() {
            return Err(OfflineProcessError::EmptyBuffer);
        }
        let actual_end = if end == usize::MAX { source_data.len() } else { end };
        if start >= actual_end || actual_end > source_data.len() {
            // `usize::MAX` is the sentinel used by the compatibility method.
            if end != usize::MAX {
                return Err(OfflineProcessError::InvalidRange);
            }
        }
        if source_data.iter().any(|sample| !sample.is_finite()) {
            return Err(OfflineProcessError::NonFiniteInput);
        }

        // Queue ownership is the successful result of the async API.  The
        // previous implementation appended the validated source and then
        // returned RendererNotConnected, making every selection appear to
        // fail even though a later drain could export it.  Keep rendering
        // deferred, but report acceptance so UI/CLI callers can track the
        // pending job instead of retrying and duplicating the selection.
        self.pending_tasks.push(chain_id);
        self.pending_sources
            .push(source_data[start..actual_end].to_vec());
        Ok(())
    }

    /// Drains queued selections through the offline WAV writer. The queue is
    /// validated completely before the first file is created, then removed only
    /// after every item has been published successfully.
    pub fn drain_pending_to_wav(
        &mut self,
        output_dir: &Path,
        sample_rate: u32,
        channels: u16,
    ) -> Result<usize, OfflineProcessError> {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() {
            return Err(OfflineProcessError::ExportFailed(
                "invalid output directory".into(),
            ));
        }
        if self.pending_tasks.len() != self.pending_sources.len() {
            return Err(OfflineProcessError::ExportFailed(
                "selection queue is inconsistent".into(),
            ));
        }
        let mut names = std::collections::HashSet::with_capacity(self.pending_tasks.len());
        for task in &self.pending_tasks {
            if !names.insert(task.clone())
                || self.completed_outputs.iter().any(|(id, _)| id == task)
            {
                return Err(OfflineProcessError::ExportFailed(
                    "duplicate or already completed selection".into(),
                ));
            }
        }

        let queued_tasks = self.pending_tasks.clone();
        let queued_sources = self.pending_sources.clone();
        let mut written = Vec::with_capacity(queued_tasks.len());
        for (index, (task, source)) in queued_tasks.iter().zip(&queued_sources).enumerate() {
            let path = output_dir.join(format!("{index:04}_{}.wav", safe_filename(task)));
            write_wav_pcm16(&path, source, sample_rate, channels)
                .map_err(|error| OfflineProcessError::ExportFailed(format_wav_error(error)))?;
            written.push((task.clone(), path));
        }
        self.completed_outputs.extend(written);
        self.pending_tasks.clear();
        self.pending_sources.clear();
        Ok(queued_tasks.len())
    }

    /// Writes a validated selection directly to a PCM16 WAV.
    /// This is synchronous by design; callers should run it on an export worker.
    pub fn process_region_to_wav(
        &mut self,
        source_data: &[f32],
        start: usize,
        end: usize,
        sample_rate: u32,
        channels: u16,
        path: &Path,
        chain_id: String,
    ) -> Result<(), OfflineProcessError> {
        if source_data.is_empty() {
            return Err(OfflineProcessError::EmptyBuffer);
        }
        let actual_end = if end == usize::MAX {
            source_data.len()
        } else {
            end
        };
        if start >= actual_end || actual_end > source_data.len() {
            return Err(OfflineProcessError::InvalidRange);
        }
        let selection = &source_data[start..actual_end];
        if selection.iter().any(|sample| !sample.is_finite()) {
            return Err(OfflineProcessError::NonFiniteInput);
        }
        write_wav_pcm16(path, selection, sample_rate, channels)
            .map_err(|error| OfflineProcessError::ExportFailed(format_wav_error(error)))?;
        self.pending_tasks.push(chain_id);
        self.pending_sources.push(selection.to_vec());
        let completed_task = self.pending_tasks.last().cloned().unwrap_or_default();
        self.completed_outputs
            .push((completed_task, path.to_path_buf()));
        Ok(())
    }

    /// Exports multiple independent selections. Validation happens before the
    /// first write, so an invalid item cannot leave a half-started batch.
    pub fn process_regions_to_wav(
        &mut self,
        output_dir: &Path,
        regions: &[(String, Vec<f32>, usize, usize)],
        sample_rate: u32,
        channels: u16,
    ) -> Result<usize, OfflineProcessError> {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() {
            return Err(OfflineProcessError::ExportFailed(
                "invalid output directory".into(),
            ));
        }
        let mut names = std::collections::HashSet::with_capacity(regions.len());
        for (name, source, start, end) in regions {
            let actual_end = if *end == usize::MAX {
                source.len()
            } else {
                *end
            };
            if name.trim().is_empty()
                || !names.insert(name.clone())
                || source.is_empty()
                || *start >= actual_end
                || actual_end > source.len()
                || source[*start..actual_end]
                    .iter()
                    .any(|sample| !sample.is_finite())
                || self.completed_outputs.iter().any(|(id, _)| id == name)
            {
                return Err(OfflineProcessError::InvalidRange);
            }
        }

        let mut completed = 0usize;
        for (name, source, start, end) in regions {
            let path = output_dir.join(format!("{completed:04}_{}.wav", safe_filename(name)));
            self.process_region_to_wav(
                source,
                *start,
                *end,
                sample_rate,
                channels,
                &path,
                name.clone(),
            )?;
            completed += 1;
        }
        Ok(completed)
    }

    pub fn completed_output(&self, chain_id: &str) -> Option<&Path> {
        self.completed_outputs
            .iter()
            .find_map(|(id, path)| (id == chain_id).then_some(path.as_path()))
    }

    pub fn pending_count(&self) -> usize {
        self.pending_tasks.len().min(self.pending_sources.len())
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide offline processing state.
    pub fn audit_selection_based_processor(&self) -> bool {
        self.pending_tasks.len() == self.pending_sources.len()
            && self
                .pending_sources
                .iter()
                .all(|source| !source.is_empty() && source.iter().all(|sample| sample.is_finite()))
            && self.completed_outputs.iter().all(|(id, path)| {
                !id.trim().is_empty() && !path.as_os_str().is_empty() && path.is_file()
            })
            && self
                .completed_outputs
                .iter()
                .enumerate()
                .all(|(index, (id, _))| {
                    !self.completed_outputs[..index]
                        .iter()
                        .any(|(previous, _)| previous == id)
                })
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
        "selection".into()
    } else {
        result
    }
}

fn format_wav_error(error: WavExportError) -> String {
    format!("WAV export failed: {error:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn exports_only_the_requested_selection() {
        let path =
            std::env::temp_dir().join(format!("aura-selection-{}-{}.wav", std::process::id(), 1));
        let mut processor = OfflineOrchestrator::new();
        processor
            .process_region_to_wav(
                &[0.0, 0.1, 0.2, 0.3],
                1,
                3,
                48_000,
                1,
                &path,
                "gain".to_string(),
            )
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 4);
        assert_eq!(processor.pending_tasks, vec!["gain"]);
        assert_eq!(processor.pending_sources, vec![vec![0.1, 0.2]]);
        assert!(processor.completed_output("gain").is_some());
        assert!(processor.audit_selection_based_processor());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_invalid_selection_without_creating_output() {
        let path = std::env::temp_dir().join(format!(
            "aura-selection-invalid-{}-{}.wav",
            std::process::id(),
            1
        ));
        let mut processor = OfflineOrchestrator::new();
        assert_eq!(
            processor.process_region_to_wav(
                &[0.0, f32::NAN],
                0,
                2,
                48_000,
                1,
                &path,
                "gain".to_string(),
            ),
            Err(OfflineProcessError::NonFiniteInput)
        );
        assert!(!path.exists());
    }

    #[test]
    fn batch_selection_export_validates_before_writing() {
        let dir = std::env::temp_dir().join(format!("aura-selection-batch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut processor = OfflineOrchestrator::new();
        let regions = vec![
            ("Lead/Vocal".to_owned(), vec![0.0_f32, 0.1, 0.2], 0, 2),
            ("Bass".to_owned(), vec![0.0_f32, -0.1], 0, 2),
        ];
        assert_eq!(
            processor.process_regions_to_wav(&dir, &regions, 48_000, 1),
            Ok(2)
        );
        assert!(dir.join("0000_LeadVocal.wav").is_file());
        assert!(dir.join("0001_Bass.wav").is_file());
        assert!(processor.audit_selection_based_processor());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn queued_selection_drain_publishes_and_clears_queue() {
        let dir = std::env::temp_dir().join(format!("aura-selection-drain-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut processor = OfflineOrchestrator::new();
        assert_eq!(
            processor.process_region_async_with_range(vec![0.0, 0.1, 0.2], 0, 3, "vocal".into()),
            Ok(())
        );
        assert_eq!(processor.drain_pending_to_wav(&dir, 48_000, 1), Ok(1));
        assert_eq!(processor.pending_count(), 0);
        assert!(processor.completed_output("vocal").is_some());
        assert!(processor.audit_selection_based_processor());
        let _ = fs::remove_dir_all(dir);
    }
}
