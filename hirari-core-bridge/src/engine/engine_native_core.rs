//! The unified engine type and its host ops entry points, split out of
//! engine_native.rs.
use super::*;

pub struct HirariUnifiedEngine {
    tracks_mutex: Box<Mutex<()>>,
    session: *mut c_void,
}

impl HirariUnifiedEngine {
    /// Builds the engine: one Rust graph lock, the session bag bound to this
    /// engine's address, and the sandbox recovery loop. The bag built and
    /// bound every store, including the block, before this returns; only the
    /// recovery loop needs the engine's own address to key by.
    pub(crate) fn new() -> Box<Self> {
        install_native_host_tables();
        let mut engine = Box::new(HirariUnifiedEngine {
            tracks_mutex: Box::default(),
            session: ptr::null_mut(),
        });
        // SAFETY: `engine`'s address is stable inside its box and the mutex
        // box outlives the session, so the bag can borrow both addresses.
        let session = {
            crate::engine_session::hirari_engine_session_create(
                &*engine as *const HirariUnifiedEngine as *mut c_void,
                &*engine.tracks_mutex as *const Mutex<()> as *mut c_void,
            )
        };
        engine.session = session;
        unsafe {
            crate::sandbox_recovery_runtime::hirari_sandbox_recovery_start(
                engine.host(),
                engine.handle(H_FORENSIC_JOURNAL),
            );
        }
        engine
    }

    pub(crate) fn handle(&self, kind: u32) -> *mut c_void {
        handle_of_host(self as *const Self as *const c_void, kind)
    }

    pub(crate) fn block(&self) -> *mut c_void {
        self.handle(H_ENGINE_BLOCK)
    }

    pub(crate) fn track_table(&self) -> *mut c_void {
        self.handle(H_TRACK_TABLE)
    }

    pub(crate) fn host(&self) -> *mut c_void {
        self as *const Self as *mut c_void
    }

    /// Locks the graph for one control-plane walk. The lock is not recursive,
    /// so a call that takes it itself — the graph resync — runs once this
    /// guard drops.
    pub(crate) fn lock_tracks(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.tracks_mutex.lock()
    }

    /// The track in the live graph, resolved under the graph lock.
    pub(crate) fn find_track_locked(&self, tid: u32) -> *mut c_void {
        unsafe { crate::engine_track_table::hirari_engine_track_find(self.track_table(), tid) }
    }

    /// The track in the published snapshot, read without the graph lock.
    pub(crate) fn find_published_track(&self, tid: u32) -> *mut c_void {
        let published = self.published_tracks();
        if published.track_map.is_null() || tid >= MAX_TRACK_MAP {
            return ptr::null_mut();
        }
        unsafe { *published.track_map.add(tid as usize) }
    }

    /// The lock-free window onto the published graph.
    pub fn published_tracks(&self) -> crate::engine_track_table::HirariTrackSnapshot {
        let mut published = crate::engine_track_table::HirariTrackSnapshot::default();
        unsafe {
            crate::engine_track_table::hirari_engine_track_snapshot(self.track_table(), &mut published)
        };
        published
    }

    /// A copy of the track that outlives the topology lock: an operation that
    /// can block on a worker or vendor UI code holds this and releases it when
    /// done.
    pub(crate) fn take_track_owner(&self, tid: u32) -> *mut c_void {
        let _lock = self.lock_tracks();
        unsafe { crate::engine_track_table::hirari_engine_track_retain(self.track_table(), tid) }
    }

    /// Republishes the track snapshot under the caller's graph lock.
    pub(crate) fn publish_track_snapshot_locked(&self, stamp_audio_generation: bool) {
        let block = self.block();
        unsafe {
            crate::engine_track_table::hirari_engine_track_publish(
                self.track_table(),
                crate::engine_block_host::hirari_engine_block_generation(block),
                stamp_audio_generation,
            );
        }
    }

