impl Default for ExportOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportOrchestrator {
    pub fn new() -> Self {
        Self {
            active_jobs: Vec::new(),
            completed: Vec::new(),
            failed: Vec::new(),
            last_error: None,
            renderer: None,
        }
    }

    /// Installs the native/offline renderer used by the ordinary queue entry
    /// point.  The callback is owned by the queue so UI, CLI and automation
    /// all use the same completion and atomic-publish path.
    pub fn set_renderer<F>(&mut self, renderer: F)
    where
        F: FnMut(u32) -> Result<Vec<f32>, String> + Send + 'static,
    {
        self.renderer = Some(Box::new(renderer));
    }

    pub fn clear_renderer(&mut self) {
        self.renderer = None;
    }

    /// Adds one stem to the delivery queue.  Queue insertion is validated and
    /// idempotent by track ID so malformed jobs cannot reach a renderer.
    pub fn queue_job(&mut self, job: StemJobRust) -> bool {
        if job.track_id == 0
            || job.stem_name.trim().is_empty()
            || job.stem_name.len() > 256
            || job.stem_name.contains('\0')
            || !job.format.validate()
            || self.active_jobs.iter().any(|queued| queued.track_id == job.track_id)
        {
            return false;
        }
        self.active_jobs.push(job);
        true
    }

    pub fn cancel_job(&mut self, track_id: u32) -> bool {
        let before = self.active_jobs.len();
        self.active_jobs.retain(|job| job.track_id != track_id);
        before != self.active_jobs.len()
    }

    /// Validates an export request and reports why it cannot be completed.
    ///
    /// The public API intentionally remains `()`: callers of the original bridge
    /// API must continue to compile.  A job is never removed from `active_jobs`
    /// unless a real renderer has completed it.  Rendering is not connected in
    /// this crate, so a valid request is an explicit failure rather than a false
    /// success.
    pub fn execute_export(&mut self, output_dir: String) {
        if let Err(reason) = Self::validate_output_dir(&output_dir) {
            self.last_error = Some(reason.to_owned());
            eprintln!("advanced export failed: {reason}");
            return;
        }

        if self.active_jobs.is_empty() {
            self.last_error = Some("no export jobs are queued".to_owned());
            eprintln!("advanced export failed: no export jobs are queued");
            return;
        }

        if let Some(mut renderer) = self.renderer.take() {
            let output = Path::new(&output_dir).to_path_buf();
            let result = self.execute_export_with_renderer(&output, 2, |track_id| renderer(track_id));
            self.renderer = Some(renderer);
            if let Err(error) = result {
                self.last_error = Some(format!("{error:?}"));
            }
            return;
        }

        // Do not pretend that encoding/rendering happened.  Keeping the jobs
        // queued makes the failure observable through the existing API and
        // prevents a later audit from treating them as delivered.
        self.last_error = Some("rendering backend is not connected".to_owned());
        eprintln!(
            "advanced export failed: rendering backend is not connected ({} job(s) remain queued)",
            self.active_jobs.len()
        );
    }

    /// Connects the export orchestrator to a real offline renderer without
    /// coupling this control-plane module to a particular native engine.
    /// Every queued stem is rendered before any output is published, so a
    /// renderer failure cannot leave a partially-consumed job batch.
    pub fn execute_export_with_renderer<F>(
        &mut self,
        output_dir: &Path,
        channels: u16,
        mut render: F,
    ) -> Result<usize, WavExportError>
    where
        F: FnMut(u32) -> Result<Vec<f32>, String>,
    {
        if !output_dir.is_dir() || !(1..=32).contains(&channels) {
            self.last_error = Some("invalid export output or channel count".to_owned());
            return Err(WavExportError::InvalidPath);
        }
        if self.active_jobs.is_empty() {
            self.last_error = Some("no export jobs are queued".to_owned());
            return Err(WavExportError::InvalidTask);
        }

        let job_ids: Vec<u32> = self.active_jobs.iter().map(|job| job.track_id).collect();
        let mut renders = Vec::with_capacity(job_ids.len());
        for track_id in job_ids {
            match render(track_id) {
                Ok(samples) if !samples.is_empty() => renders.push((track_id, samples)),
                Ok(_) => {
                    let error = WavExportError::EmptyBuffer;
                    self.failed.retain(|(id, _)| *id != track_id);
                    self.failed.push((track_id, format!("{error:?}")));
                    self.last_error = Some(format!("{error:?}"));
                    return Err(error);
                }
                Err(reason) => {
                    self.failed.retain(|(id, _)| *id != track_id);
                    self.failed.push((track_id, reason.clone()));
                    self.last_error = Some(reason);
                    return Err(WavExportError::InvalidTask);
                }
            }
        }

        self.execute_export_with_buffers(output_dir, &renders, channels)
    }

