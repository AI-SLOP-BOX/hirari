//! TrackRuntime pipeline methods, split out of track_runtime.rs.
use super::*;

impl TrackRuntime {
    /// The engine's block walk addresses a track through its runtime, so the
    /// identity and the tap buffers it publishes have to be readable natively.
    pub(crate) fn track_id(&self) -> u32 {
        self.track_id
    }

    /// Refreshes the sidechain engine handle used for sandbox delivery in
    /// the next track pass. Null disables sidechain input.
    pub(crate) fn set_sidechain_gate(&self, gate: *mut c_void) {
        self.sidechain_gate.store(gate, Ordering::Release);
    }

    /// The work buffer the graph-visible stages read at one block length. A
    /// buffer that cannot be addressed as stereo at this length publishes
    /// nothing, which is the same gate the C++ `canProcess` check applied.
    pub(crate) fn work(&self, frames: u32) -> Option<Tap> {
        self.window(self.work, frames)
    }

    /// The pre-fader tap: one Send edge's pre-fader source and the same buffer
    /// the post-FX sidechain reads.
    pub(crate) fn pre_fader(&self, frames: u32) -> Option<Tap> {
        self.window(self.pre_fader, frames)
    }

    /// The pre-insert tap: the offline render tap and the pre-FX sidechain.
    pub(crate) fn pre_insert(&self, frames: u32) -> Option<Tap> {
        self.window(self.pre_insert, frames)
    }

    pub(crate) fn send_pre(&self, frames: u32) -> Option<Tap> {
        self.window(self.send_pre, frames)
    }

    pub(crate) fn send_post(&self, frames: u32) -> Option<Tap> {
        self.window(self.send_post, frames)
    }

    /// The work-buffer channel table one routing destination publishes.
    pub(crate) fn work_channels(&self, frames: u32) -> (*const *mut f32, u32) {
        self.tap(self.work, frames)
    }

    /// The manual timing offset the fanout delays this track's master route by.
    pub(crate) fn delay_samples(&self) -> u32 {
        self.track_delay_samples.load(Ordering::Relaxed)
    }

    /// Publishes this block's tempo and playback flag, then empties the work
    /// buffer so upstream tracks can accumulate into it. A non-finite tempo
    /// keeps the previous block's value instead of inventing 120 BPM.
    pub(crate) fn stage(&self, frames: u32, bpm: f64, playing: bool) {
        let bpm = if bpm.is_finite() && bpm > 0.0 {
            bpm.clamp(1.0, 999.0)
        } else {
            f64::from_bits(self.transport_bpm.load(Ordering::Relaxed))
        };
        self.transport_bpm.store(bpm.to_bits(), Ordering::Relaxed);
        self.transport_playing.store(playing, Ordering::Relaxed);
        let (channels, channel_count, capacity) = channels_of(self.work);
        if frames == 0 || channel_count == 0 {
            return;
        }
        clear_planes(channels, channel_count, (frames as usize).min(capacity as usize));
    }

    /// Sums the live driver input into the track the monitor is routed to.
    pub(crate) fn add_input(&self, left: *const f32, right: *const f32, frames: u32) -> bool {
        let (channels, channel_count, capacity) = channels_of(self.work);
        if frames == 0
            || channels.is_null()
            || left.is_null()
            || right.is_null()
            || channel_count == 0
            || capacity < frames
        {
            return false;
        }
        unsafe {
            hirari_audio_buffer_storage_add_channels_from(self.work.cast_const(), left, right, frames)
        };
        true
    }

    /// Runs the insert chain and output stage over the block that upstream
    /// tracks already accumulated, without clearing it first.
    pub(crate) fn process_accumulated(&self, frames: u32, playhead: u64) -> bool {
        let (channels, channel_count, capacity) = channels_of(self.work);
        if channels.is_null() || frames == 0 || channel_count < 2 || capacity < frames {
            return false;
        }
        unsafe { self.process(self.work, frames, playhead, false) }
    }