    pub fn mark_project_layout_changed(&self) {
        let block = self.block();
        unsafe {
            crate::engine_track::hirari_layout_revision_mark(
                crate::engine_block_host::hirari_engine_block_layout_revision(block),
            );
        }
    }

    /// One ordered control walk, owned by Rust: the bus slots the graph
    /// occupies, the routing rebuild, the snapshot publication and the solved
    /// PDC offsets.
    pub fn sync_structural_changes(&self) {
        let _lock = self.lock_tracks();
        let block = self.block();
        unsafe {
            crate::engine_track_table::hirari_engine_sync_structure(
                self.track_table(),
                self.handle(H_BUS_SYSTEM),
                self.handle(H_ROUTING),
                self.handle(H_PDC),
                crate::engine_block_host::hirari_engine_block_generation(block),
            );
        }
        log_post(0, 0x1, "Structural Sync: Topology Rebuilt.");
    }

    /// Renders one bounded block into a wrapped buffer view.
    ///
    /// # Safety
    /// `output` must address `size` writable frames behind `offset` per
    /// channel, as produced by the audio-buffer wrap.
    pub(crate) unsafe fn process_block(&self, output: *mut c_void, offset: u32, size: u32) {
        let _denormal = DenormalGuard::enter();
        let block = self.block();
        let mut frames: u32 = 0;
        unsafe {
            if crate::engine_block_host::hirari_engine_block_begin(
                block, output, offset, size, &mut frames,
            ) == 0
            {
                return;
            }
            crate::engine_block_host::hirari_engine_block_route(block, output, offset, frames);
            crate::engine_block_host::hirari_engine_block_finish(block, output, offset, frames);
        }
    }

    /// Renders one device callback, slicing an oversized buffer into bounded
    /// engine blocks. The slices reuse the caller's memory: no allocation on
    /// the audio path.
    ///
    /// # Safety
    /// `buffers` must address `num_channels` valid plane pointers covering
    /// `num_samples` frames.
    pub(crate) unsafe fn process_block_direct(&self, buffers: *mut *mut f32, num_channels: u32, num_samples: u32) {
        let block = self.block();
        let input_blocks = self.handle(H_INPUT_BLOCKS);
        unsafe {
            crate::engine_block_host::hirari_engine_block_callback_enter(block);
        }
        struct CallbackGuard(*mut c_void);
        impl Drop for CallbackGuard {
            fn drop(&mut self) {
                unsafe {
                    crate::engine_block_host::hirari_engine_block_callback_leave(self.0);
                }
            }
        }
        let _guard = CallbackGuard(block);

        let valid_input = !buffers.is_null()
            && num_channels >= 2
            && num_channels <= 32
            && num_samples > 0
            && num_samples <= MAX_AUDIO_BLOCK_SIZE
            && !unsafe { *buffers.add(0) }.is_null()
            && !unsafe { *buffers.add(1) }.is_null();
        if !valid_input {
            // The caller does not provide the actual allocation length. When
            // the declared block size is invalid, writing even a bounded
            // prefix could overrun a shorter external buffer. Fail closed
            // without touching memory whose extent cannot be proven here.
            unsafe {
                crate::engine_input_block::hirari_input_blocks_end_talkback(input_blocks);
            }
            return;
        }
        for channel in 0..num_channels {
            if unsafe { *buffers.add(channel as usize) }.is_null() {
                return;
            }
        }
        if unsafe { crate::engine_block_host::hirari_engine_block_is_shutdown(block) } != 0 {
            unsafe {
                let silence = crate::audio_buffer_ops::hirari_audio_buffer_storage_create();
                let _ = crate::audio_buffer_ops::hirari_audio_buffer_storage_wrap_channels(
                    silence, buffers, num_channels, num_samples,
                );
                crate::audio_buffer_ops::hirari_audio_buffer_storage_clear(
                    silence, 0, num_samples,
                );
                crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(silence);
                crate::engine_input_block::hirari_input_blocks_end_talkback(input_blocks);
            }
            return;
        }
        // The track work buffers and the sandbox protocol are prepared for
        // bounded engine blocks. A host/device callback may legally be larger
        // than that bound, so feed the same external buffer in bounded slices
        // rather than silently skipping every track whose prepared buffer is
        // smaller.
        let wrap = { crate::audio_buffer_ops::hirari_audio_buffer_storage_create() };
        unsafe {
            let _ = crate::audio_buffer_ops::hirari_audio_buffer_storage_wrap_channels(
                wrap, buffers, num_channels, num_samples,
            );
        }
        let mut offset: u32 = 0;
        while offset < num_samples {
            // Rust owns the engine block cap, the transport and the
            // cycle-boundary split, including the rewind a playhead that
            // reached the loop end needs.
            let plan = unsafe {
                crate::engine_block_host::hirari_engine_block_chunk_plan(
                    block,
                    num_samples - offset,
                )
            };
            if (plan >> 32) != 0 {
                continue;
            }
            let chunk = plan as u32;
            if chunk == 0 {
                continue;
            }
            unsafe { self.process_block(wrap, offset, chunk) };
            offset += chunk;
        }
        unsafe {
            crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(wrap);
            crate::engine_input_block::hirari_input_blocks_end_monitor(input_blocks);
            crate::engine_input_block::hirari_input_blocks_end_talkback(input_blocks);
        }
    }

