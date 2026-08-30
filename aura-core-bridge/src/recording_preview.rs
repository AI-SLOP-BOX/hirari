//! Minimal, deterministic audio-recording path for the preview build.
//!
//! `RecordingPreview` owns a preallocated interleaved buffer. Callers should
//! construct it before starting the audio device; `append_interleaved` only
//! validates and copies into already reserved capacity, so a normal callback
//! path does not need to allocate.

/// Hard upper bound for the interleaved sample buffer (64 MiB of `f32`).
pub const MAX_PREVIEW_SAMPLES: usize = 16_777_216;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingPreviewError {
    InvalidSampleRate,
    InvalidChannelCount,
    InvalidMaximumLength,
    MaximumLengthTooLarge,
    InputIsNotInterleaved,
    InputContainsNonFiniteSample,
    NotRecording,
    BufferCapacityInsufficient,
}

/// Immutable region-like result returned after stopping a preview recording.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingPreviewRegion {
    pub sample_rate: f32,
    pub channels: u16,
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

impl RecordingPreviewRegion {
    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn is_finite(&self) -> bool {
        self.samples.iter().all(|sample| sample.is_finite())
    }

    pub fn audit(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.channels > 0
            && !self.samples.is_empty()
            && self.samples.len().is_multiple_of(self.channels as usize)
            && self.is_finite()
    }
}

/// Preallocated preview recorder for interleaved `f32` input.
pub struct RecordingPreview {
    sample_rate: f32,
    channels: u16,
    max_frames: usize,
    samples: Vec<f32>,
    start_sample: u64,
    recording: bool,
}

impl RecordingPreview {
    /// Allocates the complete maximum buffer up front. Call this outside the
    /// real-time callback, before recording starts.
    pub fn try_new(
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
    ) -> Result<Self, RecordingPreviewError> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(RecordingPreviewError::InvalidSampleRate);
        }
        if channels == 0 {
            return Err(RecordingPreviewError::InvalidChannelCount);
        }
        if max_frames == 0 {
            return Err(RecordingPreviewError::InvalidMaximumLength);
        }

        let capacity = max_frames
            .checked_mul(channels as usize)
            .ok_or(RecordingPreviewError::MaximumLengthTooLarge)?;
        if capacity > MAX_PREVIEW_SAMPLES {
            return Err(RecordingPreviewError::MaximumLengthTooLarge);
        }

        let mut samples = Vec::new();
        samples
            .try_reserve_exact(capacity)
            .map_err(|_| RecordingPreviewError::MaximumLengthTooLarge)?;

        Ok(Self {
            sample_rate,
            channels,
            max_frames,
            samples,
            start_sample: 0,
            recording: false,
        })
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Starts a new take without reallocating the backing buffer.
    pub fn start(&mut self, start_sample: u64) {
        self.samples.clear();
        self.start_sample = start_sample;
        self.recording = true;
    }

    /// Appends one interleaved block. This method never grows the vector: the
    /// constructor reserves the full capacity and insufficient capacity is a
    /// recoverable error instead of an allocation in the callback.
    pub fn append_interleaved(&mut self, input: &[f32]) -> Result<(), RecordingPreviewError> {
        if !self.recording {
            return Err(RecordingPreviewError::NotRecording);
        }
        if !input.len().is_multiple_of(self.channels as usize) {
            return Err(RecordingPreviewError::InputIsNotInterleaved);
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err(RecordingPreviewError::InputContainsNonFiniteSample);
        }
        if self.frame_count() + input.len() / self.channels as usize > self.max_frames {
            return Err(RecordingPreviewError::BufferCapacityInsufficient);
        }
        if self.samples.len() + input.len() > self.samples.capacity() {
            return Err(RecordingPreviewError::BufferCapacityInsufficient);
        }

        self.samples.extend_from_slice(input);
        Ok(())
    }

    /// Stops recording and returns a non-destructive snapshot of the take.
    /// The recorder remains reusable; the returned region owns its copy.
    pub fn stop(&mut self) -> Option<RecordingPreviewRegion> {
        if !self.recording {
            return None;
        }
        self.recording = false;
        Some(self.snapshot())
    }

    /// Returns a non-destructive snapshot without changing recording state.
    pub fn snapshot(&self) -> RecordingPreviewRegion {
        RecordingPreviewRegion {
            sample_rate: self.sample_rate,
            channels: self.channels,
            start_sample: self.start_sample,
            samples: self.samples.clone(),
        }
    }

    /// Borrow the current recorded samples without copying.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn audit(&self) -> bool {
        self.channels > 0
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.samples.len().is_multiple_of(self.channels as usize)
            && self.frame_count() <= self.max_frames
            && self.samples.iter().all(|sample| sample.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordingPreview, RecordingPreviewError, MAX_PREVIEW_SAMPLES};

    #[test]
    fn records_interleaved_input_and_returns_non_destructive_region() {
        let mut recorder = RecordingPreview::try_new(48_000.0, 2, 8).unwrap();
        recorder.start(1234);
        recorder
            .append_interleaved(&[0.1, -0.1, 0.2, -0.2])
            .unwrap();

        let live = recorder.snapshot();
        assert_eq!(live.start_sample, 1234);
        assert_eq!(live.frame_count(), 2);
        assert!(recorder.is_recording());

        let stopped = recorder.stop().unwrap();
        assert_eq!(stopped.samples, live.samples);
        assert!(!recorder.is_recording());
        assert!(recorder.audit());
    }

    #[test]
    fn rejects_invalid_input_and_capacity_without_growing() {
        let mut recorder = RecordingPreview::try_new(44_100.0, 2, 2).unwrap();
        recorder.start(0);
        assert_eq!(
            recorder.append_interleaved(&[0.0]),
            Err(RecordingPreviewError::InputIsNotInterleaved)
        );
        assert_eq!(
            recorder.append_interleaved(&[0.0, f32::NAN]),
            Err(RecordingPreviewError::InputContainsNonFiniteSample)
        );
        recorder.append_interleaved(&[0.0, 0.0, 0.0, 0.0]).unwrap();
        assert_eq!(
            recorder.append_interleaved(&[0.0, 0.0]),
            Err(RecordingPreviewError::BufferCapacityInsufficient)
        );
        assert_eq!(recorder.samples().len(), 4);
    }

    #[test]
    fn stop_is_idempotent_and_recorder_can_be_reused_without_reallocation() {
        let mut recorder = RecordingPreview::try_new(48_000.0, 1, 4).unwrap();
        let capacity = recorder.samples.capacity();
        recorder.start(10);
        recorder.append_interleaved(&[0.5]).unwrap();
        assert!(recorder.stop().is_some());
        assert!(recorder.stop().is_none());
        recorder.start(20);
        recorder.append_interleaved(&[0.25, 0.0]).unwrap();
        assert_eq!(recorder.samples.capacity(), capacity);
        assert_eq!(recorder.stop().unwrap().start_sample, 20);
    }

    #[test]
    fn constructor_rejects_invalid_configuration() {
        assert!(RecordingPreview::try_new(0.0, 2, 4).is_err());
        assert!(RecordingPreview::try_new(48_000.0, 0, 4).is_err());
        assert!(RecordingPreview::try_new(48_000.0, 2, 0).is_err());
        assert!(RecordingPreview::try_new(48_000.0, 2, MAX_PREVIEW_SAMPLES).is_err());
    }
}
