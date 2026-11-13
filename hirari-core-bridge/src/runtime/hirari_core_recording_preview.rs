//! Recording preview/arm/recovery entry points, split out of
//! hirari_core_recording_methods.rs.
use super::*;

impl HirariCore {
    /// Prepares a recording session without opening the spool writer yet.
    /// This makes the Armed state observable and lets the UI validate the
    /// capture format before audio input is accepted.
    pub fn arm_recording_capture(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
    ) -> anyhow::Result<()> {
        let mut slot = self
            .recording_session
            .lock()
            .map_err(|_| anyhow::anyhow!("recording session lock poisoned"))?;
        if let Some(session) = slot.as_mut() {
            if !session.configuration_matches(sample_rate, channels, max_frames) {
                return Err(anyhow::anyhow!(
                    "recording format changed; restart the recording session"
                ));
            }
            if !session.arm() {
                return Err(anyhow::anyhow!("recording session cannot be armed"));
            }
            return Ok(());
        }
        let mut session =
            recording_session::RecordingSession::try_new(sample_rate, channels, max_frames)
                .map_err(|error| anyhow::anyhow!("recording session setup failed: {error:?}"))?;
        if !session.arm() {
            return Err(anyhow::anyhow!("recording session cannot be armed"));
        }
        *slot = Some(session);
        Ok(())
    }

    /// Starts a bounded preview recording session. Audio input blocks must be
    /// supplied by the platform driver through `append_recording_preview`.
    /// The UI must not mark recording active until this method succeeds.
    /// Measured input latency in sample frames (device + unit properties
    /// plus one callback buffer), or zero when unmeasured. Recordings
    /// arriving through the input path are late by this much.
    pub(crate) fn input_latency_frames(&self, sample_rate: f32) -> u64 {
        let seconds = self.io_latency_seconds();
        if seconds.is_finite() && sample_rate.is_finite() && sample_rate > 0.0 {
            (seconds * f64::from(sample_rate)).round().max(0.0) as u64
        } else {
            0
        }
    }

    pub fn start_recording_preview(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
        start_sample: u64,
    ) -> anyhow::Result<()> {
        self.ensure_recorder_poller();
        self.discard_pending_recording_input();
        // Input audio arrives late by the measured I/O latency; start the
        // session early so captured regions align with the playhead.
        let start_sample = start_sample.saturating_sub(self.input_latency_frames(sample_rate));
        let mut slot = self
            .recording_session
            .lock()
            .map_err(|_| anyhow::anyhow!("recording session lock poisoned"))?;
        if let Some(session) = slot.as_mut() {
            if !session.configuration_matches(sample_rate, channels, max_frames) {
                return Err(anyhow::anyhow!(
                    "recording format changed; restart the recording session"
                ));
            }
            session
                .start(start_sample)
                .map_err(|error| anyhow::anyhow!("recording session start failed: {error:?}"))?;
            return Ok(());
        }
        let mut session =
            recording_session::RecordingSession::try_new(sample_rate, channels, max_frames)
                .map_err(|error| anyhow::anyhow!("recording session setup failed: {error:?}"))?;
        session
            .start(start_sample)
            .map_err(|error| anyhow::anyhow!("recording session start failed: {error:?}"))?;
        *slot = Some(session);
        Ok(())
    }