    /// Retunes the track for a new device format and returns why a block growth
    /// could not happen; see [`hirari_track_runtime_prepare`].
    pub(crate) fn prepare(&self, region_key: *mut c_void, sample_rate: f64, block_frames: u32) -> u8 {
        if !sample_rate.is_finite() || sample_rate <= 0.0 || block_frames == 0 {
            return TrackPrepare::Ready.status();
        }
        unsafe {
            hirari_region_store_prepare(region_key, sample_rate, block_frames);
            hirari_channel_strip_set_sample_rate(self.channel_strip, sample_rate);
            hirari_track_holographic_panner_set_sample_rate(self.panner, sample_rate);
            hirari_channel_strip_reset(self.channel_strip);
        }
        // The order the track's own preparation used: the work block first, so a
        // declined growth still leaves the taps the callback addresses
        // consistent.
        for storage in [
            self.work,
            self.pre_fader,
            self.send_pre,
            self.send_post,
            self.pre_insert,
        ] {
            if storage.is_null() {
                continue;
            }
            let outcome = TrackPrepare::from_reserve(unsafe {
                hirari_audio_buffer_storage_reserve_buffer(storage, 2, block_frames)
            });
            if outcome == TrackPrepare::Ready {
                unsafe { hirari_audio_buffer_storage_resize(storage, 2, block_frames) };
                continue;
            }
            if outcome != TrackPrepare::Declined {
                return outcome.status();
            }
        }
        unsafe { hirari_effect_chain_set_sample_rate(self.chain, sample_rate, block_frames) };
        TrackPrepare::Ready.status()
    }

    /// Drops a freeze the new device rate cannot replay and retires its render.
    /// A 44.1 kHz snapshot must not survive a 48/96 kHz transition: it would
    /// silently change timing and keep a stale persisted render identity. The
    /// answer says whether the layout changed, which the host reports.
    pub(crate) fn invalidate_stale_freeze(&self, sample_rate: u32) -> bool {
        if !unsafe { hirari_track_frozen_store_is_frozen(self.frozen_store) }
            || unsafe { hirari_track_frozen_store_sample_rate(self.frozen_store) } == sample_rate
        {
            return false;
        }
        unsafe {
            hirari_track_frozen_store_clear(self.frozen_store);
            hirari_track_frozen_store_retire(self.frozen_store);
        }
        true
    }

    pub(crate) fn work_storage(&self) -> *mut c_void {
        self.work
    }

    pub(crate) fn window(&self, handle: *mut c_void, frames: u32) -> Option<Tap> {
        let (planes, count, capacity) = channels_of(handle);
        // A tap the finalise stage cannot address as stereo stays unpublished.
        if planes.is_null() || count < 2 || capacity < frames {
            return None;
        }
        let planes = unsafe { slice::from_raw_parts(planes, 2) };
        if planes[0].is_null() || planes[1].is_null() {
            return None;
        }
        Some(Tap {
            left: planes[0],
            right: planes[1],
        })
    }

    pub(crate) fn tap(&self, handle: *mut c_void, frames: u32) -> (*const *mut f32, u32) {
        let (planes, count, capacity) = channels_of(handle);
        // A tap the finalise stage cannot address as stereo stays unpublished.
        if count < 2 || capacity < frames {
            return (ptr::null(), 0);
        }
        (planes, count)
    }