    /// The whole device format transaction: validity gate, quiesce, format
    /// swap, graph preparation and the structural rebuild.
    pub fn prepare_to_play(&self, sample_rate: f64, block_size: u32) {
        // A failed device negotiation must not advance the audio configuration
        // generation or partially replace the currently valid preparation
        // state. The rate/block bounds live in Rust (audio_config_rules.rs).
        if !crate::audio_config_rules::hirari_audio_format_valid(sample_rate, block_size) {
            return;
        }
        // Close callback admission before waiting for in-flight callbacks.
        let block = self.block();
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_config_transition(block, 1);
        }
        struct TransitionGuard(*mut c_void);
        impl Drop for TransitionGuard {
            fn drop(&mut self) {
                unsafe {
                    crate::engine_block_host::hirari_engine_block_set_config_transition(
                        self.0, 0,
                    );
                }
            }
        }
        let _guard = TransitionGuard(block);
        unsafe {
            crate::engine_block_host::hirari_engine_block_join_callbacks(block);
        }
        {
            let _lock = self.lock_tracks();
            // Deferred Track objects are intentionally retained until
            // shutdown. prepareToPlay may race with a device callback that
            // still has the previous published snapshot, so transport state is
            // not a reclamation proof.
            unsafe {
                crate::engine_block_host::hirari_engine_block_set_device_format(
                    block, sample_rate, block_size,
                );
                crate::engine_block_host::hirari_engine_block_bump_generation(block);
                crate::engine_block_host::hirari_engine_block_clear_midi_events(block);
                crate::master_limiter::hirari_master_limiter_prepare(
                    self.handle(H_MASTER_LIMITER),
                    sample_rate,
                );
                crate::master_meter::hirari_master_meter_prepare(
                    self.handle(H_METER),
                    sample_rate,
                );
            }
            // One Rust walk over the track table: a freeze the new rate cannot
            // replay is dropped and retired with the layout marked, the
            // track's blocks and DSP retune, and a work block that reached the
            // new length re-offers its taps to the sidechain graph at this
            // generation. Status 2 is an impossible size and 3 an exhausted
            // allocation; the C++ facade threw across the bridge, so a Rust
            // panic reports the failed preparation the same way.
            let status = unsafe {
                crate::engine_block_host::hirari_engine_block_prepare_tracks(block)
            };
            if status == 2 {
                panic!("prepareToPlay: track block size exceeds the allocation limit");
            }
            if status == 3 {
                panic!("prepareToPlay: track buffer allocation failed");
            }
        }
        // sync_structural_changes takes the graph lock itself. Never call it
        // while the preparation lock is held.
        self.sync_structural_changes();
    }

    /// Starts or stops the transport. Stopping also reclaims retired region
    /// generations once the callback is quiescent.
    pub fn set_playing(&self, playing: bool) {
        let block = self.block();
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_playing(
                block,
                u8::from(playing),
            );
        }
        if !playing {
            // The callback is quiescent once transport is stopped. Reclaim old
            // immutable region generations on the control thread only.
            unsafe {
                crate::engine_block_host::hirari_engine_block_join_callbacks(block);
            }
            let _lock = self.lock_tracks();
            let tracks = self.published_tracks_live_locked();
            for track in tracks {
                crate::region_store::hirari_region_store_trim_retired(track);
            }
        }
    }

    /// The live graph in layout order, valid while the caller's lock is held.
    pub(crate) fn published_tracks_live_locked(&self) -> Vec<*mut c_void> {
        let table = self.track_table();
        unsafe {
            let count = crate::engine_track_table::hirari_engine_track_count(table);
            let pointers = crate::engine_track_table::hirari_engine_track_ptrs(table);
            if pointers.is_null() {
                return Vec::new();
            }
            std::slice::from_raw_parts(pointers, count).to_vec()
        }
    }

    /// Stops the graph and tears down the live session. Idempotent.
    pub fn shutdown(&self) {
        let block = self.block();
        let was_shutdown = unsafe {
            crate::engine_block_host::hirari_engine_block_take_shutdown(block) != 0
        };
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_playing(block, 0);
            crate::engine_block_host::hirari_engine_block_join_callbacks(block);
            // Shutdown must cancel offline work before joining it. Joining a
            // render that is still publishing blocks can otherwise keep app
            // teardown alive indefinitely.
            crate::bounce_driver::hirari_bounce_join(self.handle(H_BOUNCE_SESSION));
        }
        crate::sandbox_recovery_runtime::hirari_sandbox_recovery_stop();
        if was_shutdown {
            return;
        }
        let _lock = self.lock_tracks();
        // Retiring the table releases every track handle, live and deferred,
        // so a callback that has already been joined cannot be holding a
        // dangling track pointer.
        crate::engine_session::hirari_engine_session_close_graph(self.session);
        self.mark_project_layout_changed();
    }

    /// Starts a fresh project: resets every project-scoped store and
    /// bootstraps one audio track so the canvas is immediately usable.
    pub fn new_project(&self) {
        self.set_playing(false);
        let block = self.block();
        unsafe {
            crate::engine_block_host::hirari_engine_block_join_callbacks(block);
            // Input monitoring is a live device assignment, not project data.
            // Clear it before reusing any track IDs in the new project.
            crate::engine_input_block::hirari_input_blocks_reset(self.handle(H_INPUT_BLOCKS));
            crate::engine_rings::hirari_engine_command_ring_clear(
                self.handle(H_ENGINE_RINGS).cast(),
            );
            crate::region_jobs::hirari_region_jobs_clear(self.host());
            crate::engine_block_host::hirari_engine_block_set_test_tone(block, 0);
            crate::engine_block_host::hirari_engine_block_set_playhead(block, 0);
            crate::midi_orchestration::hirari_mpe_state_reset(self.handle(H_MPE_STATE).cast());
            crate::mpe_manager::hirari_mpe_manager_reset(self.handle(H_MPE_MANAGER));
            crate::undo_transaction::hirari_undo_manager_clear(self.handle(H_UNDO));
            crate::native_pdc::hirari_native_pdc_reset_for_project(self.handle(H_PDC));
            crate::sidechain_manager::hirari_sidechain_engine_reset(
                self.handle(H_SIDECHAIN_ENGINE),
            );
            crate::routing_engine_core::hirari_routing_engine_reset(self.handle(H_ROUTING));
            crate::bus_audio::hirari_bus_system_reset(self.handle(H_BUS_SYSTEM));
            crate::control_room_runtime::hirari_control_room_reset(self.handle(H_CONTROL_ROOM));
            crate::tempo::hirari_tempo_map_set_current_bpm(self.handle(H_TEMPO), 120.0);
            crate::engine_block_host::hirari_engine_block_set_master_gain(block, 1.0);
            // Region ids restart with the project, so cached peaks must not
            // survive.
            crate::region_audio::hirari_region_waveform_clear(self.host());
        }
        let _lock = self.lock_tracks();
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_clear(
                self.handle(H_SIDECHAIN_ROUTES),
            );
            // Keep old Track objects alive while a device callback may still
            // hold the previous published pointers; the table destroys them at
            // shutdown.
            crate::engine_track_table::hirari_engine_track_defer_all(self.track_table());
        }
        self.mark_project_layout_changed();
        unsafe { crate::id_host::hirari_ids_reset_project(self.host()) };
        // Start with one audio track so a new project is immediately usable
        // for importing or recording, instead of presenting a dead empty
        // canvas.
        unsafe {
            let mut bootstrap: *mut c_void = ptr::null_mut();
            let bootstrap_owner = crate::engine_track::hirari_track_new(
                1,
                b"Audio 1".as_ptr(),
                7,
                0, // Track kind audio.
                crate::engine_block_host::hirari_engine_block_layout_revision(block),
                &mut bootstrap,
            );
            if !bootstrap_owner.is_null() {
                let _ = crate::engine_block_host::hirari_engine_block_prepare_track(
                    block,
                    bootstrap,
                );
                let _ = crate::engine_track_table::hirari_engine_track_insert(
                    self.track_table(),
                    0,
                    bootstrap_owner,
                );
                self.mark_project_layout_changed();
            }
        }
        self.publish_track_snapshot_locked(true);
    }

    /// Imports one region under the given track; the import policy and the
    /// worker live in Rust (`region_jobs.rs`) and this only translates the
    /// result codes into the UI diagnostics.
    pub fn add_region(&self, tid: u32, path: &str, start_sample: f64) -> bool {
        let result = unsafe {
            crate::region_jobs::hirari_region_import_add(
                self.host(),
                tid,
                path.as_ptr(),
                path.len(),
                start_sample,
            )
        };
        if result == 1 {
            log_post(1, 0x2102, "REGION_TRACK_NOT_FOUND");
        } else if result == 2 {
            log_post(1, 0x2101, "REGION_IMPORT_FAILED");
        }
        result == 0
    }

    pub fn set_plugin_bypass(&self, tid: u32, plugin_index: u32, bypassed: bool) -> bool {
        if !unsafe {
            crate::plugin_edit_host::hirari_plugin_edit_set_bypass(
                self.host(),
                tid,
                plugin_index,
                bypassed,
            )
        } {
            return false;
        }
        // Bypass changes alter the active latency path. Rebuild the published
        // routing/PDC state now that the edit host has released the graph
        // lock.
        self.sync_structural_changes();
        true
    }

    pub fn has_sidechain_link(&self, source_id: u32, dest_id: u32, plugin_index: u32) -> bool {
        unsafe {
            crate::sidechain_manager::hirari_sidechain_engine_has_link(
                self.handle(H_SIDECHAIN_ENGINE),
                dest_id,
                plugin_index,
                source_id,
            )
        }
    }

    /// Appends one processing step to a region's history.
    pub fn append_region_processing(
        &self,
        tid: u32,
        rid: u32,
        step_id: u32,
        operation: &str,
        parameter: f32,
        enabled: bool,
    ) -> bool {
        let track = self.find_published_track(tid);
        if track.is_null() {
            return false;
        }
        let step = crate::region_processing_history::HirariEventStep {
            id: step_id,
            operation: operation.as_ptr(),
            operation_len: operation.len(),
            parameter,
            enabled,
        };
        unsafe {
            crate::region_store::hirari_region_store_append_processing(track, rid, step)
        }
    }

    /// Clears a region's processing history.
    pub fn clear_region_processing(&self, tid: u32, rid: u32) -> bool {
        let track = self.find_published_track(tid);
        !track.is_null()
            && crate::region_store::hirari_region_store_clear_processing(track, rid)
    }

    /// The project's rendered waveform peaks for one region.
    pub fn get_region_waveform(&self, tid: u32, rid: u32, num_peaks: u32) -> Vec<f32> {
        let buffer =
            unsafe { crate::region_audio::hirari_region_waveform(self.host(), tid, rid, num_peaks) };
        let mut samples = Vec::new();
        unsafe {
            if !buffer.is_null() {
                let data = crate::region_audio::hirari_float_buffer_data(buffer);
                let count = crate::region_audio::hirari_float_buffer_len(buffer);
                if !data.is_null() && count != 0 {
                    samples.reserve(count);
                    samples.extend_from_slice(std::slice::from_raw_parts(data, count));
                }
                crate::region_audio::hirari_float_buffer_destroy(buffer);
            }
        }
        samples
    }

    /// Runs one synchronous bounce after rebuilding the published snapshot, so
    /// a bounce can never observe an empty or stale track graph.
    pub fn bounce_project(&self, path: &str, format: u32) -> bool {
        self.sync_structural_changes();
        unsafe {
            crate::bounce_driver::hirari_bounce_render_driver(
                self.host(),
                self.handle(H_BOUNCE_SESSION),
                bounce_ops(),
                path.as_ptr(),
                path.len(),
                format,
            )
        }
    }

    /// Starts one bounce on the offline worker.
    pub fn bounce_project_async(&self, path: &str, format: u32) -> bool {
        unsafe {
            crate::bounce_driver::hirari_bounce_start_async(
                self.handle(H_BOUNCE_SESSION),
                self.host(),
                bounce_ops(),
                path.as_ptr(),
                path.len(),
                format,
            )
        }
    }

    pub fn get_bounce_progress(&self) -> f32 {
        crate::bounce_driver::hirari_bounce_progress(self.handle(H_BOUNCE_SESSION))
    }

    pub fn get_bounce_state(&self) -> u32 {
        crate::bounce_driver::hirari_bounce_state(self.handle(H_BOUNCE_SESSION))
    }

    pub fn cancel_bounce(&self) -> bool {
        crate::bounce_driver::hirari_bounce_cancel(self.handle(H_BOUNCE_SESSION))
    }

    pub fn pause_bounce(&self) -> bool {
        crate::bounce_driver::hirari_bounce_pause(self.handle(H_BOUNCE_SESSION))
    }

    pub fn resume_bounce(&self) -> bool {
        crate::bounce_driver::hirari_bounce_resume(self.handle(H_BOUNCE_SESSION))
    }

    /// Sets the project tonality. The bridge validates the ranges.
    pub fn set_project_scale(&self, root: i32, scale_type: i32) -> bool {
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_project_scale(
                self.block(),
                root,
                scale_type,
            );
        }
        true
    }

    pub fn set_articulation_map(&self, track_id: u32, _map_hash: u32) -> bool {
        !self.find_published_track(track_id).is_null()
    }

    /// The AI arrangement pass: project extent and section analysis are
    /// Rust-owned; the engine keeps the event queue behind the marker sink.
    pub fn execute_auto_arrangement(&self) -> bool {
        let mut harmony_tension = 0.5f32;
        let mut harmony_valence = 0.5f32;
        unsafe {
            crate::harmonic_context::hirari_harmonic_context_get(
                &mut harmony_tension,
                &mut harmony_valence,
                ptr::null_mut(),
            );
            crate::auto_arrangement::hirari_engine_auto_arrange(
                self.host(),
                crate::tempo::hirari_tempo_map_get_current_bpm(self.handle(H_TEMPO)),
                crate::engine_block_host::hirari_engine_block_device_sample_rate(self.block()),
                harmony_tension,
                harmony_valence,
                Some(emit_structure_marker),
            )
        }
    }

    /// Tempo edit entry point kept for the native contract tests that drive
    /// the engine directly; every other tempo edit reaches Rust without
    /// crossing this facade.
    pub fn set_tempo(&self, bpm: f32) -> bool {
        let block = self.block();
        unsafe {
            crate::hirari_tempo_edit_set_tempo(
                self.handle(H_TEMPO),
                self.handle(H_UNDO),
                crate::log_buffer::hirari_log_buffer_default_state(),
                crate::engine_block_host::hirari_engine_block_playhead(block),
                crate::engine_block_host::hirari_engine_block_device_sample_rate(block),
                bpm,
            )
        }
    }

    /// Storage only: the bridge owns the range check and the undo entry.
    pub fn set_master_gain(&self, value: f32) -> bool {
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_master_gain(self.block(), value);
        }
        true
    }

    pub fn get_master_gain(&self) -> f32 {
        unsafe { crate::engine_block_host::hirari_engine_block_master_gain(self.block()) }
    }

    pub fn get_sample_rate(&self) -> f64 {
        unsafe {
            crate::engine_block_host::hirari_engine_block_device_sample_rate(self.block())
        }
    }

    pub fn bus_track_exists(&self, bus_track_id: u32) -> bool {
        if bus_track_id == 0 {
            return false;
        }
        let _lock = self.lock_tracks();
        let track = self.find_track_locked(bus_track_id);
        !track.is_null()
            && unsafe { crate::engine_track::hirari_track_type(track) } == TRACK_TYPE_BUS
    }

    pub fn mark_project_state_changed(&self) {
        unsafe {
            crate::engine_block_host::hirari_engine_block_mark_control(self.block());
        }
    }

    pub fn midi_clock_tick(&self, timestamp: u64) {
        unsafe {
            crate::external_sync_runtime::hirari_external_sync_tick(
                self.handle(H_EXTERNAL_SYNC),
                timestamp,
            );
        }
    }

    pub fn midi_clock_ticks(&self) -> u64 {
        unsafe {
            crate::external_sync_runtime::hirari_external_sync_ticks(self.handle(H_EXTERNAL_SYNC))
        }
    }

    pub fn midi_clock_last_tick(&self) -> u64 {
        unsafe {
            crate::external_sync_runtime::hirari_external_sync_last_tick(
                self.handle(H_EXTERNAL_SYNC),
            )
        }
    }

    pub fn midi_clock_rate(&self) -> f64 {
        unsafe {
            crate::external_sync_runtime::hirari_external_sync_rate(self.handle(H_EXTERNAL_SYNC))
        }
    }

    pub fn set_midi_clock_rate(&self, bpm: f64) {
        unsafe {
            crate::external_sync_runtime::hirari_external_sync_set_rate(
                self.handle(H_EXTERNAL_SYNC),
                bpm,
            );
        }
    }

    pub fn midi_snapshot_store(&self) -> usize {
        self.handle(H_MIDI_SNAPSHOT_STORE) as usize
    }

    pub fn midi_learn_state(&self) -> usize {
        self.handle(H_MIDI_LEARN) as usize
    }

    pub fn preview_synth_state(&self) -> usize {
        self.handle(H_PREVIEW_SYNTH) as usize
    }

    pub fn pdc_state(&self) -> usize {
        self.handle(H_PDC) as usize
    }

    pub fn preview_sampler(&self) -> usize {
        self.handle(H_PREVIEW_SAMPLER) as usize
    }

    pub fn metronome_state(&self) -> usize {
        self.handle(H_METRONOME) as usize
    }

    pub fn routing_engine_state(&self) -> usize {
        self.handle(H_ROUTING) as usize
    }

    pub fn engine_block_state(&self) -> usize {
        self.block() as usize
    }

    pub fn meter_state(&self) -> usize {
        self.handle(H_METER) as usize
    }

    pub fn bounce_session_state(&self) -> usize {
        self.handle(H_BOUNCE_SESSION) as usize
    }

    pub fn join_audio_callbacks(&self) {
        unsafe {
            crate::engine_block_host::hirari_engine_block_join_callbacks(self.block());
        }
    }
}