    /// Appends a validated interleaved block to the active preview session.
    /// This bridge method is intentionally explicit and should be fed by a
    /// preallocated driver queue rather than called directly from a callback
    /// that cannot tolerate a mutex.
    pub fn append_recording_preview(&self, input: &[f32]) -> anyhow::Result<()> {
        let mut slot = self
            .recording_session
            .lock()
            .map_err(|_| anyhow::anyhow!("recording session lock poisoned"))?;
        let session = slot
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("recording session is not active"))?;
        session
            .append_interleaved(input)
            .map_err(|error| anyhow::anyhow!("recording input rejected: {error:?}"))
    }

    /// Polls one non-realtime input block from the driver queue and appends it
    /// to the active recording session. No Rust code runs on the audio
    /// callback; this method is intended for a worker/UI timer. The block comes
    /// straight from `audio_driver_host`, which owns the planar scratch and the
    /// interleaving, so no C++ buffer is involved.
    pub fn poll_recording_preview(&self) -> anyhow::Result<bool> {
        // The dedicated poller owns the input queue once started; calling
        // into it here as well would split blocks across two consumers.
        if self
            .recorder_poller_started
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(false);
        }
        if self.is_silent_audio_fallback() {
            return Err(anyhow::anyhow!(
                "native audio input is unavailable; use an audio device before polling recording"
            ));
        }
        recorder_poll_once(
            &self.recording_session,
            self.driver_handle(),
            &std::sync::atomic::AtomicBool::new(false),
        )
    }

    pub(crate) fn discard_pending_recording_input(&self) {
        crate::audio_driver_host::discard_pending_input(self.driver_handle());
    }

    /// Stops the preview session and returns the immutable captured region.
    pub fn stop_recording_preview(
        &self,
    ) -> anyhow::Result<recording_session::RecordingPreviewRegion> {
        let mut slot = self
            .recording_session
            .lock()
            .map_err(|_| anyhow::anyhow!("recording session lock poisoned"))?;
        let session = slot
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("recording session is not active"))?;
        session
            .stop()
            .map_err(|error| anyhow::anyhow!("recording session stop failed: {error:?}"))
    }

    /// Commits the stopped preview recording as a normal audio region.
    ///
    /// This deliberately runs outside the audio callback: PCM conversion and
    /// file I/O are bounded by the preview length and then the existing native
    /// decoder/import path owns the region source. The temporary WAV is kept
    /// in the project-side `Audio Recordings` directory when a project path is
    /// available, otherwise in the OS temporary directory.
    pub fn commit_recording_preview_to_track(
        &self,
        track_id: u32,
        project_path: Option<&str>,
    ) -> anyhow::Result<usize> {
        let region = self.stop_recording_preview()?;
        let captured_frame_count = self
            .recording_capture_frame_count()
            .max(region.frame_count() as u64);
        let sample_rate = region.sample_rate;
        let channels = region.channels;
        if !sample_rate.is_finite() || !(1.0..=384_000.0).contains(&sample_rate) {
            return Err(anyhow::anyhow!("recording sample rate is invalid"));
        }
        if !(1..=32).contains(&channels) {
            return Err(anyhow::anyhow!("recording channel count is invalid"));
        }
        if region.samples.is_empty() {
            return Err(anyhow::anyhow!("recording contains no audio frames"));
        }

        // The disk spool is the authoritative capture when available. This
        // avoids copying the complete take again merely to publish it as a
        // project region; the bounded preview remains only for UI feedback.
        let spool_path = self
            .recording_capture_spool_path()
            .filter(|path| Path::new(path).is_file());

        let sequence = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| anyhow::anyhow!("recording timestamp unavailable: {error}"))?
            .as_nanos();
        let file_name = format!("hirari-recording-{}-{sequence}.wav", std::process::id());
        let path: PathBuf = if let (Some(spool), None) = (spool_path.as_deref(), project_path) {
            PathBuf::from(spool)
        } else {
            project_path
                .filter(|path| !path.trim().is_empty())
                .map(Path::new)
                .and_then(Path::parent)
                .map(|parent| parent.join("Audio Recordings"))
                .map(|directory| {
                    std::fs::create_dir_all(&directory)
                        .map(|()| directory.join(&file_name))
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "recording directory unavailable ({}): {error}",
                                directory.display()
                            )
                        })
                })
                .transpose()?
                .unwrap_or_else(|| std::env::temp_dir().join(file_name))
        };
        if let Some(source) = spool_path {
            if source != path.to_string_lossy() {
                std::fs::rename(&source, &path)
                    .map_err(|error| anyhow::anyhow!("recording spool publish failed: {error}"))?;
                if let Ok(mut slot) = self.recording_session.lock() {
                    if let Some(session) = slot.as_mut() {
                        let _ = session.rebase_last_spool_path(path.clone());
                    }
                }
            }
        } else {
            export::write_wav_float32(&path, &region.samples, sample_rate.round() as u32, channels)
                .map_err(|error| anyhow::anyhow!("recording WAV write failed: {error:?}"))?;
        }

        let start_sample = region.start_sample as f64;
        let imported = self
            .engine_opt()
            .map(|engine| {
                engine.add_region(track_id, path.to_string_lossy().as_ref(), start_sample)
            })
            .unwrap_or(false);
        if !imported {
            let _ = std::fs::remove_file(&path);
            return Err(anyhow::anyhow!("recording region import failed"));
        }
        if let Ok(mut slot) = self.recording_session.lock() {
            if let Some(session) = slot.as_mut() {
                let _ = session.mark_committed();
            }
        }
        Ok(captured_frame_count.min(usize::MAX as u64) as usize)
    }

    pub fn recording_preview_active(&self) -> bool {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| {
                slot.as_ref().map(|session| {
                    session.state() == recording_session::RecordingSessionState::Recording
                })
            })
            .unwrap_or(false)
    }

    pub fn recording_preview_waveform(&self, point_count: usize) -> Vec<f32> {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| {
                slot.as_ref()
                    .map(|session| session.waveform_points(point_count))
            })
            .unwrap_or_default()
    }

    pub fn recording_capture_spool_path(&self) -> Option<String> {
        self.recording_session.lock().ok().and_then(|slot| {
            slot.as_ref()
                .and_then(|session| session.last_spool_path())
                .map(|path| path.to_string_lossy().into_owned())
        })
    }

    pub fn recording_recovery_candidates_json(&self) -> String {
        let candidates = recording_stream::recoverable_spools(std::env::temp_dir())
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn recover_recording_spool_to_track(
        &self,
        spool_path: &str,
        track_id: u32,
    ) -> anyhow::Result<u64> {
        let requested = std::path::Path::new(spool_path);
        let temp_root = std::fs::canonicalize(std::env::temp_dir())?;
        let source = requested
            .canonicalize()
            .ok()
            .filter(|candidate| candidate.starts_with(&temp_root))
            .filter(|candidate| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".wav.part"))
            })
            .filter(|candidate| candidate.is_file())
            .ok_or_else(|| anyhow::anyhow!("recording recovery candidate is invalid"))?;
        let bytes = std::fs::read(&source)?;
        let (_channels, data_bytes) = recording_stream::recording_wav_metadata(&bytes)
            .ok_or_else(|| anyhow::anyhow!("recording recovery candidate is empty"))?;
        let frame_bytes = u64::from(
            recording_stream::recording_wav_frame_bytes(&bytes)
                .ok_or_else(|| anyhow::anyhow!("recording recovery frame format is invalid"))?,
        );
        let frames = data_bytes / frame_bytes;
        let recovered = std::env::temp_dir().join(format!(
            "hirari-recovered-recording-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| anyhow::anyhow!("clock unavailable: {error}"))?
                .as_nanos()
        ));
        std::fs::copy(&source, &recovered)?;
        let imported = self.engine_opt().is_some_and(|engine| {
            engine.add_region(track_id, recovered.to_string_lossy().as_ref(), 0.0)
        });
        if !imported {
            let _ = std::fs::remove_file(&recovered);
            return Err(anyhow::anyhow!("recovered recording could not be imported"));
        }
        Ok(frames)
    }

    pub fn recording_capture_frame_count(&self) -> u64 {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|session| session.captured_frame_count()))
            .unwrap_or(0)
    }

    /// Start sample of the most recently finalized capture. This remains
    /// available after the take is committed so arrangement and comping
    /// metadata can share the exact same timeline origin.
    pub fn recording_capture_start_sample(&self) -> u64 {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| {
                slot.as_ref()
                    .and_then(|session| session.last_region())
                    .map(|region| region.start_sample)
            })
            .unwrap_or(0)
    }

    pub fn recording_lifecycle(&self) -> recording_session::RecordingLifecycle {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|session| session.lifecycle()))
            .unwrap_or(recording_session::RecordingLifecycle::Idle)
    }

    pub fn recording_lifecycle_label(&self) -> &'static str {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|session| session.lifecycle_label()))
            .unwrap_or("Idle")
    }

    pub fn recording_take_count(&self) -> usize {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|session| session.take_count()))
            .unwrap_or(0)
    }

    pub fn active_recording_take(&self) -> usize {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|session| session.active_take()))
            .unwrap_or(0)
    }

    pub fn select_recording_take(&self, index: usize) -> bool {
        self.recording_session
            .lock()
            .ok()
            .and_then(|mut slot| slot.as_mut().map(|session| session.select_take(index)))
            .unwrap_or(false)
    }

    pub fn select_recording_take_diagnostic_json(&self, index: usize) -> String {
        let result = match self.recording_session.lock() {
            Err(_) => crate::bridge_error::BridgeError::new(
                "recording_session_unavailable",
                "recording session lock is poisoned",
            )
            .retryable(true),
            Ok(mut slot) => match slot.as_mut() {
                None => crate::bridge_error::BridgeError::new(
                    "recording_session_inactive",
                    "no recording session is active",
                ),
                Some(session) => {
                    if session.select_take(index) {
                        return format!("{{\"ok\":true,\"take_index\":{index}}}");
                    }
                    crate::bridge_error::BridgeError::new(
                        "recording_take_not_found",
                        "recording take index was rejected",
                    )
                    .object(format!("recording-take:{index}"))
                }
            },
        };
        serde_json::to_string(&result)
            .unwrap_or_else(|_| "{\"code\":\"diagnostic_serialization_failed\"}".to_owned())
    }
}