    /// Publishes a rendered PCM buffer for one queued stem. The job is moved
    /// out of the active queue only after the atomic WAV write succeeds.
    pub fn complete_job_with_buffer(
        &mut self,
        track_id: u32,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), WavExportError> {
        let Some(job) = self.active_jobs.iter().find(|job| job.track_id == track_id) else {
            return Err(WavExportError::InvalidTask);
        };
        if !job.format.validate() || job.format.sample_rate != sample_rate {
            let error = WavExportError::UnsupportedFormat;
            self.last_error = Some(format!("{error:?}"));
            self.failed.retain(|(id, _)| *id != track_id);
            self.failed.push((track_id, format!("{error:?}")));
            return Err(error);
        }
        if self
            .completed
            .iter()
            .any(|(id, output)| *id == track_id || output == path)
        {
            return Err(WavExportError::InvalidTask);
        }
        let result = publish_rendered_stem(path, samples, &job.format, channels);
        match result {
            Ok(()) => {
                self.active_jobs.retain(|job| job.track_id != track_id);
                self.failed.retain(|(id, _)| *id != track_id);
                self.completed.push((track_id, path.to_path_buf()));
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.failed.retain(|(id, _)| *id != track_id);
                let error_text = format!("{error:?}");
                self.failed.push((track_id, error_text.clone()));
                self.last_error = Some(error_text);
                Err(error)
            }
        }
    }

    /// Completes multiple already-rendered stem buffers. Rendering is supplied
    /// by the caller; this method owns validation, deterministic filenames,
    /// atomic writes, and completion state transitions.
    pub fn execute_export_with_buffers(
        &mut self,
        output_dir: &Path,
        renders: &[(u32, Vec<f32>)],
        channels: u16,
    ) -> Result<usize, WavExportError> {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() {
            return Err(WavExportError::InvalidPath);
        }
        if !(1..=32).contains(&channels) {
            return Err(WavExportError::InvalidChannelCount);
        }
        let mut seen = std::collections::HashSet::with_capacity(renders.len());
        let mut destinations = std::collections::HashSet::with_capacity(renders.len());
        for (track_id, samples) in renders {
            if !seen.insert(*track_id)
                || self.completed.iter().any(|(id, _)| *id == *track_id)
                || !self.active_jobs.iter().any(|job| job.track_id == *track_id)
            {
                return Err(WavExportError::InvalidTask);
            }
            if samples.is_empty() {
                return Err(WavExportError::EmptyBuffer);
            }
            let Some(job) = self.active_jobs.iter().find(|job| job.track_id == *track_id) else {
                return Err(WavExportError::InvalidTask);
            };
            if !job.format.validate() {
                return Err(WavExportError::UnsupportedFormat);
            }
            let path = output_dir.join(format!(
                "{track_id:04}_{}.{}",
                safe_filename(&job.stem_name),
                codec_extension(job.format.codec)
            ));
            if !destinations.insert(path.clone()) || path.exists() {
                return Err(WavExportError::InvalidTask);
            }
        }

        let mut completed = 0usize;
        for (track_id, samples) in renders {
            let Some(job) = self
                .active_jobs
                .iter()
                .find(|job| job.track_id == *track_id)
            else {
                return Err(WavExportError::InvalidTask);
            };
            let sample_rate = job.format.sample_rate;
            let path = output_dir.join(format!(
                "{track_id:04}_{}.{}",
                safe_filename(&job.stem_name),
                codec_extension(job.format.codec)
            ));
            self.complete_job_with_buffer(*track_id, &path, samples, sample_rate, channels)?;
            completed += 1;
        }
        Ok(completed)
    }

