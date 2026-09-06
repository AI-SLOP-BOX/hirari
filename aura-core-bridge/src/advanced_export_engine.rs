#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodecRust {
    Wav,
    Aiff,
    Flac,
    Mp3,
    Aac,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchExportSpec { pub stem: String, pub formats: Vec<CodecRust>, pub normalize_lufs: Option<i16> }

impl BatchExportSpec {
    pub fn validate(&self) -> bool { !self.stem.trim().is_empty() && self.stem.len() <= 256 && !self.formats.is_empty() && self.formats.len() <= 8 && self.deduplicated_formats().len() == self.formats.len() && self.normalize_lufs.map(|v| (-60..=0).contains(&v)).unwrap_or(true) }
    pub fn safe_stem(&self) -> Option<String> { if !self.validate() || self.stem.bytes().any(|b| b == 0 || b == b'/' || b == b'\\' || b < 0x20) { return None; } let normalized = self.stem.trim().trim_matches('.'); (!normalized.is_empty() && normalized != ".." && normalized.len() <= 128).then(|| normalized.to_owned()) }
    pub fn deduplicated_formats(&self) -> Vec<CodecRust> { let mut out = Vec::new(); for format in &self.formats { if !out.contains(format) { out.push(*format); } } out }
}

#[cfg(test)]
mod batch_export_tests {
    use super::*;
    #[test]
    fn safe_stem_rejects_paths_and_normalizes() {
        let base = |stem: &str| BatchExportSpec { stem: stem.into(), formats: vec![CodecRust::Wav], normalize_lufs: None };
        assert_eq!(base("  Mixdown... ").safe_stem().as_deref(), Some("Mixdown"));
        assert!(base("../escape").safe_stem().is_none());
        assert!(base("bad\nname").safe_stem().is_none());
        let duplicate = BatchExportSpec { stem: "x".into(), formats: vec![CodecRust::Wav, CodecRust::Wav, CodecRust::Mp3], normalize_lufs: None };
        assert_eq!(duplicate.deduplicated_formats(), vec![CodecRust::Wav, CodecRust::Mp3]);
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExportFormatRust {
    pub codec: CodecRust,
    pub bit_depth: u32,
    pub sample_rate: u32,
    pub normalize: bool,
}

impl ExportFormatRust {
    pub fn validate(&self) -> bool {
        (8_000..=384_000).contains(&self.sample_rate)
            && match self.codec {
                CodecRust::Wav => matches!(self.bit_depth, 16 | 24 | 32),
                CodecRust::Aiff => matches!(self.bit_depth, 8 | 16 | 24 | 32),
                CodecRust::Flac | CodecRust::Mp3 | CodecRust::Aac => self.bit_depth == 16,
            }
    }
}

pub fn validate_rendered_buffer(samples: &[f32], channels: u16, peak_limit: f32) -> bool { !samples.is_empty() && (1..=32).contains(&channels) && samples.len().is_multiple_of(channels as usize) && peak_limit.is_finite() && samples.iter().all(|s| s.is_finite() && s.abs() <= peak_limit) }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StemJobRust {
    pub track_id: u32,
    pub stem_name: String,
    pub format: ExportFormatRust,
}

pub struct ExportOrchestrator {
    pub active_jobs: Vec<StemJobRust>,
    completed: Vec<(u32, PathBuf)>,
    failed: Vec<(u32, String)>,
    pub last_error: Option<String>,
    renderer: Option<Box<dyn FnMut(u32) -> Result<Vec<f32>, String> + Send>>,
}

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

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportChannelKind { Audio, Instrument, Group, Effect, Output }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AvailableExportChannel {
    pub id: u32,
    pub name: String,
    pub kind: ExportChannelKind,
    pub channels: u16,
    pub selected: bool,
    pub requires_realtime: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct NamedExportRange { pub id: u32, pub name: String, pub start_sample: u64, pub end_sample: u64 }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportRangeSelection {
    Locators { start_sample: u64, end_sample: u64 },
    CycleMarkers(Vec<NamedExportRange>),
    ArrangerChains(Vec<NamedExportRange>),
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportEffectsMode { InsertsAndStrip, Dry, GroupsAndSends, MasterGroupsAndSends }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportChannelMode { Interleaved, SplitChannels, MonoDownmix, LeftRightFromSurround }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExistingFilePolicy { Error, IncrementName, Overwrite }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum NamingPart { Project, Channel, Range, Format, Counter { width: u8 }, Literal(String) }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportNamingScheme { pub parts: Vec<NamingPart>, pub separator: String }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportRequestPro {
    pub project_name: String,
    pub channel_ids: Vec<u32>,
    pub range: ExportRangeSelection,
    pub codec: CodecRust,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub effects: ExportEffectsMode,
    pub channel_mode: ExportChannelMode,
    pub naming: ExportNamingScheme,
    pub realtime: bool,
    pub deactivate_external_midi: bool,
    pub existing_file_policy: ExistingFilePolicy,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PlannedExportFile {
    pub channel_id: u32,
    pub range_id: u32,
    pub start_sample: u64,
    pub end_sample: u64,
    pub output_channels: u16,
    pub filename: String,
    pub realtime: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportPlanPro {
    pub files: Vec<PlannedExportFile>,
    pub total_source_samples: u128,
    pub realtime: bool,
}

pub fn plan_export(request: &ExportRequestPro, available: &[AvailableExportChannel], existing_names: &[String])
    -> Result<ExportPlanPro, String> {
    request.validate()?;
    if available.len() > 65_536 || !available.iter().all(AvailableExportChannel::validate)
        || available.iter().enumerate().any(|(index, channel)| available[..index].iter().any(|previous| previous.id == channel.id)) {
        return Err("available export channels are invalid".into());
    }
    let selected: Vec<_> = request.channel_ids.iter().map(|id| available.iter().find(|channel| channel.id == *id)
        .ok_or_else(|| format!("export channel {id} is unavailable"))).collect::<Result<_, _>>()?;
    let ranges = request.ranges()?;
    let extension = codec_extension(request.codec);
    let mut occupied: std::collections::BTreeSet<String> = existing_names.iter().map(|name| name.to_ascii_lowercase()).collect();
    let mut planned = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    let mut counter = 1u64;
    let realtime = request.realtime || selected.iter().any(|channel| channel.requires_realtime);
    for range in ranges {
        for channel in &selected {
            let output_channels = output_channel_count(channel.channels, request.channel_mode)?;
            let split_count = if request.channel_mode == ExportChannelMode::SplitChannels { channel.channels } else { 1 };
            for split_index in 0..split_count {
                let mut stem = request.naming.render(&request.project_name, &channel.name, &range.name, extension, counter)?;
                if split_count > 1 { stem.push_str(&format!("_ch{:02}", split_index + 1)); }
                let proposed = format!("{stem}.{extension}");
                let filename = resolve_collision(
                    &proposed,
                    request.existing_file_policy,
                    &mut occupied,
                    &mut planned,
                )?;
                files.push(PlannedExportFile { channel_id: channel.id, range_id: range.id,
                    start_sample: range.start_sample, end_sample: range.end_sample,
                    output_channels, filename, realtime });
                counter = counter.checked_add(1).ok_or_else(|| "export counter overflow".to_owned())?;
            }
        }
    }
    if files.is_empty() || files.len() > 1_000_000 { return Err("export plan file count is invalid".into()); }
    let total_source_samples = files.iter().map(|file| u128::from(file.end_sample - file.start_sample)).sum();
    Ok(ExportPlanPro { files, total_source_samples, realtime })
}

impl ExportPlanPro {
    pub fn validate(&self) -> bool {
        if self.files.is_empty() || self.files.len() > 1_000_000 { return false; }
        let mut names = std::collections::BTreeSet::new();
        let mut total = 0u128;
        for file in &self.files {
            if file.channel_id == 0 || file.range_id == 0 || file.start_sample >= file.end_sample
                || !(1..=32).contains(&file.output_channels) || file.filename.len() > 1024
                || unsafe_name(&file.filename) || !file.filename.contains('.')
                || !names.insert(file.filename.to_ascii_lowercase()) { return false; }
            let Some(next) = total.checked_add(u128::from(file.end_sample - file.start_sample)) else { return false; };
            total = next;
        }
        total == self.total_source_samples
            && self.realtime == self.files.iter().any(|file| file.realtime)
            && self.files.iter().all(|file| file.realtime == self.realtime)
    }
}

impl ExportRequestPro {
    fn validate(&self) -> Result<(), String> {
        if self.project_name.trim().is_empty() || self.project_name.len() > 256 || unsafe_name(&self.project_name) {
            return Err("export project name is invalid".into());
        }
        if self.channel_ids.is_empty() || self.channel_ids.len() > 65_536 || self.channel_ids.contains(&0)
            || self.channel_ids.iter().enumerate().any(|(index, id)| self.channel_ids[..index].contains(id)) {
            return Err("export channel selection is invalid".into());
        }
        if !(8_000..=384_000).contains(&self.sample_rate) || !valid_bit_depth(self.codec, self.bit_depth) {
            return Err("export format is invalid".into());
        }
        self.naming.validate()?;
        self.ranges().map(|_| ())
    }

    fn ranges(&self) -> Result<Vec<NamedExportRange>, String> {
        let ranges = match &self.range {
            ExportRangeSelection::Locators { start_sample, end_sample } => vec![NamedExportRange {
                id: 1, name: "Locators".into(), start_sample: *start_sample, end_sample: *end_sample }],
            ExportRangeSelection::CycleMarkers(ranges) | ExportRangeSelection::ArrangerChains(ranges) => ranges.clone(),
        };
        if ranges.is_empty() || ranges.len() > 65_536 || !ranges.iter().all(NamedExportRange::validate)
            || ranges.iter().enumerate().any(|(index, range)| ranges[..index].iter().any(|previous| previous.id == range.id)) {
            return Err("export range selection is invalid".into());
        }
        Ok(ranges)
    }
}

impl AvailableExportChannel {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 256 && !unsafe_name(&self.name)
            && (1..=32).contains(&self.channels)
    }
}

impl NamedExportRange {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 256 && !unsafe_name(&self.name)
            && self.start_sample < self.end_sample
    }
}

impl ExportNamingScheme {
    fn validate(&self) -> Result<(), String> {
        if self.parts.is_empty() || self.parts.len() > 32 || self.separator.len() > 8 || unsafe_name(&self.separator) {
            return Err("export naming scheme is invalid".into());
        }
        for part in &self.parts {
            match part {
                NamingPart::Counter { width } if !(1..=12).contains(width) => return Err("export counter width is invalid".into()),
                NamingPart::Literal(value) if value.len() > 128 || unsafe_name(value) => return Err("export naming literal is invalid".into()),
                _ => {}
            }
        }
        Ok(())
    }

    fn render(&self, project: &str, channel: &str, range: &str, format: &str, counter: u64) -> Result<String, String> {
        self.validate()?;
        let values: Vec<String> = self.parts.iter().map(|part| match part {
            NamingPart::Project => sanitize_export_name(project), NamingPart::Channel => sanitize_export_name(channel),
            NamingPart::Range => sanitize_export_name(range), NamingPart::Format => format.to_owned(),
            NamingPart::Counter { width } => format!("{counter:0width$}", width = usize::from(*width)),
            NamingPart::Literal(value) => sanitize_export_name(value),
        }).collect();
        let result = values.join(&self.separator);
        if result.is_empty() || result.len() > 512 { Err("rendered export filename is invalid".into()) } else { Ok(result) }
    }
}

fn output_channel_count(source: u16, mode: ExportChannelMode) -> Result<u16, String> {
    match mode {
        ExportChannelMode::Interleaved => Ok(source),
        ExportChannelMode::SplitChannels | ExportChannelMode::MonoDownmix => Ok(1),
        ExportChannelMode::LeftRightFromSurround if source >= 2 => Ok(2),
        ExportChannelMode::LeftRightFromSurround => Err("L/R export requires at least two source channels".into()),
    }
}

fn valid_bit_depth(codec: CodecRust, depth: u16) -> bool {
    match codec { CodecRust::Wav | CodecRust::Aiff => matches!(depth, 16 | 24 | 32),
        CodecRust::Flac => matches!(depth, 16 | 24), CodecRust::Mp3 | CodecRust::Aac => depth == 16 }
}

fn codec_extension(codec: CodecRust) -> &'static str {
    match codec { CodecRust::Wav => "wav", CodecRust::Aiff => "aiff", CodecRust::Flac => "flac",
        CodecRust::Mp3 => "mp3", CodecRust::Aac => "m4a" }
}

fn resolve_collision(
    proposed: &str,
    policy: ExistingFilePolicy,
    occupied: &mut std::collections::BTreeSet<String>,
    planned: &mut std::collections::BTreeSet<String>,
)
    -> Result<String, String> {
    let normalized = proposed.to_ascii_lowercase();
    if !occupied.contains(&normalized) && planned.insert(normalized.clone()) {
        occupied.insert(normalized); return Ok(proposed.to_owned());
    }
    match policy {
        ExistingFilePolicy::Error => Err(format!("export filename already exists: {proposed}")),
        ExistingFilePolicy::Overwrite if !planned.contains(&normalized) => {
            planned.insert(normalized);
            Ok(proposed.to_owned())
        }
        ExistingFilePolicy::Overwrite => Err(format!("export plan contains a duplicate filename: {proposed}")),
        ExistingFilePolicy::IncrementName => {
            let (stem, extension) = proposed.rsplit_once('.').unwrap_or((proposed, ""));
            for suffix in 2..=1_000_000u32 {
                let candidate = if extension.is_empty() { format!("{stem}_{suffix}") }
                    else { format!("{stem}_{suffix}.{extension}") };
                let normalized = candidate.to_ascii_lowercase();
                if occupied.insert(normalized.clone()) && planned.insert(normalized) { return Ok(candidate); }
            }
            Err("could not allocate unique export filename".into())
        }
    }
}

fn unsafe_name(value: &str) -> bool {
    value.contains('\0') || value.contains('/') || value.contains('\\') || value.contains("..")
        || value.chars().any(char::is_control)
}

fn sanitize_export_name(value: &str) -> String {
    value.trim().trim_matches('.').chars().filter(|character| !matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .filter(|character| !character.is_control()).collect::<String>().trim().to_owned()
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum QuickExportFormat {
    Wav,
    Mp3,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectEventRange {
    pub start_sample: u64,
    pub end_sample: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct QuickExportRequest {
    pub project_name: String,
    pub format: QuickExportFormat,
    pub project_sample_rate: u32,
    pub event_ranges: Vec<ProjectEventRange>,
    pub effect_tail_samples: u64,
    pub main_mix: AvailableExportChannel,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct QuickExportPlan {
    pub channel_id: u32,
    pub start_sample: u64,
    pub end_sample: u64,
    pub filename: String,
    pub codec: CodecRust,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub bitrate_kbps: Option<u16>,
    pub realtime: bool,
}

/// Builds Cubase-style Quick Audio Export settings for the Main Mix. The
/// caller supplies project event bounds, keeping project/Arranger ownership
/// outside the export engine.
pub fn plan_quick_export(request: &QuickExportRequest) -> Result<QuickExportPlan, String> {
    if request.project_name.trim().is_empty() || request.project_name.len() > 256
        || unsafe_name(&request.project_name) || !(8_000..=384_000).contains(&request.project_sample_rate)
        || request.main_mix.kind != ExportChannelKind::Output || !request.main_mix.validate()
        || request.event_ranges.is_empty() || request.event_ranges.len() > 1_000_000
        || request.event_ranges.iter().any(|range| range.start_sample >= range.end_sample) {
        return Err("quick export request is invalid".into());
    }
    let start_sample = request.event_ranges.iter().map(|range| range.start_sample).min().unwrap();
    let content_end = request.event_ranges.iter().map(|range| range.end_sample).max().unwrap();
    let max_tail = u64::from(request.project_sample_rate)
        .checked_mul(30).ok_or_else(|| "quick export tail overflow".to_owned())?;
    let end_sample = content_end.checked_add(request.effect_tail_samples.min(max_tail))
        .ok_or_else(|| "quick export range overflow".to_owned())?;
    let stem = sanitize_export_name(&request.project_name);
    if stem.is_empty() { return Err("quick export filename is invalid".into()); }
    let (codec, sample_rate, bit_depth, bitrate_kbps, extension) = match request.format {
        QuickExportFormat::Wav => (CodecRust::Wav, request.project_sample_rate, 24, None, "wav"),
        QuickExportFormat::Mp3 => (CodecRust::Mp3, 44_100, 16, Some(256), "mp3"),
    };
    Ok(QuickExportPlan {
        channel_id: request.main_mix.id,
        start_sample,
        end_sample,
        filename: format!("{stem}.{extension}"),
        codec,
        sample_rate,
        bit_depth,
        bitrate_kbps,
        realtime: request.main_mix.requires_realtime,
    })
}

impl QuickExportPlan {
    pub fn validate(&self) -> bool {
        self.channel_id != 0 && self.start_sample < self.end_sample && !self.filename.is_empty()
            && self.filename.len() <= 1024 && !unsafe_name(&self.filename)
            && match self.codec {
                CodecRust::Wav => self.filename.ends_with(".wav") && (8_000..=384_000).contains(&self.sample_rate)
                    && self.bit_depth == 24 && self.bitrate_kbps.is_none(),
                CodecRust::Mp3 => self.filename.ends_with(".mp3") && self.sample_rate == 44_100
                    && self.bit_depth == 16 && self.bitrate_kbps == Some(256),
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportQueueStatus { Pending, Rendering, Completed, Failed, Cancelled }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportQueueJobPro { pub id: u64, pub plan: ExportPlanPro, pub status: ExportQueueStatus, pub error: Option<String> }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportQueuePro { pub jobs: Vec<ExportQueueJobPro>, next_id: u64 }

impl Default for ExportQueuePro { fn default() -> Self { Self { jobs: Vec::new(), next_id: 1 } } }

impl ExportQueuePro {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid export queue".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        for job in &mut value.jobs {
            if job.status == ExportQueueStatus::Rendering {
                job.status = ExportQueueStatus::Pending;
                job.error = None;
            }
        }
        if value.audit() { Ok(value) } else { Err("invalid export queue".into()) }
    }

    pub fn enqueue(&mut self, plan: ExportPlanPro) -> Option<u64> {
        if !plan.validate() || self.jobs.len() >= 20 { return None; }
        let id = self.next_id; self.next_id = self.next_id.checked_add(1)?;
        self.jobs.push(ExportQueueJobPro { id, plan, status: ExportQueueStatus::Pending, error: None }); Some(id)
    }
    pub fn move_job(&mut self, id: u64, new_index: usize) -> bool {
        let Some(old_index) = self.jobs.iter().position(|job| job.id == id && job.status == ExportQueueStatus::Pending) else { return false; };
        if new_index >= self.jobs.len() { return false; }
        let job = self.jobs.remove(old_index); self.jobs.insert(new_index, job); true
    }
    pub fn update_job(&mut self, id: u64, plan: ExportPlanPro) -> bool {
        if !plan.validate() { return false; }
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == ExportQueueStatus::Pending) else { return false; };
        job.plan = plan; job.error = None; true
    }
    pub fn remove_job(&mut self, id: u64) -> bool {
        let Some(index) = self.jobs.iter().position(|job| job.id == id
            && matches!(job.status, ExportQueueStatus::Pending | ExportQueueStatus::Failed | ExportQueueStatus::Cancelled)) else { return false; };
        self.jobs.remove(index); true
    }
    pub fn remove_all(&mut self) -> bool {
        if self.jobs.iter().any(|job| job.status == ExportQueueStatus::Rendering) { return false; }
        self.jobs.clear(); true
    }
    pub fn begin(&mut self, id: u64) -> bool { self.transition(id, ExportQueueStatus::Pending, ExportQueueStatus::Rendering, None) }
    pub fn complete(&mut self, id: u64) -> bool { self.transition(id, ExportQueueStatus::Rendering, ExportQueueStatus::Completed, None) }
    pub fn fail(&mut self, id: u64, error: &str) -> bool {
        if error.trim().is_empty() || error.len() > 2048 || error.contains('\0') { return false; }
        self.transition(id, ExportQueueStatus::Rendering, ExportQueueStatus::Failed, Some(error.trim().to_owned()))
    }
    pub fn retry(&mut self, id: u64) -> bool { self.transition(id, ExportQueueStatus::Failed, ExportQueueStatus::Pending, None) }
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id
            && matches!(job.status, ExportQueueStatus::Pending | ExportQueueStatus::Failed)) else { return false; };
        job.status = ExportQueueStatus::Cancelled; job.error = None; true
    }
    pub fn next_pending(&self) -> Option<u64> {
        self.jobs.iter().find(|job| job.status == ExportQueueStatus::Pending).map(|job| job.id)
    }
    pub fn retry_all_failed(&mut self) -> usize {
        let mut count = 0;
        for job in &mut self.jobs {
            if job.status == ExportQueueStatus::Failed { job.status = ExportQueueStatus::Pending; job.error = None; count += 1; }
        }
        count
    }
    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        let mut counts = [0usize; 5];
        for job in &self.jobs { counts[match job.status { ExportQueueStatus::Pending => 0, ExportQueueStatus::Rendering => 1, ExportQueueStatus::Completed => 2, ExportQueueStatus::Failed => 3, ExportQueueStatus::Cancelled => 4 }] += 1; }
        (counts[0], counts[1], counts[2], counts[3], counts[4])
    }
    pub fn audit(&self) -> bool {
        self.jobs.len() <= 20 && self.next_id > 0
            && self.jobs.iter().enumerate().all(|(index, job)| job.id > 0 && job.id < self.next_id
                && job.plan.validate() && self.jobs[..index].iter().all(|previous| previous.id != job.id)
                && match job.status {
                    ExportQueueStatus::Failed => job.error.as_ref().is_some_and(|error| !error.trim().is_empty()
                        && error.len() <= 2048 && !error.contains('\0')),
                    _ => job.error.is_none(),
                })
    }
    fn transition(&mut self, id: u64, from: ExportQueueStatus, to: ExportQueueStatus, error: Option<String>) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == from) else { return false; };
        job.status = to; job.error = error; true
    }
}

/// Tracks only files produced by the current queue run. Cancellation delegates
/// deletion to the host, then restores jobs to Pending so the saved queue is
/// retained exactly as Cubase does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportQueueRun { published: std::collections::BTreeMap<u64, Vec<PathBuf>> }

impl ExportQueueRun {
    pub fn record_published(&mut self, queue: &ExportQueuePro, job_id: u64, path: PathBuf) -> bool {
        if path.as_os_str().is_empty() || path.file_name().is_none()
            || !queue.jobs.iter().any(|job| job.id == job_id && job.status == ExportQueueStatus::Rendering) { return false; }
        let paths = self.published.entry(job_id).or_default();
        if paths.len() >= 1_000_000 || paths.contains(&path) { return false; }
        paths.push(path); true
    }

    pub fn cancel_with<F>(&mut self, queue: &mut ExportQueuePro, mut remove: F) -> Result<usize, String>
    where F: FnMut(&Path) -> Result<(), String> {
        let mut removed = 0usize;
        while let Some((job_id, path)) = self.published.iter().find_map(|(job_id, paths)| paths.first().cloned().map(|path| (*job_id, path))) {
            remove(&path)?;
            let paths = self.published.get_mut(&job_id).expect("published job exists");
            paths.remove(0); removed += 1;
            if paths.is_empty() { self.published.remove(&job_id); }
        }
        for job in &mut queue.jobs {
            if matches!(job.status, ExportQueueStatus::Rendering | ExportQueueStatus::Completed) {
                job.status = ExportQueueStatus::Pending; job.error = None;
            }
        }
        Ok(removed)
    }

    pub fn published_count(&self) -> usize { self.published.values().map(Vec::len).sum() }
}

#[cfg(test)]
mod export_plan_tests {
    use super::*;

    fn request() -> ExportRequestPro {
        ExportRequestPro { project_name: "Album".into(), channel_ids: vec![1, 2],
            range: ExportRangeSelection::CycleMarkers(vec![NamedExportRange { id: 1, name: "Verse".into(),
                start_sample: 0, end_sample: 48_000 }, NamedExportRange { id: 2, name: "Chorus".into(),
                start_sample: 48_000, end_sample: 96_000 }]), codec: CodecRust::Wav, sample_rate: 48_000,
            bit_depth: 24, effects: ExportEffectsMode::MasterGroupsAndSends,
            channel_mode: ExportChannelMode::Interleaved, naming: ExportNamingScheme {
                parts: vec![NamingPart::Project, NamingPart::Channel, NamingPart::Range], separator: "_".into() },
            realtime: false, deactivate_external_midi: true, existing_file_policy: ExistingFilePolicy::Error }
    }
    fn channels() -> Vec<AvailableExportChannel> { vec![
        AvailableExportChannel { id: 1, name: "Drums".into(), kind: ExportChannelKind::Group,
            channels: 2, selected: true, requires_realtime: false },
        AvailableExportChannel { id: 2, name: "Hardware Synth".into(), kind: ExportChannelKind::Instrument,
            channels: 2, selected: true, requires_realtime: true }] }

    #[test]
    fn plans_channel_by_range_matrix_and_detects_realtime() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        assert_eq!(plan.files.len(), 4); assert!(plan.realtime);
        assert_eq!(plan.files[0].filename, "Album_Drums_Verse.wav");
        assert!(plan.files.iter().all(|file| file.output_channels == 2 && file.realtime));
        assert_eq!(plan.total_source_samples, 192_000);
    }

    #[test]
    fn split_channels_and_increment_policy_never_collide() {
        let mut request = request(); request.channel_ids = vec![1]; request.range = ExportRangeSelection::Locators {
            start_sample: 0, end_sample: 48_000 }; request.channel_mode = ExportChannelMode::SplitChannels;
        request.naming.parts = vec![NamingPart::Literal("Stem".into())];
        request.existing_file_policy = ExistingFilePolicy::IncrementName;
        let plan = plan_export(&request, &channels(), &["Stem_ch01.wav".into()]).unwrap();
        assert_eq!(plan.files.iter().map(|file| file.filename.as_str()).collect::<Vec<_>>(),
            vec!["Stem_ch01_2.wav", "Stem_ch02.wav"]);
    }

    #[test]
    fn invalid_range_and_lr_from_mono_fail_before_rendering() {
        let mut request = request(); request.channel_ids = vec![1];
        request.range = ExportRangeSelection::Locators { start_sample: 10, end_sample: 10 };
        assert!(plan_export(&request, &channels(), &[]).is_err());
        request.range = ExportRangeSelection::Locators { start_sample: 0, end_sample: 10 };
        request.channel_mode = ExportChannelMode::LeftRightFromSurround;
        let mono = vec![AvailableExportChannel { channels: 1, ..channels()[0].clone() }];
        assert!(plan_export(&request, &mono, &[]).is_err());
    }

    #[test]
    fn overwrite_allows_disk_replacement_but_not_duplicate_plan_outputs() {
        let mut request = request();
        request.channel_ids = vec![1];
        request.range = ExportRangeSelection::Locators {
            start_sample: 0,
            end_sample: 48_000,
        };
        request.naming.parts = vec![NamingPart::Literal("Mix".into())];
        request.existing_file_policy = ExistingFilePolicy::Overwrite;
        let plan = plan_export(&request, &channels(), &["Mix.wav".into()]).unwrap();
        assert_eq!(plan.files[0].filename, "Mix.wav");

        request.channel_ids = vec![1, 2];
        assert!(plan_export(&request, &channels(), &[]).is_err());
    }

    #[test]
    fn quick_wav_uses_main_mix_project_rate_and_caps_tail_at_thirty_seconds() {
        let request = QuickExportRequest {
            project_name: "Film Score".into(),
            format: QuickExportFormat::Wav,
            project_sample_rate: 48_000,
            event_ranges: vec![
                ProjectEventRange { start_sample: 24_000, end_sample: 96_000 },
                ProjectEventRange { start_sample: 0, end_sample: 48_000 },
            ],
            effect_tail_samples: 48_000 * 45,
            main_mix: AvailableExportChannel { id: 9, name: "Main Mix".into(),
                kind: ExportChannelKind::Output, channels: 2, selected: true, requires_realtime: true },
        };
        let plan = plan_quick_export(&request).unwrap();
        assert_eq!(plan.start_sample, 0);
        assert_eq!(plan.end_sample, 96_000 + 48_000 * 30);
        assert_eq!(plan.filename, "Film Score.wav");
        assert_eq!(plan.sample_rate, 48_000);
        assert_eq!(plan.bit_depth, 24);
        assert!(plan.realtime);
        assert!(plan.validate());
    }

    #[test]
    fn quick_mp3_uses_fixed_delivery_format_and_rejects_non_output_channel() {
        let mut request = QuickExportRequest {
            project_name: "Demo".into(),
            format: QuickExportFormat::Mp3,
            project_sample_rate: 96_000,
            event_ranges: vec![ProjectEventRange { start_sample: 100, end_sample: 1_000 }],
            effect_tail_samples: 0,
            main_mix: AvailableExportChannel { id: 4, name: "Main Mix".into(),
                kind: ExportChannelKind::Output, channels: 2, selected: true, requires_realtime: false },
        };
        let plan = plan_quick_export(&request).unwrap();
        assert_eq!((plan.sample_rate, plan.bit_depth, plan.bitrate_kbps), (44_100, 16, Some(256)));
        assert_eq!(plan.filename, "Demo.mp3");
        assert!(!plan.realtime);
        assert!(plan.validate());

        request.main_mix.kind = ExportChannelKind::Group;
        assert!(plan_quick_export(&request).is_err());
        request.main_mix.kind = ExportChannelKind::Output;
        request.event_ranges[0].end_sample = request.event_ranges[0].start_sample;
        assert!(plan_quick_export(&request).is_err());
    }

    #[test]
    fn export_queue_has_explicit_retryable_transitions() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default(); let id = queue.enqueue(plan).unwrap();
        assert!(queue.begin(id)); assert!(queue.fail(id, "encoder unavailable")); assert!(queue.retry(id));
        assert!(queue.begin(id)); assert!(queue.complete(id));
        assert_eq!(queue.jobs[0].status, ExportQueueStatus::Completed);
    }

    #[test]
    fn export_queue_reports_counts_and_retries_all_failures() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let first = queue.enqueue(plan.clone()).unwrap();
        let second = queue.enqueue(plan).unwrap();
        assert!(queue.begin(first));
        assert!(queue.fail(first, "temporary"));
        assert!(queue.begin(second));
        assert!(queue.fail(second, "temporary"));
        assert_eq!(queue.counts(), (0, 0, 0, 2, 0));
        assert_eq!(queue.retry_all_failed(), 2);
        assert_eq!(queue.next_pending(), Some(first));
        assert!(queue.audit());
    }