    /// Runs one block: input gating, region rendering, MIDI, the insert chain
    /// and the output stage. A false return means the block was fully handled
    /// here (mute, freeze or an invalid block) and nothing downstream may run.
    pub(crate) unsafe fn process(&self, audio: *mut c_void, frames: u32, playhead: u64, clear: bool) -> bool {
        let (channels, channel_count, capacity) = channels_of(audio);
        if channels.is_null() || channel_count == 0 {
            return false;
        }
        if frames == 0 || frames > MAX_TRACK_BLOCK_FRAMES || frames > capacity {
            clear_planes(channels, channel_count, (frames as usize).min(capacity as usize));
            return false;
        }
        if self.muted.load(Ordering::Relaxed) {
            clear_planes(channels, channel_count, frames as usize);
            return false;
        }
        if unsafe { hirari_track_frozen_store_is_frozen(self.frozen_store) } {
            let view = unsafe { hirari_track_frozen_store_current(self.frozen_store) };
            let (sources, source_count, source_capacity) = channels_of(view.cast_mut());
            unsafe {
                hirari_audio_buffer_render_frozen(
                    channels,
                    channel_count,
                    capacity,
                    sources.cast(),
                    source_count,
                    source_capacity,
                    hirari_track_frozen_store_total_samples(self.frozen_store),
                    playhead,
                    frames,
                );
            }
            return false;
        }
        if clear {
            clear_planes(channels, channel_count, frames as usize);
        }
        let inputs = self
            .region_inputs
            .load(Ordering::Acquire)
            .cast::<crate::track_region_render::HirariTrackRegionRenderInput>();
        let region_count = self.region_count.load(Ordering::Acquire);
        if channel_count < 2
            || self.region_published.load(Ordering::Acquire) == 0
            || (region_count != 0 && inputs.is_null())
        {
            return false;
        }
        let planes = unsafe { slice::from_raw_parts(channels, channel_count as usize) };
        if planes[0].is_null() || planes[1].is_null() {
            return false;
        }
        unsafe {
            hirari_track_render_regions(
                inputs,
                region_count,
                playhead,
                frames,
                planes[0],
                planes[1],
                u8::from(self.phase_invert.load(Ordering::Relaxed)),
            );
        }

        let mut delay_count = 0usize;
        let delay_points = unsafe {
            hirari_track_automation_store_points(self.automation_store, 2, &mut delay_count)
        };
        let delay_hint = unsafe { hirari_track_automation_store_hint(self.automation_store, 2) };
        if !delay_points.is_null() && delay_count != 0 && !delay_hint.is_null() {
            let normalized = unsafe {
                hirari_realtime_automation_evaluate(delay_points.cast(), delay_count, playhead as f64, delay_hint)
            }
            .clamp(0.0, 1.0);
            let requested = (normalized * MAX_DELAY_SAMPLES as f32).round() as u32;
            self.track_delay_samples
                .store(requested.min(MAX_DELAY_SAMPLES), Ordering::Relaxed);
        }

        let mut event_count = 0usize;
        if self.transport_playing.load(Ordering::Relaxed) {
            let mut lane_count = 0usize;
            let lanes = unsafe {
                hirari_track_automation_store_plugin_lane_views(self.automation_store, &mut lane_count)
            };
            event_count = unsafe {
                hirari_plugin_automation_render_block(
                    lanes.cast(),
                    lane_count,
                    playhead,
                    frames,
                    self.plugin_events,
                    PLUGIN_EVENTS_PER_BLOCK,
                )
            };
        }

        self.prepare_midi_block(playhead, frames, planes[0], planes[1]);

        let (pre_insert, pre_insert_count, pre_insert_capacity) = channels_of(self.pre_insert);
        if pre_insert_count >= 2 && pre_insert_capacity >= frames && !pre_insert.is_null() {
            let destinations = unsafe { slice::from_raw_parts(pre_insert, pre_insert_count as usize) };
            if !destinations[0].is_null() && !destinations[1].is_null() {
                unsafe {
                    ptr::copy_nonoverlapping(planes[0], destinations[0], frames as usize);
                    ptr::copy_nonoverlapping(planes[1], destinations[1], frames as usize);
                }
            }
        }

        unsafe {
            hirari_effect_chain_parameter_automation(
                self.chain,
                self.plugin_events.cast::<TimedParameterEvent>().cast_const(),
                event_count,
            );
        }
        if channel_count >= 2 {
            unsafe {
                hirari_effect_chain_process(
                    self.chain,
                    audio,
                    self.midi,
                    f64::from_bits(self.transport_bpm.load(Ordering::Relaxed)),
                    self.sidechain_gate.load(Ordering::Acquire),
                    self.track_id,
                );
                // Processors share the mutable MIDI buffer for one block, so it
                // is dropped here rather than left for an alternate render path
                // to replay.
                hirari_midi_buffer_clear(self.midi);
            }
        }
        self.finalize(frames, playhead);
        true
    }