    /// Connects the queue to a renderer that publishes a native WAV file per
    /// track. This is the bridge used by the loaded-project UI/CLI path: the
    /// engine owns the graph render, while this queue owns naming, collision
    /// checks, publication, and job state transitions.
    pub fn execute_export_with_file_renderer<F>(
        &mut self,
        output_dir: &Path,
        mut render: F,
    ) -> Result<usize, WavExportError>
    where
        F: FnMut(u32, &Path) -> Result<(), String>,
    {
        if !output_dir.is_dir() || self.active_jobs.is_empty() {
            return Err(WavExportError::InvalidPath);
        }
        let mut destinations = Vec::with_capacity(self.active_jobs.len());
        for job in &self.active_jobs {
            if !job.format.validate() || job.format.codec != CodecRust::Wav {
                return Err(WavExportError::UnsupportedFormat);
            }
            let path = output_dir.join(format!(
                "{id:04}_{}.wav",
                safe_filename(&job.stem_name),
                id = job.track_id
            ));
            if path.exists() || destinations.iter().any(|candidate| candidate == &path) {
                return Err(WavExportError::InvalidTask);
            }
            destinations.push(path);
        }

        let mut temporary = Vec::with_capacity(self.active_jobs.len());
        let mut published = Vec::with_capacity(self.active_jobs.len());
        for (job, destination) in self.active_jobs.iter().zip(&destinations) {
            let temp = output_dir.join(format!(
                ".aura-render-{}-{}.tmp.wav",
                std::process::id(),
                job.track_id
            ));
            let _ = std::fs::remove_file(&temp);
            if let Err(reason) = render(job.track_id, &temp) {
                let _ = std::fs::remove_file(&temp);
                for path in &temporary { let _ = std::fs::remove_file(path); }
                for path in &published { let _ = std::fs::remove_file(path); }
                self.last_error = Some(reason);
                return Err(WavExportError::InvalidTask);
            }
            let valid = std::fs::metadata(&temp).map(|meta| meta.is_file() && meta.len() > 44).unwrap_or(false);
            if !valid || std::fs::rename(&temp, destination).is_err() {
                let _ = std::fs::remove_file(&temp);
                for path in &temporary { let _ = std::fs::remove_file(path); }
                for path in &published { let _ = std::fs::remove_file(path); }
                self.last_error = Some("native renderer produced an invalid WAV".to_owned());
                return Err(WavExportError::Io("native renderer publication failed".to_owned()));
            }
            temporary.push(temp);
            published.push(destination.clone());
        }

        let completed_ids: Vec<u32> = self.active_jobs.iter().map(|job| job.track_id).collect();
        let completed_count = completed_ids.len();
        for (track_id, path) in completed_ids.into_iter().zip(published) {
            self.completed.push((track_id, path));
            self.active_jobs.retain(|job| job.track_id != track_id);
        }
        self.last_error = None;
        Ok(completed_count)
    }

    pub fn completed_output(&self, track_id: u32) -> Option<&Path> {
        self.completed
            .iter()
            .find_map(|(id, path)| (*id == track_id).then_some(path.as_path()))
    }

    pub fn retry_failed_job(&mut self, track_id: u32) -> bool {
        let was_failed = self.failed.iter().any(|(id, _)| *id == track_id);
        let is_active = self.active_jobs.iter().any(|job| job.track_id == track_id);
        if was_failed && is_active {
            self.failed.retain(|(id, _)| *id != track_id);
            self.last_error = None;
            return true;
        }
        false
    }

    fn validate_output_dir(output_dir: &str) -> Result<(), &'static str> {
        if output_dir.trim().is_empty() {
            return Err("output directory is empty");
        }

        let path = std::path::Path::new(output_dir);
        if !path.exists() {
            return Err("output directory does not exist");
        }
        if !path.is_dir() {
            return Err("output path is not a directory");
        }

        // Checking the directory's metadata is portable, while the actual
        // create/write operation remains the renderer's responsibility.
        if std::fs::metadata(path).is_err() {
            return Err("output directory cannot be inspected");
        }
        Ok(())
    }

    /// Performs a delivery audit without claiming that an unconnected render
    /// backend produced output.
    pub fn audit_advanced_export_engine(&self) -> bool {
        let active_unique = self.active_jobs.iter().enumerate().all(|(index, job)| {
            !self.active_jobs[..index]
                .iter()
                .any(|previous| previous.track_id == job.track_id)
        });
        let completed_valid = self.completed.iter().all(|(id, path)| {
            !self.active_jobs.iter().any(|job| job.track_id == *id)
                && !path.as_os_str().is_empty()
                && path.is_file()
        });
        active_unique
            && self.active_jobs.iter().all(|job| {
                job.track_id != 0
                    && !job.stem_name.trim().is_empty()
                    && !job.stem_name.contains('\0')
                    && job.format.validate()
            })
            && completed_valid
            && self.last_error.is_none()
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
        "stem".to_owned()
    } else {
        result
    }
}

