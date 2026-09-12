pub use crate::automation_recorder::recording_preview::RecordingPreviewRegion;
use crate::automation_recorder::recording_preview::{RecordingPreview, RecordingPreviewError};
use crate::recording_stream::{RecordingStreamError, StreamingRecordingWriter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CAPTURE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingSessionState {
    Idle,
    Recording,
    Stopped,
}

/// Product-level lifecycle for a recording capture.
///
/// `RecordingSessionState` is retained as a small compatibility state for
/// older callers. This richer state is what recovery and UI status surfaces
/// should use, because stopping a capture is not the same as publishing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingLifecycle {
    Idle,
    Armed,
    Recording,
    Finalizing,
    Stopped,
    Committed,
    Recovered,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecordingSessionError {
    AlreadyRecording,
    NotRecording,
    EmptyRecording,
    InvalidPunchRange,
    PunchRangeExpired,
    Preview(RecordingPreviewError),
    Stream(RecordingStreamError),
}

impl From<RecordingPreviewError> for RecordingSessionError {
    fn from(error: RecordingPreviewError) -> Self {
        Self::Preview(error)
    }
}

impl From<RecordingStreamError> for RecordingSessionError {
    fn from(error: RecordingStreamError) -> Self {
        Self::Stream(error)
    }
}

/// Coordinates transport-facing recording state with the preallocated preview buffer.
/// Audio input is still supplied by the host callback; this type owns the lifecycle
/// and prevents UI state from claiming a recording that never started.
pub struct RecordingSession {
    preview: RecordingPreview,
    state: RecordingSessionState,
    lifecycle: RecordingLifecycle,
    last_region: Option<RecordingPreviewRegion>,
    takes: Vec<RecordingPreviewRegion>,
    take_paths: Vec<PathBuf>,
    active_take: usize,
    active_writer: Option<StreamingRecordingWriter>,
    last_spool_path: Option<PathBuf>,
    captured_frames: u64,
    start_sample: u64,
    count_in_remaining_frames: u64,
    pending_start_sample: u64,
    punch_out_sample: Option<u64>,
    pending_punch_out_sample: Option<u64>,
    auto_stop_requested: bool,
}

impl RecordingSession {
    pub fn try_new(
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
    ) -> Result<Self, RecordingPreviewError> {
        Ok(Self {
            preview: RecordingPreview::try_new(sample_rate, channels, max_frames)?,
            state: RecordingSessionState::Idle,
            lifecycle: RecordingLifecycle::Idle,
            last_region: None,
            takes: Vec::with_capacity(16),
            take_paths: Vec::with_capacity(16),
            active_take: 0,
            active_writer: None,
            last_spool_path: None,
            captured_frames: 0,
            start_sample: 0,
            count_in_remaining_frames: 0,
            pending_start_sample: 0,
            punch_out_sample: None,
            pending_punch_out_sample: None,
            auto_stop_requested: false,
        })
    }

    pub fn state(&self) -> RecordingSessionState {
        self.state
    }

    pub fn lifecycle(&self) -> RecordingLifecycle {
        self.lifecycle
    }

    pub fn lifecycle_label(&self) -> &'static str {
        match self.lifecycle {
            RecordingLifecycle::Idle => "Idle",
            RecordingLifecycle::Armed => "Armed",
            RecordingLifecycle::Recording => "Recording",
            RecordingLifecycle::Finalizing => "Finalizing",
            RecordingLifecycle::Stopped => "Stopped",
            RecordingLifecycle::Committed => "Committed",
            RecordingLifecycle::Recovered => "Recovered",
            RecordingLifecycle::Failed => "Failed",
        }
    }

    pub fn mark_committed(&mut self) -> bool {
        if self.state != RecordingSessionState::Stopped
            || self.lifecycle != RecordingLifecycle::Stopped
        {
            return false;
        }
        self.lifecycle = RecordingLifecycle::Committed;
        true
    }

    pub fn mark_recovered(&mut self) {
        self.lifecycle = RecordingLifecycle::Recovered;
    }

    pub fn arm(&mut self) -> bool {
        if self.state == RecordingSessionState::Recording
            || self.lifecycle == RecordingLifecycle::Finalizing
        {
            return false;
        }
        self.lifecycle = RecordingLifecycle::Armed;
        true
    }

    pub fn configuration_matches(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
    ) -> bool {
        self.preview.sample_rate() == sample_rate
            && self.preview.channels() == channels
            && self.preview.max_frames() == max_frames
    }

    pub fn start(&mut self, start_sample: u64) -> Result<(), RecordingSessionError> {
        if self.state == RecordingSessionState::Recording {
            return Err(RecordingSessionError::AlreadyRecording);
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| {
                RecordingSessionError::Stream(RecordingStreamError::Io(error.to_string()))
            })?
            .as_nanos();
        let sequence = NEXT_CAPTURE_ID.fetch_add(1, Ordering::Relaxed);
        let spool_path = std::env::temp_dir().join(format!(
            "aura-capture-{}-{stamp}-{sequence}.wav",
            std::process::id(),
        ));
        let writer = StreamingRecordingWriter::create(
            &spool_path,
            self.preview.sample_rate().round() as u32,
            self.preview.channels(),
        )?;
        self.preview.start(start_sample);
        self.active_writer = Some(writer);
        self.captured_frames = 0;
        self.start_sample = start_sample;
        self.last_region = None;
        self.count_in_remaining_frames = 0;
        self.punch_out_sample = self.pending_punch_out_sample.take();
        self.auto_stop_requested = false;
        self.state = RecordingSessionState::Recording;
        self.lifecycle = RecordingLifecycle::Recording;
        Ok(())
    }

    /// Arms a recording capture behind a sample-accurate count-in. Input
    /// arriving while the count-in is active is deliberately consumed and
    /// discarded; the first frame written to the take is exactly the
    /// requested start sample.
    pub fn start_with_count_in(
        &mut self,
        start_sample: u64,
        count_in_frames: u64,
    ) -> Result<(), RecordingSessionError> {
        if count_in_frames == 0 {
            return self.start(start_sample);
        }
        if self.state == RecordingSessionState::Recording {
            return Err(RecordingSessionError::AlreadyRecording);
        }
        self.pending_start_sample = start_sample;
        self.count_in_remaining_frames = count_in_frames;
        self.state = RecordingSessionState::Idle;
        self.lifecycle = RecordingLifecycle::Armed;
        Ok(())
    }

    /// Starts a sample-accurate punch capture. Input before `punch_in_sample`
    /// is consumed during count-in, while input at and after punch-out is not
    /// written to the take. Finalization remains outside the audio callback;
    /// callers observe `auto_stop_requested()` and commit through `stop()`.
    pub fn start_with_punch(
        &mut self,
        current_sample: u64,
        punch_in_sample: u64,
        punch_out_sample: u64,
    ) -> Result<(), RecordingSessionError> {
        if punch_out_sample <= punch_in_sample {
            return Err(RecordingSessionError::InvalidPunchRange);
        }
        if current_sample >= punch_out_sample {
            return Err(RecordingSessionError::PunchRangeExpired);
        }

        self.auto_stop_requested = false;
        self.pending_punch_out_sample = Some(punch_out_sample);
        if current_sample < punch_in_sample {
            self.start_with_count_in(
                punch_in_sample,
                punch_in_sample.saturating_sub(current_sample),
            )
        } else {
            self.start(current_sample)
        }
    }

    pub fn append_interleaved(&mut self, input: &[f32]) -> Result<(), RecordingSessionError> {
        if self.lifecycle == RecordingLifecycle::Armed && self.count_in_remaining_frames > 0 {
            let channels = self.preview.channels() as usize;
            if channels == 0 {
                return Err(RecordingSessionError::EmptyRecording);
            }
            let available_frames = input.len() / channels;
            if available_frames == 0 {
                return Ok(());
            }
            let discarded_frames = available_frames
                .min(self.count_in_remaining_frames.min(usize::MAX as u64) as usize);
            self.count_in_remaining_frames -= discarded_frames as u64;
            if self.count_in_remaining_frames > 0 {
                return Ok(());
            }
            let start_sample = self.pending_start_sample;
            self.start(start_sample)?;
            let consumed_samples = discarded_frames.saturating_mul(channels);
            if consumed_samples >= input.len() {
                return Ok(());
            }
            return self.append_interleaved(&input[consumed_samples..]);
        }
        if self.state != RecordingSessionState::Recording {
            return Err(RecordingSessionError::NotRecording);
        }
        let channels = self.preview.channels() as usize;
        if channels == 0 || !input.len().is_multiple_of(channels) {
            return Err(RecordingSessionError::Preview(
                RecordingPreviewError::InputIsNotInterleaved,
            ));
        }
        let requested_frames = input.len() / channels;
        let allowed_frames = self
            .punch_out_sample
            .map(|end| end.saturating_sub(self.start_sample.saturating_add(self.captured_frames)))
            .unwrap_or(requested_frames as u64)
            .min(requested_frames as u64) as usize;
        if allowed_frames == 0 {
            self.auto_stop_requested = true;
            return Ok(());
        }
        let input = &input[..allowed_frames * channels];
        match self.preview.append_interleaved(input) {
            Ok(()) | Err(RecordingPreviewError::BufferCapacityInsufficient) => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(writer) = self.active_writer.as_mut() {
            writer.append_interleaved(input)?;
            self.captured_frames = writer.frames();
        }
        if allowed_frames < requested_frames {
            self.auto_stop_requested = true;
        }
        Ok(())
    }

    pub fn auto_stop_requested(&self) -> bool {
        self.auto_stop_requested
    }

    pub fn stop(&mut self) -> Result<RecordingPreviewRegion, RecordingSessionError> {
        if self.state != RecordingSessionState::Recording {
            return Err(RecordingSessionError::NotRecording);
        }
        self.lifecycle = RecordingLifecycle::Finalizing;
        let mut region = match self.preview.stop() {
            Some(region) => region,
            None => {
                self.state = RecordingSessionState::Idle;
                self.active_writer = None;
                self.lifecycle = RecordingLifecycle::Failed;
                return Err(RecordingSessionError::NotRecording);
            }
        };
        let spool_path = match self.active_writer.take() {
            Some(writer) => match writer.finalize() {
                Ok(path) => path,
                Err(error) => {
                    self.state = RecordingSessionState::Idle;
                    self.last_region = None;
                    self.last_spool_path = None;
                    self.lifecycle = RecordingLifecycle::Failed;
                    return Err(error.into());
                }
            },
            None => {
                self.state = RecordingSessionState::Idle;
                self.last_region = None;
                self.last_spool_path = None;
                self.lifecycle = RecordingLifecycle::Failed;
                return Err(RecordingSessionError::NotRecording);
            }
        };
        if !region.audit() && self.captured_frames > 0 {
            // A disk-backed capture may exceed the bounded UI preview. Keep a
            // finite marker region for the UI while the published WAV remains
            // the authoritative full-length take.
            region = RecordingPreviewRegion {
                sample_rate: self.preview.sample_rate(),
                channels: self.preview.channels(),
                start_sample: self.start_sample,
                samples: vec![0.0; self.preview.channels() as usize],
            };
        }
        if !region.audit() {
            let _ = std::fs::remove_file(spool_path);
            self.state = RecordingSessionState::Idle;
            self.lifecycle = RecordingLifecycle::Idle;
            return Err(RecordingSessionError::EmptyRecording);
        }
        if self.takes.len() == 16 {
            self.takes.remove(0);
            let old_path = self.take_paths.remove(0);
            let _ = std::fs::remove_file(old_path);
        }
        self.takes.push(region.clone());
        self.take_paths.push(spool_path.clone());
        self.active_take = self.takes.len() - 1;
        self.last_region = Some(region.clone());
        self.last_spool_path = Some(spool_path);
        self.state = RecordingSessionState::Stopped;
        self.lifecycle = RecordingLifecycle::Stopped;
        self.punch_out_sample = None;
        self.pending_punch_out_sample = None;
        self.auto_stop_requested = false;
        Ok(region)
    }

    pub fn last_region(&self) -> Option<&RecordingPreviewRegion> {
        self.last_region.as_ref()
    }

    pub fn take_count(&self) -> usize {
        self.takes.len()
    }

    pub fn captured_frame_count(&self) -> u64 {
        self.captured_frames
    }

    pub fn active_take(&self) -> usize {
        self.active_take
    }

    pub fn last_spool_path(&self) -> Option<&std::path::Path> {
        self.last_spool_path.as_deref()
    }

    pub fn rebase_last_spool_path(&mut self, path: PathBuf) -> bool {
        let Some(path_slot) = self.take_paths.get_mut(self.active_take) else {
            return false;
        };
        *path_slot = path.clone();
        self.last_spool_path = Some(path);
        true
    }

    pub fn select_take(&mut self, index: usize) -> bool {
        if self.state == RecordingSessionState::Recording || index >= self.takes.len() {
            return false;
        }
        self.active_take = index;
        self.last_region = self.takes.get(index).cloned();
        self.last_spool_path = self.take_paths.get(index).cloned();
        true
    }

    /// Returns a compact peak envelope for the current take. This is a UI
    /// snapshot path; it is never called from the audio callback.
    pub fn waveform_points(&self, point_count: usize) -> Vec<f32> {
        let samples = if self.state == RecordingSessionState::Recording {
            self.preview.samples()
        } else {
            self.last_region
                .as_ref()
                .map(|region| region.samples.as_slice())
                .unwrap_or_default()
        };
        let channels = self.preview.channels() as usize;
        if samples.is_empty() || channels == 0 || point_count == 0 {
            return Vec::new();
        }
        let frames = samples.len() / channels;
        let count = point_count.min(frames).max(1);
        (0..count)
            .map(|point| {
                let begin = point * frames / count;
                let end = ((point + 1) * frames / count).max(begin + 1).min(frames);
                let mut peak = 0.0f32;
                for frame in begin..end {
                    for channel in 0..channels {
                        peak = peak.max(samples[frame * channels + channel].abs());
                    }
                }
                peak.clamp(0.0, 1.0)
            })
            .collect()
    }

    pub fn audit(&self) -> bool {
        self.preview.audit()
            && self.takes.iter().all(RecordingPreviewRegion::audit)
            && self.take_paths.len() == self.takes.len()
            && self.take_paths.iter().all(|path| path.is_file())
            && (self.takes.is_empty() || self.active_take < self.takes.len())
            && self
                .last_region
                .as_ref()
                .is_none_or(RecordingPreviewRegion::audit)
            && match self.state {
                RecordingSessionState::Recording => self.preview.is_recording(),
                RecordingSessionState::Idle | RecordingSessionState::Stopped => {
                    !self.preview.is_recording()
                        && (self.state == RecordingSessionState::Idle
                            || self.last_region.as_ref().is_some_and(|region| {
                                self.takes.get(self.active_take) == Some(region)
                            }))
                }
            }
            && match self.lifecycle {
                RecordingLifecycle::Recording => {
                    self.state == RecordingSessionState::Recording && self.active_writer.is_some()
                }
                RecordingLifecycle::Finalizing => self.active_writer.is_some(),
                RecordingLifecycle::Idle | RecordingLifecycle::Armed => {
                    self.state != RecordingSessionState::Recording
                }
                RecordingLifecycle::Stopped
                | RecordingLifecycle::Committed
                | RecordingLifecycle::Recovered
                | RecordingLifecycle::Failed => self.state != RecordingSessionState::Recording,
            }
    }
}

#[cfg(test)]
#[path = "recording_session_tests.rs"]
mod recording_session_tests;