    #[test]
    fn export_queue_round_trip_recovers_interrupted_render_as_pending() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let id = queue.enqueue(plan).unwrap();
        assert!(queue.begin(id));
        let json = queue.to_json().unwrap();
        let restored = ExportQueuePro::from_json(&json).unwrap();
        assert_eq!(restored.jobs[0].status, ExportQueueStatus::Pending);
        assert_eq!(restored.next_pending(), Some(id));
        assert!(restored.audit());
    }

    #[test]
    fn export_queue_rejects_tampered_plan_totals_and_duplicate_outputs() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        queue.enqueue(plan).unwrap();
        let json = queue.to_json().unwrap();
        let invalid_total = json.replace("\"total_source_samples\":192000", "\"total_source_samples\":1");
        assert!(ExportQueuePro::from_json(&invalid_total).is_err());
        let mut duplicate = queue.clone();
        duplicate.jobs[0].plan.files[1].filename = duplicate.jobs[0].plan.files[0].filename.clone();
        assert!(duplicate.to_json().is_err());
    }

    #[test]
    fn queue_enforces_cubase_twenty_job_limit_and_updates_pending_job() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        for _ in 0..20 { assert!(queue.enqueue(plan.clone()).is_some()); }
        assert!(queue.enqueue(plan.clone()).is_none());
        let id = queue.jobs[0].id;
        let mut updated = plan; updated.files[0].filename = "Updated.wav".into();
        assert!(queue.update_job(id, updated));
        assert_eq!(queue.jobs[0].plan.files[0].filename, "Updated.wav");
        assert!(queue.begin(id));
        assert!(!queue.update_job(id, queue.jobs[0].plan.clone()));
        assert!(queue.audit());
    }

    #[test]
    fn cancelling_run_removes_only_recorded_outputs_and_keeps_jobs_pending() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let first = queue.enqueue(plan.clone()).unwrap();
        let second = queue.enqueue(plan).unwrap();
        assert!(queue.begin(first));
        let mut run = ExportQueueRun::default();
        assert!(run.record_published(&queue, first, PathBuf::from("/mixdown/one.wav")));
        assert!(run.record_published(&queue, first, PathBuf::from("/mixdown/two.wav")));
        assert!(!run.record_published(&queue, second, PathBuf::from("/mixdown/not-rendering.wav")));
        assert!(queue.complete(first));
        assert!(queue.begin(second));
        let mut removed = Vec::new();
        assert_eq!(run.cancel_with(&mut queue, |path| { removed.push(path.to_path_buf()); Ok(()) }).unwrap(), 2);
        assert_eq!(removed.len(), 2);
        assert!(queue.jobs.iter().all(|job| job.status == ExportQueueStatus::Pending));
        assert_eq!(run.published_count(), 0);
        assert!(queue.audit());
    }

    #[test]
    fn remove_all_refuses_to_mutate_a_running_queue() {
        let plan = plan_export(&request(), &channels(), &[]).unwrap();
        let mut queue = ExportQueuePro::default();
        let id = queue.enqueue(plan).unwrap();
        assert!(queue.begin(id));
        assert!(!queue.remove_all());
        assert_eq!(queue.jobs.len(), 1);
    }
}