fn publish_rendered_stem(
    path: &Path,
    samples: &[f32],
    format: &ExportFormatRust,
    channels: u16,
) -> Result<(), WavExportError> {
    if !format.validate() { return Err(WavExportError::UnsupportedFormat); }
    match format.codec {
        CodecRust::Wav => write_wav_pcm(path, samples, format.sample_rate, channels, format.bit_depth),
        CodecRust::Aiff => write_aiff_pcm(path, samples, format.sample_rate, channels, format.bit_depth),
        CodecRust::Flac => export_interleaved_buffer_to_flac(path, samples, format.sample_rate, channels, true),
        CodecRust::Mp3 => export_interleaved_buffer_to_lossy(path, samples, format.sample_rate, channels,
            LossyAudioCodec::Mp3, &LossyExportSettings::cubase_quick_export(), true),
        CodecRust::Aac => export_interleaved_buffer_to_lossy(path, samples, format.sample_rate, channels,
            LossyAudioCodec::Aac, &LossyExportSettings::cubase_quick_export(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_output_without_consuming_jobs() {
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.active_jobs.push(StemJobRust {
            track_id: 1,
            stem_name: "vocals".to_owned(),
            format: ExportFormatRust {
                codec: CodecRust::Wav,
                bit_depth: 24,
                sample_rate: 48_000,
                normalize: false,
            },
        });

        orchestrator.execute_export("/path/that/does/not/exist".to_owned());

        assert_eq!(orchestrator.active_jobs.len(), 1);
        assert!(!orchestrator.audit_advanced_export_engine());
    }

    #[test]
    fn native_file_renderer_consumes_queue_only_after_publication() {
        let dir = std::env::temp_dir().join(format!("aura-native-file-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.active_jobs.push(StemJobRust {
            track_id: 7,
            stem_name: "Main Mix".into(),
            format: ExportFormatRust { codec: CodecRust::Wav, bit_depth: 16, sample_rate: 48_000, normalize: false },
        });
        let rendered = orchestrator.execute_export_with_file_renderer(&dir, |_track, path| {
            write_wav_pcm(path, &[0.0, 0.25, -0.25, 0.0], 48_000, 2, 16)
                .map_err(|error| format!("{error:?}"))
        }).unwrap();
        assert_eq!(rendered, 1);
        assert!(orchestrator.active_jobs.is_empty());
        assert_eq!(orchestrator.completed_output(7).unwrap().file_name().unwrap(), "0007_Main_Mix.wav");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn queue_rejects_invalid_and_duplicate_jobs() {
        let mut orchestrator = ExportOrchestrator::new();
        let valid = || StemJobRust {
            track_id: 9,
            stem_name: "vocals".to_owned(),
            format: ExportFormatRust { codec: CodecRust::Wav, bit_depth: 16, sample_rate: 48_000, normalize: false },
        };
        assert!(orchestrator.queue_job(valid()));
        assert!(!orchestrator.queue_job(valid()));
        assert!(!orchestrator.queue_job(StemJobRust { track_id: 10, stem_name: "".into(), format: valid().format }));
        assert!(!orchestrator.queue_job(StemJobRust { track_id: 11, stem_name: "bad\0name".into(), format: valid().format }));
        assert!(orchestrator.cancel_job(9));
        assert!(!orchestrator.cancel_job(9));
    }

    #[test]
    fn connected_output_still_fails_explicitly_until_renderer_exists() {
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.active_jobs.push(StemJobRust {
            track_id: 1,
            stem_name: "drums".to_owned(),
            format: ExportFormatRust {
                codec: CodecRust::Wav,
                bit_depth: 24,
                sample_rate: 48_000,
                normalize: false,
            },
        });

        let output_dir = std::env::temp_dir();
        orchestrator.execute_export(output_dir.to_string_lossy().into_owned());

        assert_eq!(orchestrator.active_jobs.len(), 1);
        assert!(!orchestrator.audit_advanced_export_engine());
    }

    #[test]
    fn batch_completion_writes_each_stem_and_consumes_only_successes() {
        let output_dir =
            std::env::temp_dir().join(format!("aura-advanced-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output_dir);
        std::fs::create_dir_all(&output_dir).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        for (track_id, name) in [(1, "Drums/Main"), (2, "Bass")] {
            orchestrator.active_jobs.push(StemJobRust {
                track_id,
                stem_name: name.to_owned(),
                format: ExportFormatRust {
                    codec: CodecRust::Wav,
                    bit_depth: 16,
                    sample_rate: 48_000,
                    normalize: false,
                },
            });
        }

        let renders = vec![(1, vec![0.0_f32, 0.25]), (2, vec![-0.25, 0.0])];
        assert_eq!(
            orchestrator.execute_export_with_buffers(&output_dir, &renders, 1),
            Ok(2)
        );
        assert!(orchestrator.active_jobs.is_empty());
        assert!(output_dir.join("0001_DrumsMain.wav").is_file());
        assert!(output_dir.join("0002_Bass.wav").is_file());
        assert!(orchestrator.audit_advanced_export_engine());
        let _ = std::fs::remove_dir_all(output_dir);
    }

    #[test]
    fn duplicate_batch_entries_are_rejected_before_writing() {
        let output_dir =
            std::env::temp_dir().join(format!("aura-advanced-duplicate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output_dir);
        std::fs::create_dir_all(&output_dir).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        orchestrator.active_jobs.push(StemJobRust {
            track_id: 3,
            stem_name: "Synth".to_owned(),
            format: ExportFormatRust {
                codec: CodecRust::Wav,
                bit_depth: 16,
                sample_rate: 48_000,
                normalize: false,
            },
        });
        assert_eq!(
            orchestrator.execute_export_with_buffers(
                &output_dir,
                &[(3, vec![0.0]), (3, vec![0.0])],
                1
            ),
            Err(WavExportError::InvalidTask)
        );
        assert!(orchestrator.active_jobs.iter().any(|job| job.track_id == 3));
        let _ = std::fs::remove_dir_all(output_dir);
    }

    #[test]
    fn publishes_wav_24_and_aiff_with_format_specific_extensions() {
        let output_dir = std::env::temp_dir().join(format!("aura-advanced-formats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output_dir);
        std::fs::create_dir_all(&output_dir).unwrap();
        let mut orchestrator = ExportOrchestrator::new();
        assert!(orchestrator.queue_job(StemJobRust { track_id: 1, stem_name: "Mix 24".into(),
            format: ExportFormatRust { codec: CodecRust::Wav, bit_depth: 24, sample_rate: 48_000, normalize: false } }));
        assert!(orchestrator.queue_job(StemJobRust { track_id: 2, stem_name: "Mix AIFF".into(),
            format: ExportFormatRust { codec: CodecRust::Aiff, bit_depth: 16, sample_rate: 48_000, normalize: false } }));
        let renders = vec![(1, vec![0.0, 0.25]), (2, vec![0.0, -0.25])];
        assert_eq!(orchestrator.execute_export_with_buffers(&output_dir, &renders, 1), Ok(2));
        assert!(output_dir.join("0001_Mix_24.wav").is_file());
        assert!(output_dir.join("0002_Mix_AIFF.aiff").is_file());
        let wav = std::fs::read(output_dir.join("0001_Mix_24.wav")).unwrap();
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 24);
        assert_eq!(&std::fs::read(output_dir.join("0002_Mix_AIFF.aiff")).unwrap()[..4], b"FORM");
        let _ = std::fs::remove_dir_all(output_dir);
    }

    #[test]
    fn queue_accepts_connected_lossy_formats_but_rejects_unsupported_depths() {
        let mut orchestrator = ExportOrchestrator::new();
        assert!(orchestrator.queue_job(StemJobRust { track_id: 1, stem_name: "MP3".into(),
            format: ExportFormatRust { codec: CodecRust::Mp3, bit_depth: 16, sample_rate: 44_100, normalize: false } }));
        assert!(orchestrator.queue_job(StemJobRust { track_id: 2, stem_name: "AIFF24".into(),
            format: ExportFormatRust { codec: CodecRust::Aiff, bit_depth: 24, sample_rate: 48_000, normalize: false } }));
        assert!(!orchestrator.queue_job(StemJobRust { track_id: 3, stem_name: "FLAC32".into(),
            format: ExportFormatRust { codec: CodecRust::Flac, bit_depth: 32, sample_rate: 48_000, normalize: false } }));
    }
}
use std::path::{Path, PathBuf};

use crate::export::{
    export_interleaved_buffer_to_flac, export_interleaved_buffer_to_lossy, write_aiff_pcm,
    write_wav_pcm, LossyAudioCodec, LossyExportSettings, WavExportError,
};