    /// Prepares one block's MIDI before the chain runs: expired expression
    /// note-offs are queued, the generated events are put in chronological
    /// order, and a track without an instrument gets its built-in voice. The
    /// scratch is bound at creation, so the audio thread never allocates.
    pub(crate) fn prepare_midi_block(&self, playhead: u64, frames: u32, left: *mut f32, right: *mut f32) {
        let capacity =
            unsafe { hirari_midi_buffer_remaining_capacity(self.midi) }.min(MIDI_EVENTS_PER_BLOCK);
        let count = unsafe {
            hirari_track_midi_flush_scheduled_note_offs(
                self.articulation,
                playhead,
                frames,
                if capacity == 0 {
                    ptr::null_mut()
                } else {
                    self.midi_events
                },
                capacity,
            )
        };
        for event in unsafe { slice::from_raw_parts(self.midi_events, count) } {
            unsafe {
                hirari_midi_buffer_add(
                    self.midi,
                    event.sample_offset,
                    event.data.as_ptr(),
                    u32::from(event.size),
                    event.articulation_id,
                );
            }
        }
        unsafe { hirari_midi_buffer_sort_owned(self.midi) };
        if !matches!(self.track_type, TRACK_TYPE_MIDI | TRACK_TYPE_INSTRUMENT)
            || unsafe { hirari_effect_chain_midi_instrument_count(self.chain) } != 0
            || frames == 0
            || left.is_null()
            || right.is_null()
        {
            return;
        }
        unsafe {
            hirari_builtin_midi_instrument_process(
                self.built_in_instrument,
                hirari_midi_buffer_event_data(self.midi),
                hirari_midi_buffer_event_count(self.midi),
                left,
                right,
                frames,
                hirari_channel_strip_sample_rate(self.channel_strip),
            )
        };
    }

    pub(crate) fn finalize(&self, frames: u32, playhead: u64) {
        let mut volume_count = 0usize;
        let mut pan_count = 0usize;
        let volume_points = unsafe {
            hirari_track_automation_store_points(self.automation_store, 0, &mut volume_count)
                .cast()
        };
        let pan_points = unsafe {
            hirari_track_automation_store_points(self.automation_store, 1, &mut pan_count).cast()
        };
        let (active, active_count) = self.tap(self.work, frames);
        if active.is_null() {
            return;
        }
        let (pre_insert, pre_insert_count) = self.tap(self.pre_insert, frames);
        let (send_pre, send_pre_count) = self.tap(self.send_pre, frames);
        let (send_post, send_post_count) = self.tap(self.send_post, frames);
        let (pre_fader, pre_fader_count) = self.tap(self.pre_fader, frames);
        let mut low_state_left = read(&self.low_state_left);
        let mut low_state_right = read(&self.low_state_right);
        unsafe {
            finalize_track_output(&TrackFinalizeInput {
                track_id: self.track_id,
                frames,
                start_time: playhead as f64,
                volume_points,
                volume_count,
                volume_hint: hirari_track_automation_store_hint(self.automation_store, 0),
                pan_points,
                pan_count,
                pan_hint: hirari_track_automation_store_hint(self.automation_store, 1),
                channel_strip: self.channel_strip,
                active_channels: active,
                active_channel_count: active_count,
                pre_insert_channels: pre_insert,
                pre_insert_channel_count: pre_insert_count,
                send_pre_channels: send_pre,
                send_pre_channel_count: send_pre_count,
                send_post_channels: send_post,
                send_post_channel_count: send_post_count,
                panner: self.panner,
                spatial_mode: 0,
                spatial_x: read(&self.spatial_x),
                spatial_y: read(&self.spatial_y),
                spatial_z: read(&self.spatial_z),
                low_gain: read(&self.low_gain),
                high_gain: read(&self.high_gain),
                low_state_l: &mut low_state_left,
                low_state_r: &mut low_state_right,
                pdc_delay: self.pdc_delay,
                requested_pdc: self.pdc_requested.load(Ordering::Acquire),
                manual_delay: self.track_delay_samples.load(Ordering::Acquire),
                pre_fader_channels: pre_fader,
                pre_fader_channel_count: pre_fader_count,
            });
        }
        self.low_state_left
            .store(low_state_left.to_bits(), Ordering::Relaxed);
        self.low_state_right
            .store(low_state_right.to_bits(), Ordering::Relaxed);
    }
}
