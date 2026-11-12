//! MIDI staging + block state edits, split out of engine_block_host.rs.
use super::*;

impl EngineBlockState {
    /// Feeds each track's post-graph block into its one-block delayed routing
    /// edges, in published graph order.
    pub(in crate::engine_block_host) unsafe fn capture_feedback(&self, frames: u32, view: View) {        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            unsafe {
                hirari_routing_engine_capture_feedback(
                    self.routing,
                    runtime.track_id(),
                    tap.left,
                    tap.right,
                    frames,
                )
            };
        }
    }

    /// Publishes per-track linear peaks through the double buffer. The payload
    /// stays strictly aligned with the active track order, so a track that
    /// cannot hold the block keeps its slot at silence.
    pub(in crate::engine_block_host) unsafe fn publish_peaks(&self, frames: u32, view: View) {
        let flag = self.telemetry_lock.swap(true, Ordering::Acquire);
        debug_assert!(!flag, "the metering handoff must not re-enter the callback");
        let next = ((self.telemetry_active.load(Ordering::Relaxed) as usize) ^ 1) as isize;
        let telemetry = unsafe { &mut *self.telemetry.offset(next) };
        telemetry.count = view.count.min(MAX_TRACKS as u32);
        let slots = telemetry.count as usize;
        let ids = unsafe { slice::from_raw_parts_mut(self.telemetry_ids, slots) };
        let lefts = unsafe { slice::from_raw_parts_mut(self.telemetry_left, slots) };
        let rights = unsafe { slice::from_raw_parts_mut(self.telemetry_right, slots) };
        ids.fill(0);
        lefts.fill(std::ptr::null());
        rights.fill(std::ptr::null());
        for slot in 0..slots {
            let Some(&track) = view.active().get(slot) else {
                continue;
            };
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            ids[slot] = runtime.track_id();
            lefts[slot] = tap.left;
            rights[slot] = tap.right;
        }
        unsafe {
            hirari_mixer_track_peaks(
                self.mixer_telemetry,
                ids.as_ptr(),
                lefts.as_ptr(),
                rights.as_ptr(),
                telemetry.count,
                frames,
                telemetry.peaks_left.as_mut_ptr(),
                telemetry.peaks_right.as_mut_ptr(),
            )
        };
        telemetry.version += 1;
        self.telemetry_active.store(next as u32, Ordering::Release);
        self.telemetry_lock.store(false, Ordering::Release);
    }

    /// The metronome belongs to the rendered monitor signal, not to a track
    /// node, so it is injected here and left for the master limiter to trim.
    /// Returns whether the click is part of this block.
    pub(in crate::engine_block_host) unsafe fn render_metronome(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
        playing: bool,
    ) -> bool {
        let block = frames as usize;
        let scratch = unsafe { &mut *self.click };
        scratch[0][..block].fill(0.0);
        scratch[1][..block].fill(0.0);
        if !playing || !unsafe { hirari_metronome_is_enabled(self.metronome) } {
            return false;
        }
        unsafe {
            hirari_metronome_process(
                self.metronome,
                self.click(0),
                self.click(1),
                frames,
                playhead,
                sample_rate,
                hirari_tempo_map_get_current_bpm(self.tempo) as f64,
            )
        };
        true
    }

    pub(in crate::engine_block_host) fn find_track(&self, track_id: u32, view: View) -> Option<*mut c_void> {
        view.active()
            .iter()
            .copied()
            .filter(|&track| !track.is_null())
            .find(|&track| {
                self.runtime(track)
                    .is_some_and(|runtime| runtime.track_id() == track_id)
            })
    }

    /// Control Room is strictly device monitoring: speaker trim and DIM are
    /// never baked into an offline master or stem render.
    pub(in crate::engine_block_host) unsafe fn render_control_room(
        &self,
        planes: *const *mut f32,
        channel_count: u32,
        off: u32,
        frames: u32,
        talkback_offset: u32,
        view: View,
    ) {
        if hirari_bounce_offline_active(self.bounce) != 0 {
            return;
        }
        let planes = unsafe { slice::from_raw_parts(planes, channel_count as usize) };
        let offset = off as usize;
        let active_cue_bus = unsafe { hirari_control_room_active_cue_bus(self.control_room) };
        let mut cue = unsafe { hirari_control_room_active_cue_output(self.control_room) };
        let dedicated_pair = cue >= 2;
        let cue_available = cue + 1 < channel_count;
        if !cue_available {
            cue = 0;
        }
        let index = cue as usize;
        if active_cue_bus != 0 && (!dedicated_pair || cue_available) {
            if let Some(bus) = self.find_track(active_cue_bus, view) {
                let bus_state = self.bus_state(bus);
                if let Some(runtime) = self.runtime(bus).filter(|_| !bus_state.is_null()) {
                    if let Some(tap) = runtime.work(frames) {
                        unsafe {
                            hirari_control_room_mix_cue(
                                tap.left,
                                tap.right,
                                self.click(0) as *const f32,
                                self.click(1) as *const f32,
                                planes[index].add(offset),
                                planes[index + 1].add(offset),
                                frames,
                                hirari_control_room_active_cue_gain(self.control_room),
                                hirari_control_room_active_cue_click(self.control_room),
                            )
                        };
                    }
                }
            }
        }
        let talkback =
            unsafe { hirari_input_blocks_take_talkback(self.input_blocks, talkback_offset, frames) };
        if dedicated_pair {
            // Keep the Main control-room monitor live while a selected Cue feeds
            // its own output pair; talkback stays on that pair so it cannot leak
            // into the control-room mix.
            unsafe {
                hirari_control_room_process_monitor_state(
                    self.control_room,
                    planes[0].add(offset),
                    planes[1].add(offset),
                    std::ptr::null(),
                    frames,
                );
            }
        }
        if !dedicated_pair || cue_available {
            unsafe {
                hirari_control_room_process_monitor_state(
                    self.control_room,
                    planes[index].add(offset),
                    planes[index + 1].add(offset),
                    talkback,
                    frames,
                )
            };
        }
    }

    /// Graph eligibility for one track, read through the routing kernel the
    /// engine's solo and offline-target state feeds.
    pub(in crate::engine_block_host) fn should_process(&self, track: *mut c_void, gating: &Gating) -> bool {
        let Some(runtime) = self.runtime(track) else {
            return false;
        };
        unsafe {
            hirari_routing_engine_should_process(
                self.routing,
                runtime.track_id(),
                gating.offline_target_active,
                gating.offline_target_id,
                gating.any_solo,
                self.solo(track),
            )
        }
    }

    /// Publishes one block's tempo and playback state, then empties every work
    /// buffer so upstream tracks accumulate into a known-zero tap.
    pub(in crate::engine_block_host) unsafe fn stage_tracks(&self, frames: u32, view: View, playing: bool) {
        let bpm = unsafe { hirari_tempo_map_get_current_bpm(self.tempo) } as f64;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            if let Some(runtime) = self
                .runtime(track)
                .filter(|runtime| runtime.work(frames).is_some())
            {
                runtime.stage(frames, bpm, playing);
            }
        }
    }

    /// Feedback edges are explicitly delayed by one engine block. They are
    /// injected before any graph node runs, so a cycle cannot recurse on the
    /// current block or allocate on the audio thread.
    pub(in crate::engine_block_host) unsafe fn inject_feedback(&self, frames: u32, view: View) {
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(tap) = runtime.work(frames) else {
                continue;
            };
            unsafe {
                hirari_routing_engine_inject_feedback(
                    self.routing,
                    runtime.track_id(),
                    tap.left,
                    tap.right,
                    frames,
                )
            };
        }
    }

    /// Feeds the live driver input into the selected monitor track before graph
    /// processing. The staged planes were copied by the driver callback, so this
    /// path performs no allocation or locking.
    pub(in crate::engine_block_host) unsafe fn feed_monitor_input(&self, frames: u32, monitor_offset: u32, view: View) {
        let monitor =
            unsafe { hirari_input_blocks_take_monitor(self.input_blocks, monitor_offset, frames) };
        if monitor.left.is_null() {
            return;
        }
        let Some(track) = self.find_track(monitor.track_id, view) else {
            return;
        };
        if let Some(runtime) = self.runtime(track) {
            runtime.add_input(monitor.left.cast_const(), monitor.right.cast_const(), frames);
        }
    }

    /// Publishes the destination table one fanout call reads, in graph order.
    /// The scratch it lives in is bound once, so no block allocates.
    pub(in crate::engine_block_host) unsafe fn publish_destinations(&self, frames: u32, view: View, gating: &Gating) -> usize {
        let table = unsafe { &mut *self.destinations };
        let mut count = 0usize;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let (channels, channel_count) = runtime.work_channels(frames);
            if channels.is_null() {
                continue;
            }
            let bus_state = self.bus_state(track);
            table[count] = RoutingTrackDestination {
                id: runtime.track_id(),
                can_process: u8::from(self.should_process(track, gating)),
                is_bus: u8::from(!bus_state.is_null()),
                channel_count,
                work_channels: channels,
                bus_context: bus_state,
            };
            count += 1;
        }
        count
    }

    /// Applies one processed track's normal routes and parallel bus sends.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::engine_block_host) unsafe fn route_to_next_stages(
        &self,
        runtime: &TrackRuntime,
        gating: &Gating,
        master_left: *mut f32,
        master_right: *mut f32,
        destinations: &[RoutingTrackDestination],
        frames: u32,
    ) {
        // Cue buses are isolated monitor sources. Their Send inputs and insert
        // chain run normally, but their signal is selected only at the Control
        // Room boundary, never summed into the Main mix.
        if !gating.offline_target_active
            && unsafe { hirari_control_room_is_cue_bus(self.control_room, runtime.track_id()) }
        {
            return;
        }
        if gating.offline_target_active
            && gating.offline_target_id != 0
            && runtime.track_id() == gating.offline_target_id
        {
            let tap = if hirari_bounce_pre_fader(self.bounce) != 0 {
                runtime.pre_fader(frames)
            } else if hirari_bounce_include_inserts(self.bounce) == 0 {
                runtime.pre_insert(frames)
            } else {
                runtime.work(frames)
            };
            if let Some(tap) = tap {
                unsafe {
                    hirari_audio_add_sanitized_stereo(
                        master_left,
                        master_right,
                        tap.left.cast_const(),
                        tap.right.cast_const(),
                        frames,
                    )
                };
            }
            return;
        }
        let send_pre = runtime.send_pre(frames);
        let send_post = runtime.send_post(frames);
        let (send_pre_left, send_pre_right) = send_pre.map_or((ptr::null(), ptr::null()), |tap| {
            (tap.left.cast_const(), tap.right.cast_const())
        });
        let (send_post_left, send_post_right) =
            send_post.map_or((ptr::null(), ptr::null()), |tap| {
                (tap.left.cast_const(), tap.right.cast_const())
            });
        let (source_channels, source_channel_count) = runtime.work_channels(frames);
        // One state-owned scratch pair is reused for each sequential Send edge;
        // delay histories live in the routing kernel and nothing here allocates.
        let scratch = unsafe { &mut *self.send_pdc };
        unsafe {
            hirari_routing_engine_process_track_fanout(
                self.routing,
                runtime.track_id(),
                source_channels.cast(),
                source_channel_count,
                send_pre_left,
                send_pre_right,
                send_post_left,
                send_post_right,
                runtime.delay_samples(),
                destinations.as_ptr(),
                destinations.len(),
                master_left,
                master_right,
                scratch[0].as_mut_ptr(),
                scratch[1].as_mut_ptr(),
                frames,
                MAX_BUS_FRAMES as u32,
                Some(add_bus_pre_fx as BusPreFxAdd),
            );
        }
    }

    /// Walks the graph in resolved process order: upstream tracks have already
    /// accumulated into this node's work buffer, so the node renders what it was
    /// given, publishes itself as a sidechain source and fans out.
    pub(in crate::engine_block_host) unsafe fn walk_graph(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: u32,
        generation: u64,
        view: View,
        gating: &Gating,
        master_left: *mut f32,
        master_right: *mut f32,
        destinations: &[RoutingTrackDestination],
    ) {
        let ids = unsafe { &mut *self.graph_ids };
        let mut id_count = 0usize;
        for &track in view.active() {
            if track.is_null() {
                continue;
            }
            if let Some(runtime) = self.runtime(track) {
                ids[id_count] = runtime.track_id();
                id_count += 1;
            }
        }
        let order = unsafe { &mut *self.process_order };
        let mut order_count = 0u32;
        if !unsafe {
            hirari_routing_engine_resolve_process_order(
                self.routing,
                ids.as_ptr(),
                id_count,
                order.as_mut_ptr(),
                order.len(),
                &mut order_count,
            )
        } {
            // A graph the topology cannot order still renders in published
            // order rather than dropping every track's block.
            order_count = id_count as u32;
            order[..id_count].copy_from_slice(&ids[..id_count]);
        }
        for index in 0..order_count as usize {
            let Some(track) = self.find_track(order[index], view) else {
                continue;
            };
            if !self.should_process(track, gating) {
                continue;
            }
            let Some(runtime) = self.runtime(track) else {
                continue;
            };
            let Some(work) = runtime.work(frames) else {
                continue;
            };
            let bus_state = self.bus_state(track);
            if !bus_state.is_null() && frames != 0 && frames <= MAX_BUS_FRAMES as u32 {
                unsafe { hirari_bus_audio_read(bus_state, work.left, work.right, frames, false) };
            }
            // Refresh the sidechain gate handle so sandbox workers fetch
            // this block's published sources during the track pass.
            runtime.set_sidechain_gate(self.sidechain);
            // The graph publishes the node it just rendered whether or not the
            // chain ran: a muted or frozen track still owns its bus stage and
            // still feeds downstream sidechain and Send edges.
            let _ = runtime.process_accumulated(frames, playhead);
            if !bus_state.is_null() && frames != 0 && frames <= MAX_BUS_FRAMES as u32 {
                unsafe {
                    hirari_bus_audio_replace_post(
                        bus_state,
                        work.left.cast_const(),
                        work.right.cast_const(),
                        frames,
                    )
                };
            }
            let (pre_left, pre_right) = runtime
                .pre_insert(frames)
                .map_or((ptr::null(), ptr::null()), |tap| {
                    (tap.left.cast_const(), tap.right.cast_const())
                });
            let (post_left, post_right) = runtime
                .pre_fader(frames)
                .map_or((ptr::null(), ptr::null()), |tap| {
                    (tap.left.cast_const(), tap.right.cast_const())
                });
            unsafe {
                hirari_sidechain_engine_publish_source(
                    self.sidechain,
                    runtime.track_id(),
                    pre_left,
                    pre_right,
                    post_left,
                    post_right,
                    work.left.cast_const(),
                    work.right.cast_const(),
                    frames,
                    sample_rate,
                    generation,
                );
            }
            self.route_to_next_stages(
                runtime,
                gating,
                master_left,
                master_right,
                destinations,
                frames,
            );
        }
    }
}