impl Drop for HirariUnifiedEngine {
    fn drop(&mut self) {
        // Async region jobs hold the engine host pointer; join them before any
        // store they may still reach is destroyed.
        crate::region_jobs::hirari_region_jobs_shutdown();
        // Rust keys cached peaks by engine address; a later engine may reuse it.
        unsafe { crate::region_audio::hirari_region_waveform_clear(self.host()) };
        self.shutdown();
        // Sandbox observed-health state is keyed by engine address for the
        // same reason. Drop it after shutdown() has joined the recovery thread.
        unsafe { crate::plugin_sandbox_maintenance::hirari_plugin_sandbox_maintenance_forget(self.host()) };
        // The bag owns the teardown order: the block goes before the stores it
        // borrows and the journal stops before the state its writer reads.
        crate::engine_session::hirari_engine_session_destroy(self.session);
        self.session = ptr::null_mut();
    }
}

/// The graph resync the undo replay and plugin edit host call by address.
///
/// # Safety
/// `host` must be a live engine host.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_sync_structural_changes(host: *mut c_void) {
    if !host.is_null() {
        unsafe { (*(host as *const HirariUnifiedEngine)).sync_structural_changes() };
    }
}

/// The FPU denormal guard. The enter/leave implementation is Rust
/// (`realtime_thread.rs`) and is a no-op where the FPU needs no help.
struct DenormalGuard(u64);

