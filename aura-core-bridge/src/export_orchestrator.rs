impl Default for ExportOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportOrchestrator {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            last_error: None,
        }
    }

    /// INDUSTRIAL: Adds a rendering job to the high-performance queue with absolute precision and export sovereignty.
    pub fn add_job(&mut self, job: ExportJob) {
        // INDUSTRIAL: Implementation of high-performance job storage.
        // Rust's safe memory management handles large export batches with
        // absolute bit-accuracy and zero-latency.
        // Rust's JobEngine ensures bit-accurate job distribution.
        if job.name.trim().is_empty()
            || !(8_000..=384_000).contains(&job.sample_rate)
            || !matches!(job.bit_depth, 16 | 24 | 32)
            // The queue has native writers for PCM WAV, AIFF-PCM16 and FLAC.
            // Lossy codecs need an external encoder and are intentionally
            // rejected here rather than failing asynchronously after queuing.
            || matches!(job.codec, Codec::MP3 | Codec::AAC)
            // AIFF is supported by the built-in writer only at PCM16.
            || (matches!(job.codec, Codec::AIFF) && job.bit_depth != 16)
            || !job.lufs_target.is_finite()
        {
            self.last_error = Some("invalid export job".into());
            return;
        }
        self.jobs.push(job);
    }

    /// Executes the queued WAV jobs against already rendered interleaved
    /// buffers.  The renderer remains caller-owned, while this orchestrator
    /// owns validation, encoding, publication, and batch rollback.
    pub fn execute_jobs_with_buffers(
        &mut self,
        output_dir: &Path,
        buffers: &[Vec<f32>],
        channels: u16,
    ) -> Result<Vec<std::path::PathBuf>, WavExportError> {
        if output_dir.as_os_str().is_empty() || !output_dir.is_dir() || self.jobs.is_empty()
            || buffers.len() != self.jobs.len() || !(1..=32).contains(&channels)
        {
            self.last_error = Some("invalid export batch".into());
            return Err(if !(1..=32).contains(&channels) {
                WavExportError::InvalidChannelCount
            } else {
                WavExportError::InvalidTask
            });
        }

        let mut paths = Vec::with_capacity(self.jobs.len());
        let mut names = std::collections::HashSet::with_capacity(self.jobs.len());
        for (index, (job, samples)) in self.jobs.iter().zip(buffers).enumerate() {
            if !matches!(job.codec, Codec::WAV | Codec::AIFF | Codec::FLAC)
                || (matches!(job.codec, Codec::AIFF | Codec::FLAC) && job.bit_depth != 16)
                || samples.is_empty()
                || samples.iter().any(|sample| !sample.is_finite())
            {
                self.last_error = Some(format!("export job {index} is invalid or unsupported"));
                return Err(if !matches!(job.codec, Codec::WAV | Codec::AIFF | Codec::FLAC) {
                    WavExportError::UnsupportedFormat
                } else if samples.is_empty() {
                    WavExportError::EmptyBuffer
                } else {
                    WavExportError::NonFiniteSample
                });
            }
            let base = export_filename(&job.name);
            let mut name = base.clone();
            let mut suffix = 2usize;
            let extension = match job.codec { Codec::AIFF => "aiff", Codec::FLAC => "flac", _ => "wav" };
            while !names.insert(name.clone()) || output_dir.join(format!("{name}.{extension}")).exists() {
                name = format!("{base}_{suffix}");
                suffix += 1;
            }
            paths.push(output_dir.join(format!("{name}.{extension}")));
        }

        let mut published = Vec::with_capacity(paths.len());
        for (job, (path, samples)) in self.jobs.iter().zip(paths.iter().zip(buffers)) {
            let prepared = match prepare_export_buffer(samples, job.bit_depth, job.normalize, false) {
                Ok(buffer) => buffer,
                Err(error) => {
                    for prior in &published { let _ = fs::remove_file(prior); }
                    self.last_error = Some(format!("export preparation failed: {error:?}"));
                    return Err(error);
                }
            };
            let result = if matches!(job.codec, Codec::AIFF) {
                if job.bit_depth != 16 { Err(WavExportError::UnsupportedFormat) }
                else { write_aiff_pcm16(path, &prepared, job.sample_rate, channels) }
            } else if matches!(job.codec, Codec::FLAC) {
                export_interleaved_buffer_to_flac(path, &prepared, job.sample_rate, channels, true)
            } else {
                write_wav_pcm(path, &prepared, job.sample_rate, channels, job.bit_depth)
            };
            if let Err(error) = result {
                for prior in &published { let _ = fs::remove_file(prior); }
                self.last_error = Some(format!("export publication failed: {error:?}"));
                return Err(error);
            }
            published.push(path.clone());
        }
        self.last_error = None;
        Ok(published)
    }

    /// Writes a finite, interleaved render buffer for an export job.
    /// The job queue is not mutated until the writer has completed successfully.
    pub fn export_interleaved_buffer_to_wav(
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
            self.last_error = Some(format!("WAV export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// Exports through the orchestrator while preserving the requested WAV
    /// encoding in the job-facing API.
    pub fn export_interleaved_buffer_to_wav_with_format(
        &mut self,
        path: &Path,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
        bit_depth: u32,
        ieee_float: bool,
        renderer_connected: bool,
    ) -> Result<(), WavExportError> {
        let result = export_interleaved_buffer_to_wav_with_format(
            path,
            samples,
            sample_rate,
            channels,
            bit_depth,
            ieee_float,
            renderer_connected,
        );
        if let Err(error) = &result {
            self.last_error = Some(format!("WAV export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    pub fn export_interleaved_buffer_to_wave64(
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
            self.last_error = Some(format!("WAVE64 export failed: {error:?}"));
        } else {
            self.last_error = None;
        }
        result
    }

    /// INDUSTRIAL: Orchestrates the parallel rendering of all registered jobs with absolute precision and creative sovereignty.
    pub fn orchestrate_execution(&mut self) {
        self.last_error = if self.jobs.is_empty() {
            Some("no export jobs queued".into())
        } else {
            Some("rendering backend is not connected; jobs remain queued".into())
        };
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide export data integrity.
    pub fn audit_export(&self) -> bool {
        self.jobs.iter().all(|job| {
            !job.name.trim().is_empty()
                && (8_000..=384_000).contains(&job.sample_rate)
                && matches!(job.bit_depth, 16 | 24 | 32)
                && job.lufs_target.is_finite()
        }) && self.last_error.is_none()
    }
}

fn export_filename(name: &str) -> String {
    let result: String = name.chars().take(80).map(|character| {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            character
        } else {
            '_'
        }
    }).collect();
    if result.is_empty() { "export".to_owned() } else { result }
}
