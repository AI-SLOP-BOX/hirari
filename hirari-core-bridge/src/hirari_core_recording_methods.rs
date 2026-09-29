impl HirariCore {
    fn recording_take_regions(
        &self,
        published: &[(u32, PathBuf)],
    ) -> anyhow::Result<Vec<comping::TakeRegion>> {
        let layout = self
            .engine
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("audio engine is unavailable"))?
            .get_project_layout_json();
        let tracks: Vec<NativeLayoutTrack> = serde_json::from_str(&layout)
            .map_err(|error| anyhow::anyhow!("recorded region layout is invalid: {error}"))?;
        let mut links = Vec::with_capacity(published.len());
        for (track_id, path) in published {
            let path = path.to_string_lossy();
            let region = tracks
                .iter()
                .find(|track| track.id == *track_id)
                .and_then(|track| track.regions.iter().find(|region| region.path == path))
                .ok_or_else(|| anyhow::anyhow!("recorded audio region could not be located"))?;
            links.push(comping::TakeRegion {
                track_id: *track_id,
                region_id: region.id,
            });
        }
        Ok(links)
    }

    fn record_comping_history(&self, before: crate::comping::CompingOrchestrator) {
        let depth = self.engine.as_ref().map(|engine| engine.get_undo_count());
        self.record_comping_history_with_undo_depth(before, depth);
    }

    pub(crate) fn record_comping_history_with_undo_depth(
        &self,
        before: crate::comping::CompingOrchestrator,
        native_undo_depth: Option<u32>,
    ) {
        let after = self.comping_snapshot_json();
        let Ok(after) = serde_json::from_str::<crate::comping::CompingOrchestrator>(&after) else {
            return;
        };
        if before.takes == after.takes && before.current_comp == after.current_comp {
            return;
        }
        if let Ok(mut history) = self.comping_history.lock() {
            history.push(crate::CompingHistoryEntry { before, after, native_undo_depth });
        }
        if let Ok(mut redo) = self.comping_redo_history.lock() {
            redo.clear();
        }
    }

    /// Production-facing capture entry point. The legacy preview-named
    /// methods remain below as compatibility shims for older integrations.
    pub fn start_recording_capture(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
        start_sample: u64,
    ) -> anyhow::Result<()> {
        self.start_recording_preview(sample_rate, channels, max_frames, start_sample)
    }

    /// Starts capture after discarding exactly `count_in_frames` of driver
    /// input. This is the command-facing count-in path; the metronome remains
    /// a native audio signal while the recording spool opens only on the
    /// first post-count-in frame.
    pub fn start_recording_capture_with_count_in(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
        start_sample: u64,
        count_in_frames: u64,
    ) -> anyhow::Result<()> {
        self.discard_pending_recording_input();
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
                .start_with_count_in(start_sample, count_in_frames)
                .map_err(|error| anyhow::anyhow!("recording count-in failed: {error:?}"))?;
            return Ok(());
        }
        let mut session =
            recording_session::RecordingSession::try_new(sample_rate, channels, max_frames)
                .map_err(|error| anyhow::anyhow!("recording session setup failed: {error:?}"))?;
        session
            .start_with_count_in(start_sample, count_in_frames)
            .map_err(|error| anyhow::anyhow!("recording count-in failed: {error:?}"))?;
        *slot = Some(session);
        Ok(())
    }

    pub fn start_recording_capture_with_punch(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
        current_sample: u64,
        punch_in_sample: u64,
        punch_out_sample: u64,
    ) -> anyhow::Result<()> {
        self.discard_pending_recording_input();
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
                .start_with_punch(current_sample, punch_in_sample, punch_out_sample)
                .map_err(|error| anyhow::anyhow!("recording punch start failed: {error:?}"))?;
            return Ok(());
        }
        let mut session =
            recording_session::RecordingSession::try_new(sample_rate, channels, max_frames)
                .map_err(|error| anyhow::anyhow!("recording session setup failed: {error:?}"))?;
        session
            .start_with_punch(current_sample, punch_in_sample, punch_out_sample)
            .map_err(|error| anyhow::anyhow!("recording punch start failed: {error:?}"))?;
        *slot = Some(session);
        Ok(())
    }

    pub fn poll_recording_capture(&self) -> anyhow::Result<bool> {
        self.poll_recording_preview()
    }

    pub fn recording_capture_auto_stop_requested(&self) -> bool {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|session| session.auto_stop_requested()))
            .unwrap_or(false)
    }

    pub fn set_recording_capture_cycle_range(&self, range: Option<(u64, u64)>) -> bool {
        self.recording_session
            .lock()
            .ok()
            .and_then(|mut slot| slot.as_mut().map(|session| session.set_cycle_range(range)))
            .unwrap_or(false)
    }

    pub fn recording_capture_cycle_range(&self) -> Option<(u64, u64)> {
        self.recording_session
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(|session| session.cycle_range()))
    }

    pub fn commit_recording_capture_to_track(
        &self,
        track_id: u32,
        project_path: Option<&str>,
    ) -> anyhow::Result<usize> {
        self.commit_recording_preview_to_track(track_id, project_path)
    }

    /// Publishes each armed track's selected input channel set as one project
    /// edit. The original multichannel spool is retained as the recoverable
    /// source take until every region has been imported.
    pub fn commit_recording_capture_to_tracks(
        &self,
        track_input_channels: &[(u32, Vec<u16>)],
        project_path: Option<&str>,
    ) -> anyhow::Result<usize> {
        if track_input_channels.is_empty() || track_input_channels.len() > 32 {
            return Err(anyhow::anyhow!("recording target count is invalid"));
        }
        let mut tracks = std::collections::HashSet::with_capacity(track_input_channels.len());
        if track_input_channels.iter().any(|(track, inputs)| {
            *track == 0
                || !tracks.insert(*track)
                || !(1..=32).contains(&inputs.len())
                || inputs.iter().any(|input| *input >= 32)
                || inputs
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != inputs.len()
        }) {
            return Err(anyhow::anyhow!("multi-track recording route is invalid"));
        }

        let cycle_range = self.recording_capture_cycle_range();
        let region = self.stop_recording_preview()?;
        if !region.sample_rate.is_finite()
            || !(1.0..=384_000.0).contains(&region.sample_rate)
            || region.channels == 0
            || region.channels > 32
            || track_input_channels
                .iter()
                .any(|(_, inputs)| inputs.iter().any(|channel| *channel >= region.channels))
        {
            return Err(anyhow::anyhow!(
                "multi-track recording input format is invalid"
            ));
        }
        let captured_frame_count = self
            .recording_capture_frame_count()
            .max(region.frame_count() as u64);
        region
            .start_sample
            .checked_add(captured_frame_count)
            .ok_or_else(|| anyhow::anyhow!("recorded take sample range overflowed"))?;
        let source = self
            .recording_capture_spool_path()
            .filter(|path| std::path::Path::new(path).is_file())
            .ok_or_else(|| anyhow::anyhow!("multi-track recording spool is unavailable"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| anyhow::anyhow!("recording timestamp unavailable: {error}"))?
            .as_nanos();
        let output_dir = project_path
            .filter(|path| !path.trim().is_empty())
            .map(std::path::Path::new)
            .and_then(std::path::Path::parent)
            .map(|parent| parent.join("Audio Recordings"))
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&output_dir).map_err(|error| {
            anyhow::anyhow!(
                "recording directory unavailable ({}): {error}",
                output_dir.display()
            )
        })?;

        let mut published = Vec::with_capacity(track_input_channels.len());
        let mut temporary_channels = Vec::with_capacity(track_input_channels.len());
        let split_result = (|| -> anyhow::Result<()> {
            use std::io::{Read, Seek, SeekFrom};

            let mut source_file = std::fs::File::open(&source)?;
            let mut header = [0u8; 80];
            source_file.read_exact(&mut header)?;
            if (&header[0..4] != b"RIFF" && &header[0..4] != b"RF64")
                || &header[8..12] != b"WAVE"
                || &header[48..52] != b"fmt "
                || &header[72..76] != b"data"
                || !matches!(
                    (
                        u16::from_le_bytes([header[56], header[57]]),
                        u16::from_le_bytes([header[70], header[71]])
                    ),
                    (1, 16) | (3, 32)
                )
            {
                anyhow::bail!("multi-track recording spool format is unsupported");
            }
            let source_format = u16::from_le_bytes([header[56], header[57]]);
            let source_channels = u16::from_le_bytes([header[58], header[59]]);
            let sample_rate = u32::from_le_bytes([header[60], header[61], header[62], header[63]]);
            let block_align = u16::from_le_bytes([header[68], header[69]]);
            let bits_per_sample = u16::from_le_bytes([header[70], header[71]]);
            let data_bytes = if &header[0..4] == b"RF64" {
                u64::from_le_bytes(header[28..36].try_into().unwrap())
            } else {
                u32::from_le_bytes(header[76..80].try_into().unwrap()) as u64
            };
            let bytes_per_sample = match (source_format, bits_per_sample) {
                (1, 16) => 2usize,
                (3, 32) => 4usize,
                _ => 0,
            };
            let expected_align = source_channels
                .checked_mul(bytes_per_sample as u16)
                .unwrap_or(0);
            if source_channels != region.channels
                || !(8_000..=384_000).contains(&sample_rate)
                || block_align != expected_align
                || data_bytes == 0
                || data_bytes % u64::from(block_align.max(1)) != 0
                || source_file.metadata()?.len() < 80u64.saturating_add(data_bytes)
            {
                anyhow::bail!("multi-track recording spool metadata is invalid");
            }
            let frame_count = data_bytes / u64::from(block_align);
            source_file.seek(SeekFrom::Start(80))?;

            let mut writers = Vec::with_capacity(track_input_channels.len());
            for (route_index, (track_id, input_channels)) in track_input_channels.iter().enumerate()
            {
                let path = output_dir.join(format!(
                    "hirari-recording-{}-track-{}-inputs-{}-{stamp}-{route_index}.wav",
                    std::process::id(),
                    track_id,
                    input_channels
                        .iter()
                        .map(|channel| (channel + 1).to_string())
                        .collect::<Vec<_>>()
                        .join("-")
                ));
                let writer = crate::recording_stream::StreamingRecordingWriter::create_float32(
                    &path,
                    sample_rate,
                    input_channels.len() as u16,
                )
                .map_err(|error| {
                    anyhow::anyhow!("recording channel spool setup failed: {error:?}")
                })?;
                let mut temporary_path = path.clone();
                temporary_path.set_extension("wav.part");
                temporary_channels.push(temporary_path);
                writers.push((
                    *track_id,
                    path,
                    input_channels
                        .iter()
                        .map(|channel| *channel as usize)
                        .collect::<Vec<_>>(),
                    writer,
                ));
            }

            const CHUNK_FRAMES: usize = 4096;
            let source_frame_bytes = source_channels as usize * bytes_per_sample;
            let mut source_bytes = vec![0u8; CHUNK_FRAMES * source_frame_bytes];
            let mut channel_samples = writers
                .iter()
                .map(|writer| Vec::<u8>::with_capacity(CHUNK_FRAMES * writer.2.len() * 4))
                .collect::<Vec<_>>();
            let mut frames_left = frame_count;
            while frames_left > 0 {
                let frames = frames_left.min(CHUNK_FRAMES as u64) as usize;
                let byte_count = frames * source_frame_bytes;
                source_file.read_exact(&mut source_bytes[..byte_count])?;
                for route_index in 0..writers.len() {
                    let input_channels = &writers[route_index].2;
                    let samples = &mut channel_samples[route_index];
                    samples.clear();
                    for frame in 0..frames {
                        for input_channel in input_channels {
                            let offset = (frame * source_channels as usize + input_channel)
                                * bytes_per_sample;
                            if bytes_per_sample == 2 {
                                let pcm = i16::from_le_bytes([
                                    source_bytes[offset],
                                    source_bytes[offset + 1],
                                ]);
                                samples.extend_from_slice(&(pcm as f32 / 32768.0).to_le_bytes());
                            } else {
                                samples.extend_from_slice(&source_bytes[offset..offset + 4]);
                            }
                        }
                    }
                    writers[route_index]
                        .3
                        .append_float32_interleaved(samples)
                        .map_err(|error| {
                            anyhow::anyhow!("recording channel write failed: {error:?}")
                        })?;
                }
                frames_left -= frames as u64;
            }

            for (track_id, path, _input_channels, writer) in writers {
                let final_path = writer.finalize().map_err(|error| {
                    anyhow::anyhow!("recording channel publish failed: {error:?}")
                })?;
                published.push((track_id, final_path));
                let _ = path;
            }
            Ok(())
        })();
        if let Err(error) = split_result {
            for (_, path) in published.drain(..) {
                let _ = std::fs::remove_file(path);
            }
            for path in temporary_channels.drain(..) {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }

        let comping_before = serde_json::from_str::<comping::CompingOrchestrator>(
            &self.comping_snapshot_json(),
        ).map_err(|error| anyhow::anyhow!("comping snapshot is invalid before recording: {error}"))?;
        self.begin_undo_transaction("Record Audio Tracks");
        let mut imported = true;
        for (track_id, path) in &published {
            if !self.engine.as_ref().is_some_and(|engine| {
                engine.add_region(
                    *track_id,
                    path.to_string_lossy().as_ref(),
                    region.start_sample as f64,
                )
            }) {
                imported = false;
                break;
            }
        }
        if !imported {
            // Keep the media if rollback did not confirm, so an unexpectedly
            // retained region can never become a dangling reference.
            if self.abort_undo_transaction() {
                for (_, path) in published {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(anyhow::anyhow!(
                "one or more recorded tracks could not be imported"
            ));
        }
        let initial_regions = match self.recording_take_regions(&published) {
            Ok(regions) if regions.len() == published.len() => regions,
            Ok(_) => {
                if self.abort_undo_transaction() {
                    for (_, path) in published {
                        let _ = std::fs::remove_file(path);
                    }
                }
                return Err(anyhow::anyhow!(
                    "recorded audio regions could not be linked to the take"
                ));
            }
            Err(error) => {
                if self.abort_undo_transaction() {
                    for (_, path) in published {
                        let _ = std::fs::remove_file(path);
                    }
                }
                return Err(error);
            }
        };
        let cycle_lengths = if let Some((cycle_start, cycle_end)) = cycle_range {
            // Cycle playback can begin before the left locator. That first
            // recording pass still runs to the right locator before looping
            // back, so retain the pre-cycle section in the first take and
            // split every later pass at the cycle span.
            if cycle_start < cycle_end && region.start_sample < cycle_end {
                let first = (cycle_end - region.start_sample).min(captured_frame_count);
                let span = cycle_end - cycle_start;
                let mut lengths = vec![first];
                let mut remaining = captured_frame_count - first;
                while remaining > 0 {
                    if lengths.len() >= 65_536 {
                        let _ = self.abort_undo_transaction();
                        return Err(anyhow::anyhow!(
                            "cycle recording produced too many takes to commit safely"
                        ));
                    }
                    let length = remaining.min(span);
                    lengths.push(length);
                    remaining -= length;
                }
                lengths
            } else {
                vec![captured_frame_count]
            }
        } else {
            vec![captured_frame_count]
        };
        let mut region_ids_by_pass = vec![vec![0u32; published.len()]; cycle_lengths.len()];
        for (track_index, initial) in initial_regions.iter().enumerate() {
            let mut tail_region_id = initial.region_id;
            let mut offset = cycle_lengths[0];
            region_ids_by_pass[0][track_index] = initial.region_id;
            for pass in 1..cycle_lengths.len() {
                let Some(split_sample) = region.start_sample.checked_add(offset) else {
                    let _ = self.abort_undo_transaction();
                    return Err(anyhow::anyhow!("cycle recording sample range overflowed"));
                };
                let Some((track_id, _)) = published.get(track_index) else {
                    let _ = self.abort_undo_transaction();
                    return Err(anyhow::anyhow!("cycle recording target was lost"));
                };
                let right_region_id = self.split_region_at_sample_with_right_id(
                    *track_id,
                    tail_region_id,
                    split_sample,
                );
                if right_region_id == 0 {
                    let _ = self.abort_undo_transaction();
                    return Err(anyhow::anyhow!("cycle take lane could not be split"));
                }
                region_ids_by_pass[pass][track_index] = right_region_id;
                tail_region_id = right_region_id;
                offset = offset.saturating_add(cycle_lengths[pass]);
            }
        }
        if let Some((cycle_start, _)) = cycle_range {
            for pass in 1..cycle_lengths.len() {
                for (track_index, (track_id, _)) in published.iter().enumerate() {
                    if !self.move_region_to_sample(
                        *track_id,
                        region_ids_by_pass[pass][track_index],
                        cycle_start,
                    ) {
                        let _ = self.abort_undo_transaction();
                        return Err(anyhow::anyhow!("cycle take lane could not be positioned"));
                    }
                }
            }
        }
        let take_regions_by_pass = region_ids_by_pass
            .iter()
            .map(|pass_regions| {
                published
                    .iter()
                    .zip(pass_regions)
                    .map(|((track_id, _), region_id)| comping::TakeRegion {
                        track_id: *track_id,
                        region_id: *region_id,
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let first_take_id = self.next_comp_take_id();
        if first_take_id == 0 {
            if self.abort_undo_transaction() {
                for (_, path) in published {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(anyhow::anyhow!("recorded take identity space is exhausted"));
        }
        let mut recorded_takes = Vec::with_capacity(cycle_lengths.len());
        for (pass, length) in cycle_lengths.iter().enumerate() {
            let take_start = if pass == 0 {
                region.start_sample
            } else {
                cycle_range.map_or(region.start_sample, |(start, _)| start)
            };
            let Some(take_end) = take_start.checked_add(*length) else {
                if self.abort_undo_transaction() {
                    for (_, path) in published {
                        let _ = std::fs::remove_file(path);
                    }
                }
                return Err(anyhow::anyhow!("recorded take sample range overflowed"));
            };
            let Some(take_id) = first_take_id.checked_add(pass as u32) else {
                if self.abort_undo_transaction() {
                    for (_, path) in published {
                        let _ = std::fs::remove_file(path);
                    }
                }
                return Err(anyhow::anyhow!("recorded take identity space is exhausted"));
            };
            recorded_takes.push(comping::Take {
                id: take_id,
                name: format!("Take {take_id}"),
                start_sample: take_start,
                end_sample: take_end,
                regions: take_regions_by_pass[pass].clone(),
            });
        }
        if !self.register_recording_cycle_takes(&recorded_takes) {
            if self.abort_undo_transaction() {
                for (_, path) in published {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(anyhow::anyhow!("recorded cycle takes could not be registered atomically"));
        }
        if !self.end_undo_transaction() {
            let _ = self.restore_comping_snapshot_json(
                &serde_json::to_string(&comping_before).unwrap_or_default(),
            );
            if self.abort_undo_transaction() {
                for (_, path) in published {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(anyhow::anyhow!("multi-track recording undo transaction failed"));
        }
        let undo_depth = self.engine.as_ref().map(|engine| engine.get_undo_count());
        self.record_comping_history_with_undo_depth(comping_before, undo_depth);
        if let Ok(mut slot) = self.recording_session.lock() {
            if let Some(session) = slot.as_mut() {
                let _ = session.mark_committed();
            }
        }
        Ok(captured_frame_count.min(usize::MAX as u64) as usize)
    }

    pub fn recording_capture_waveform(&self, point_count: usize) -> Vec<f32> {
        self.recording_preview_waveform(point_count)
    }

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
    pub fn start_recording_preview(
        &self,
        sample_rate: f32,
        channels: u16,
        max_frames: usize,
        start_sample: u64,
    ) -> anyhow::Result<()> {
        self.discard_pending_recording_input();
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

    /// Polls one non-realtime input block from the CoreAudio queue and appends
    /// it to the active recording session. No Rust code runs on the audio
    /// callback; this method is intended for a worker/UI timer.
    pub fn poll_recording_preview(&self) -> anyhow::Result<bool> {
        if self.is_silent_audio_fallback() {
            return Err(anyhow::anyhow!(
                "native audio input is unavailable; use an audio device before polling recording"
            ));
        }
        let mut block_channels = 0;
        let mut dropped_blocks = 0;
        let input = self.engine.as_ref().map_or_else(Vec::new, |engine| {
            engine.poll_audio_input(&mut block_channels, &mut dropped_blocks)
        });
        if !input.is_empty() {
            self.append_recording_preview_block(input.as_slice(), block_channels as u16)?;
        }
        if dropped_blocks > 0 {
            anyhow::bail!(
                "audio input queue dropped {dropped_blocks} block(s); capture stopped at the last contiguous input"
            );
        }
        Ok(!input.is_empty())
    }

    fn discard_pending_recording_input(&self) {
        if let Some(engine) = self.engine.as_ref() {
            engine.discard_pending_audio_input();
        }
    }

    fn append_recording_preview_block(
        &self,
        input: &[f32],
        block_channels: u16,
    ) -> anyhow::Result<()> {
        let mut slot = self
            .recording_session
            .lock()
            .map_err(|_| anyhow::anyhow!("recording session lock poisoned"))?;
        let session = slot
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("recording session is not active"))?;
        if block_channels == 0 || session.channel_count() != block_channels {
            return Err(anyhow::anyhow!(
                "audio input channel layout changed during recording"
            ));
        }
        session
            .append_interleaved(input)
            .map_err(|error| anyhow::anyhow!("recording input rejected: {error:?}"))
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
            .engine
            .as_ref()
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
        let imported = self.engine.as_ref().is_some_and(|engine| {
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
include!("hirari_core_comping_methods.rs");