impl DenormalGuard {
    pub(crate) fn enter() -> Self {
        Self(crate::realtime_thread::hirari_denormal_control_enter())
    }
}

impl Drop for DenormalGuard {
    fn drop(&mut self) {
        crate::realtime_thread::hirari_denormal_control_leave(self.0);
    }
}

/// The engine's graph lock, driven by its address through the same ABI the
/// Rust edit-host sessions already use (`track_session.rs`). The mutex is the
/// engine's own, so the graph is guarded against every control-plane edit.
///
/// # Safety
/// `tracks_mutex` must be the address of the engine's `Mutex<()>`.
#[no_mangle]
pub unsafe extern "C" fn hirari_tracks_mutex_lock(tracks_mutex: *mut c_void) {
    if !tracks_mutex.is_null() {
        let mutex = unsafe { &*(tracks_mutex as *const Mutex<()>) };
        // The guard is intentionally leaked: the lock is released by the
        // matching `hirari_tracks_mutex_unlock` on the same thread, exactly as
        // the C++ `std::mutex::lock`/`unlock` pair behaved.
        std::mem::forget(mutex.lock());
    }
}

/// # Safety
/// `tracks_mutex` must be the address of the engine's `Mutex<()>` currently
/// locked by this thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_tracks_mutex_unlock(tracks_mutex: *mut c_void) {
    if !tracks_mutex.is_null() {
        let mutex = unsafe { &*(tracks_mutex as *const Mutex<()>) };
        unsafe { mutex.force_unlock() };
    }
}

/// One section marker per detected arrangement section; the section length
/// travels as the event value.
unsafe extern "C" fn emit_structure_marker(context: *mut c_void, start: u64, length: u64) {
    let engine = unsafe { &*(context as *const HirariUnifiedEngine) };
    unsafe {
        crate::engine_rings::hirari_engine_event_ring_push(
            engine.handle(H_ENGINE_RINGS).cast(),
            start,
            0x50, // EVENT_TYPE_STRUCTURE_MARKER
            0,
            length as f32,
            ptr::null(),
            0,
        );
    }
    log_post(0, 0, "AI | ARRANGEMENT | SECTION_DETECTED");
}
