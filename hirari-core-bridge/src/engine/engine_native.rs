//! The native engine facade, now Rust.
//!
//! `HirariUnifiedEngine` and `AudioEngine` used to be C++ classes behind the
//! cxx bridge. Every store they touched had already moved to Rust
//! (`engine_session.rs` and friends), so the classes were a facade: a session
//! bag handle, one graph lock, the driver-host lifetime and one thin method per
//! store operation. This module owns that facade in Rust and the C++ source
//! tree under `src/` is gone.
//!
//! The realtime rules from the old `SKILL.md` still hold and are now enforced
//! by the language itself: no allocation on the audio path (the block walk
//! reuses one wrapped view per callback), no `std::mutex` on the audio thread
//! (the graph lock is taken on the control plane only), and the denormal FPU
//! guard brackets every block exactly as the C++ guard did.

use std::ffi::c_void;
use std::ptr;
use parking_lot::Mutex;
use std::sync::OnceLock;

use crate::engine_session::{
    handle_of_host, H_BOUNCE_SESSION, H_BUS_SYSTEM, H_CONTROL_ROOM, H_ENGINE_BLOCK,
    H_ENGINE_RINGS, H_EXTERNAL_SYNC, H_FORENSIC_JOURNAL, H_INPUT_BLOCKS, H_MASTER_LIMITER,
    H_METRONOME, H_METER, H_MIDI_LEARN, H_MIDI_SNAPSHOT_STORE, H_MPE_MANAGER, H_MPE_STATE,
    H_PDC, H_PREVIEW_SAMPLER, H_PREVIEW_SYNTH, H_ROUTING, H_SIDECHAIN_ENGINE,
    H_SIDECHAIN_ROUTES, H_TEMPO, H_TRACK_TABLE, H_UNDO,
};
use crate::track_session::TRACK_TYPE_BUS;

/// The largest block the device host may deliver. The native graph, the driver
/// callback and the FFI block entry points all share this bound.
pub const MAX_AUDIO_BLOCK_SIZE: u32 = 16_384;
/// Published-graph capacities the C++ engine declared.
pub const MAX_TRACKS: u32 = 256;
pub const MAX_TRACK_MAP: u32 = 2048;

const DRIVER_MAX_CHANNELS: usize = 32;

/// The diagnostics ring is Rust-owned; post through the shared helper.
fn log_post(level: u32, component: u32, message: &str) {
    unsafe {
        crate::log_buffer::hirari_log_buffer_post(
            crate::log_buffer::hirari_log_buffer_default_state(),
            level,
            component,
            message.as_ptr(),
            message.len(),
        );
    }
}

/// The engine's whole store set and its graph lock. The lock is a Rust
/// [`Mutex`] now: the address handed to the session bag is the address of this
/// box, and every control-plane walk — the Rust edit hosts and the facade
/// methods below — brackets the same graph with it.
pub struct HirariUnifiedEngine {
    tracks_mutex: Box<Mutex<()>>,
    session: *mut c_void,
}

impl HirariUnifiedEngine {
    /// Builds the engine: one Rust graph lock, the session bag bound to this
    /// engine's address, and the sandbox recovery loop. The bag built and
    /// bound every store, including the block, before this returns; only the
    /// recovery loop needs the engine's own address to key by.
    fn new() -> Box<Self> {
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

    fn handle(&self, kind: u32) -> *mut c_void {
        handle_of_host(self as *const Self as *const c_void, kind)
    }

    fn block(&self) -> *mut c_void {
        self.handle(H_ENGINE_BLOCK)
    }

    fn track_table(&self) -> *mut c_void {
        self.handle(H_TRACK_TABLE)
    }

    fn host(&self) -> *mut c_void {
        self as *const Self as *mut c_void
    }

    /// Locks the graph for one control-plane walk. The lock is not recursive,
    /// so a call that takes it itself — the graph resync — runs once this
    /// guard drops.
    fn lock_tracks(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.tracks_mutex.lock()
    }

    /// The track in the live graph, resolved under the graph lock.
    fn find_track_locked(&self, tid: u32) -> *mut c_void {
        unsafe { crate::engine_track_table::hirari_engine_track_find(self.track_table(), tid) }
    }

    /// The track in the published snapshot, read without the graph lock.
    fn find_published_track(&self, tid: u32) -> *mut c_void {
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
    fn take_track_owner(&self, tid: u32) -> *mut c_void {
        let _lock = self.lock_tracks();
        unsafe { crate::engine_track_table::hirari_engine_track_retain(self.track_table(), tid) }
    }

    /// Republishes the track snapshot under the caller's graph lock.
    fn publish_track_snapshot_locked(&self, stamp_audio_generation: bool) {
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
    unsafe fn process_block(&self, output: *mut c_void, offset: u32, size: u32) {
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
    unsafe fn process_block_direct(&self, buffers: *mut *mut f32, num_channels: u32, num_samples: u32) {
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
    fn published_tracks_live_locked(&self) -> Vec<*mut c_void> {
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
    fn enter() -> Self {
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

/// The static ops table the bounce driver calls back into. Static storage: an
/// async worker uses it after the starting call returned.
fn bounce_ops() -> &'static crate::bounce_driver::HirariBounceHostOps {
    static OPS: OnceLock<crate::bounce_driver::HirariBounceHostOps> = OnceLock::new();
    OPS.get_or_init(|| crate::bounce_driver::HirariBounceHostOps {
        run: Some(bounce_run),
        reset: Some(bounce_reset),
        store: Some(bounce_store),
        render: Some(bounce_render),
        render_chunk: Some(bounce_render_chunk),
        read_transport: Some(bounce_read_transport),
        set_transport: Some(bounce_set_transport),
    })
}

unsafe extern "C" fn bounce_run(host: *mut c_void, path: *const u8, length: usize, format: u32) -> u8 {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let bytes = unsafe { std::slice::from_raw_parts(path, length) };
    let Ok(path) = std::str::from_utf8(bytes) else { return 0 };
    u8::from(engine.bounce_project(path, format))
}

/// Clears stateful processor history so a bounce is reproducible.
unsafe extern "C" fn bounce_reset(host: *mut c_void) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let limiter = engine.handle(H_MASTER_LIMITER);
    if !limiter.is_null() {
        unsafe { crate::master_limiter::hirari_master_limiter_reset(limiter) };
    }
    let _lock = engine.lock_tracks();
    for track in engine.published_tracks_live_locked() {
        if !track.is_null() {
            unsafe { crate::engine_track::hirari_track_reset_for_offline_render(track) };
        }
    }
}

/// The plan walk is Rust's (bounce_driver.rs): this only names the stores.
unsafe extern "C" fn bounce_store(host: *mut c_void, kind: u32) -> *mut c_void {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    match kind {
        0 => engine.track_table(),                       // kHirariBounceStoreTrackTable
        1 => engine.block(),                             // kHirariBounceStoreEngineBlock
        2 => engine.handle(H_MIDI_SNAPSHOT_STORE),       // kHirariBounceStoreMidiSnapshot
        _ => engine.handle(H_MASTER_LIMITER),            // kHirariBounceStoreMasterLimiter
    }
}

/// The driver's channel pairs are wrapped, not copied, so the offline graph
/// renders straight into the caller's block buffers.
unsafe extern "C" fn bounce_render(host: *mut c_void, left: *mut f32, right: *mut f32, frames: u32) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let mut channels: [*mut f32; 2] = [left, right];
    let view = { crate::audio_buffer_ops::hirari_audio_buffer_storage_create() };
    unsafe {
        let _ = crate::audio_buffer_ops::hirari_audio_buffer_storage_wrap_channels(
            view,
            channels.as_mut_ptr(),
            2,
            frames,
        );
        engine.process_block(view, 0, frames);
        crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(view);
    }
}

/// Tracks size their buffers to the engine block size and silently skip larger
/// blocks; never render past the prepared block.
unsafe extern "C" fn bounce_render_chunk(host: *mut c_void) -> u32 {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let block = unsafe {
        crate::engine_block_host::hirari_engine_block_device_block_frames(engine.block())
    };
    if block == 0 {
        1024
    } else {
        block.min(1024)
    }
}

unsafe extern "C" fn bounce_read_transport(host: *mut c_void, playhead: *mut u64, playing: *mut u8) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe {
        *playhead = crate::engine_block_host::hirari_engine_block_playhead(engine.block());
        *playing = crate::engine_block_host::hirari_engine_block_is_playing(engine.block());
    }
}

unsafe extern "C" fn bounce_set_transport(host: *mut c_void, playhead: u64, playing: u8) {
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe {
        crate::engine_block_host::hirari_engine_block_set_playhead(engine.block(), playhead);
        crate::engine_block_host::hirari_engine_block_set_playing(engine.block(), playing);
    }
}

// ---------------------------------------------------------------------------
// Region edit host callbacks.
//
// The Rust region store owns the table, its undo capture and its snapshot
// publication; what remains is only the engine surface it cannot own: the
// track registry, the handle lifetimes for imported audio and the
// pitch-analysis gate on the Track itself.
// ---------------------------------------------------------------------------

unsafe extern "C" fn region_host_list_tracks(host: *mut c_void, out: *mut u32, capacity: usize) -> usize {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let _lock = engine.lock_tracks();
    let mut count = 0usize;
    for track in engine.published_tracks_live_locked() {
        if track.is_null() {
            continue;
        }
        if !out.is_null() && count < capacity {
            unsafe { *out.add(count) = crate::engine_track::hirari_track_id(track) };
        }
        count += 1;
    }
    count
}

unsafe extern "C" fn region_host_tempo_state(host: *mut c_void) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).handle(H_TEMPO) }
}

unsafe extern "C" fn region_host_undo_state(host: *mut c_void) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).handle(H_UNDO) }
}

unsafe extern "C" fn region_host_sample_rate(host: *mut c_void) -> f64 {
    if host.is_null() {
        return 0.0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe { crate::engine_block_host::hirari_engine_block_device_sample_rate(engine.block()) }
}

unsafe extern "C" fn region_host_track_identity(host: *mut c_void, track_id: u32) -> usize {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let _lock = engine.lock_tracks();
    engine.find_track_locked(track_id) as usize
}

unsafe extern "C" fn region_host_retain_track(host: *mut c_void, track_id: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).take_track_owner(track_id) }
}

unsafe extern "C" fn region_host_release_track(handle: *mut c_void) {
    unsafe { crate::engine_track::hirari_track_owner_release(handle) };
}

unsafe extern "C" fn region_host_pitch_begin(track_owner: *mut c_void) -> bool {
    unsafe { crate::engine_track::hirari_track_pitch_begin(owner_object(track_owner)) }
}

unsafe extern "C" fn region_host_pitch_finish(track_owner: *mut c_void) {
    unsafe { crate::engine_track::hirari_track_pitch_finish(owner_object(track_owner)) };
}

unsafe extern "C" fn region_host_note_snapshot(track_owner: *mut c_void, region_id: u32) -> *mut c_void {
    unsafe {
        crate::region_store::hirari_region_store_note_snapshot(
            owner_object(track_owner),
            region_id,
        )
    }
}

/// The live track behind an owner handle, or null.
unsafe fn owner_object(owner: *mut c_void) -> *mut c_void {
    unsafe { crate::engine_track::hirari_track_owner_object(owner) }
}

fn region_edit_host_ops() -> crate::region_edit_host::HirariRegionHostOps {
    crate::region_edit_host::HirariRegionHostOps {
        // The Rust region store answers the region-data callbacks.
        audio_view: Some(crate::storage_handle::hirari_storage_handle_view),
        release_audio: Some(crate::storage_handle::hirari_storage_handle_release),
        list_tracks: Some(region_host_list_tracks),
        tempo_state: Some(region_host_tempo_state),
        undo_state: Some(region_host_undo_state),
        sample_rate: Some(region_host_sample_rate),
        // region_path and insert_region: Rust-owned.
        track_identity: Some(region_host_track_identity),
        retain_track: Some(region_host_retain_track),
        release_track: Some(region_host_release_track),
        pitch_begin: Some(region_host_pitch_begin),
        pitch_finish: Some(region_host_pitch_finish),
        note_snapshot: Some(region_host_note_snapshot),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// The other edit-host tables. Every store they name is a session-bag slot, so
// the handlers are one-line resolvers plus the two engine operations that must
// stay here: the layout mark and the graph resync.
// ---------------------------------------------------------------------------

unsafe extern "C" fn track_edit_store(host: *mut c_void, kind: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    match kind {
        0 => engine.track_table(),                  // kHirariTrackEditStoreTrackTable
        1 => engine.block(),                        // kHirariTrackEditStoreEngineBlock
        2 => engine.handle(H_SIDECHAIN_ENGINE),     // kHirariTrackEditStoreSidechain
        3 => engine.handle(H_SIDECHAIN_ROUTES),     // kHirariTrackEditStoreSidechainRoutes
        4 => engine.handle(H_ROUTING),              // kHirariTrackEditStoreRouting
        5 => unsafe {
            crate::engine_block_host::hirari_engine_block_layout_revision(engine.block())
        },                                          // kHirariTrackEditStoreLayoutRevision
        6 => engine.handle(H_BUS_SYSTEM),           // kHirariTrackEditStoreBusSystem
        _ => engine.handle(H_ROUTING),
    }
}

unsafe extern "C" fn track_edit_undo_state(host: *mut c_void) -> *mut c_void {
    region_host_undo_state(host)
}

unsafe extern "C" fn track_edit_sync(host: *mut c_void) {
    if !host.is_null() {
        unsafe { (*(host as *const HirariUnifiedEngine)).sync_structural_changes() };
    }
}

unsafe extern "C" fn routing_mark_layout_changed(host: *mut c_void) {
    if !host.is_null() {
        unsafe { (*(host as *const HirariUnifiedEngine)).mark_project_layout_changed() };
    }
}

unsafe extern "C" fn routing_sync_structural(host: *mut c_void) {
    // sync_structural_changes takes the graph lock itself; Rust calls this only
    // after every mutation callback has released its lock.
    track_edit_sync(host);
}

unsafe extern "C" fn routing_store_routing(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_ROUTING)
}

unsafe extern "C" fn routing_store_routes(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_SIDECHAIN_ROUTES)
}

unsafe extern "C" fn routing_store_engine(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_SIDECHAIN_ENGINE)
}

unsafe extern "C" fn routing_block_store(host: *mut c_void) -> *mut c_void {
    routing_store_of(host, H_ENGINE_BLOCK)
}

unsafe extern "C" fn routing_store_of(host: *mut c_void, kind: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*(host as *const HirariUnifiedEngine)).handle(kind) }
}

unsafe extern "C" fn sandbox_maintenance_statuses(
    host: *mut c_void,
    context: *mut c_void,
    visit: Option<crate::plugin_sandbox_maintenance::StatusVisitorFn>,
) -> bool {
    if host.is_null() {
        return false;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let _lock = engine.lock_tracks();
    for track in engine.published_tracks_live_locked() {
        unsafe {
            crate::engine_track::hirari_track_visit_sandbox_statuses(track, context, visit);
        }
    }
    true
}

/// The restart can block on a worker process, so the track is held by handle
/// and the topology lock is already released around it.
unsafe extern "C" fn sandbox_maintenance_maintain(
    host: *mut c_void,
    track_id: u32,
    auto_restart: bool,
    context: *mut c_void,
    visit: Option<crate::plugin_sandbox_maintenance::StatusVisitorFn>,
) -> u32 {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let owner = engine.take_track_owner(track_id);
    if owner.is_null() {
        return 0;
    }
    let recovered = unsafe {
        crate::engine_track::hirari_track_maintain_sandboxes(
            owner_object(owner),
            auto_restart,
            context,
            visit,
        )
    };
    unsafe { crate::engine_track::hirari_track_owner_release(owner) };
    recovered
}

unsafe extern "C" fn sandbox_maintenance_publish_event(
    host: *mut c_void,
    event_type: u32,
    track_id: u32,
    value: f32,
    label: *const u8,
    label_length: usize,
) {
    if host.is_null() {
        return;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let block = engine.block();
    unsafe {
        crate::engine_rings::hirari_engine_event_ring_push(
            engine.handle(H_ENGINE_RINGS).cast(),
            crate::engine_block_host::hirari_engine_block_playhead(block),
            event_type,
            track_id,
            value,
            label,
            label_length,
        );
    }
}

unsafe extern "C" fn expression_map_track_articulation(host: *mut c_void, track_id: u32) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let track = engine.find_published_track(track_id);
    if track.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::engine_track::hirari_track_storage(track, 5) } // STORAGE_ARTICULATION
}

unsafe extern "C" fn expression_map_set_map_json(
    host: *mut c_void,
    track_id: u32,
    kind: u8,
    data: *const u8,
    length: usize,
) -> bool {
    if host.is_null() || (length != 0 && data.is_null()) {
        return false;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    let track = engine.find_published_track(track_id);
    unsafe { crate::engine_track::hirari_track_expression_map_set(track, kind, data, length) }
}

unsafe extern "C" fn freeze_acquire(host: *mut c_void, tid: u32, wait: bool) -> *mut c_void {
    if host.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    if wait {
        engine.join_audio_callbacks();
    }
    engine.take_track_owner(tid)
}

unsafe extern "C" fn freeze_block_size(host: *mut c_void) -> u32 {
    if host.is_null() {
        return 0;
    }
    let engine = unsafe { &*(host as *const HirariUnifiedEngine) };
    unsafe {
        crate::engine_block_host::hirari_engine_block_device_block_frames(engine.block())
    }
}

/// Every host table is installed once, before any caller can reach the engine,
/// so the install order is deterministic instead of depending on a linker's
/// cross-file static order.
fn install_native_host_tables() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| unsafe {
        crate::track_edit_host::hirari_track_edit_host_install(
            &crate::track_edit_host::HirariTrackEditHostOps {
                list_tracks: Some(region_host_list_tracks),
                list_regions: Some(crate::region_edit_host::hirari_region_store_list_regions),
                store: Some(track_edit_store),
                // Undo capture lives in the Rust track edit host.
                record_replay: None,
                sync_structural_changes: Some(track_edit_sync),
                undo_state: Some(track_edit_undo_state),
            },
        );
        crate::plugin_sandbox_maintenance::hirari_plugin_sandbox_maintenance_host_install(
            &crate::plugin_sandbox_maintenance::HirariPluginSandboxHostOps {
                statuses: Some(sandbox_maintenance_statuses),
                maintain: Some(sandbox_maintenance_maintain),
                publish_event: Some(sandbox_maintenance_publish_event),
            },
        );
        crate::routing_edit_host::hirari_routing_edit_host_install(
            &crate::routing_edit_host::HirariRoutingHostOps {
                routing_store: Some(routing_store_routing),
                sidechain_routes_store: Some(routing_store_routes),
                sidechain_engine_store: Some(routing_store_engine),
                // Sidechain context and registration live in the Rust routing
                // edit host; undo capture lives there too.
                sidechain_context: None,
                register_sidechain: None,
                mark_layout_changed: Some(routing_mark_layout_changed),
                sync_structural: Some(routing_sync_structural),
                record_undo: None,
                undo_state: Some(track_edit_undo_state),
                engine_block_store: Some(routing_block_store),
            },
        );
        crate::expression_map_edit_host::hirari_expression_map_edit_host_install(
            &crate::expression_map_edit_host::HirariExpressionMapEditHostOps {
                track_articulation: Some(expression_map_track_articulation),
                set_map_json: Some(expression_map_set_map_json),
            },
        );
        crate::track_freeze_host::hirari_track_freeze_host_install(
            &crate::track_freeze_host::HirariTrackFreezeHostOps {
                // The owner handle is the Rust Arc's boxed clone, so it keeps
                // the track alive after the topology lock is released.
                acquire: Some(freeze_acquire),
                release: Some(crate::engine_track::hirari_track_owner_release),
                block_size: Some(freeze_block_size),
            },
        );
        // The tracks and their storages are Rust objects, so the spatial-edit
        // host needs no native table, and neither do the automation and
        // control edits or command admission. The region storage host must
        // install before the region edit host, which routes its data callbacks
        // only once it answers.
        crate::engine_track::hirari_track_region_host_install();
        crate::region_edit_host::hirari_region_host_install(&region_edit_host_ops());
    });
}

// ---------------------------------------------------------------------------
// The session wrapper.
// ---------------------------------------------------------------------------

/// The session audio engine: one graph, one device host and the config lock
/// the device transactions serialize through.
pub struct AudioEngine {
    /// Session-owned graph. Separate FFI handles receive separate project
    /// state.
    engine: Box<HirariUnifiedEngine>,
    /// The Rust device host this session constructed.
    driver: *mut c_void,
    /// The configuration transaction lock (`engine_config.rs` drives it).
    config_lock: Mutex<()>,
}

impl AudioEngine {
    fn new(start_device: bool) -> Box<Self> {
        install_native_host_tables();
        let engine = HirariUnifiedEngine::new();
        let mut session = Box::new(AudioEngine {
            engine,
            driver: ptr::null_mut(),
            config_lock: Mutex::new(()),
        });
        // The caller owns these buffers; the Rust input store copies what the
        // graph needs so no driver pointer is retained beyond the callback.
        // SAFETY: the context registered here is `session` itself, whose
        // address is stable inside its box, and the device is not running yet.
        let context = &*session as *const AudioEngine as *mut c_void;
        session.driver = {
            crate::audio_driver_host::hirari_audio_driver_host_create(
                Some(process_driver_block),
                context,
            )
        };
        // Claiming the device is the Rust host's creation policy: it declines
        // for an offline or isolated session and publishes the failure itself.
        {
            crate::audio_driver_host::hirari_audio_driver_host_start_at_creation(
                session.driver,
                start_device,
            );
        }
        session
    }

    pub fn get_core(&self) -> &HirariUnifiedEngine {
        &self.engine
    }

    pub fn control_room_state(&self) -> usize {
        self.engine.handle(H_CONTROL_ROOM) as usize
    }

    pub fn tempo_state(&self) -> usize {
        self.engine.handle(H_TEMPO) as usize
    }

    pub fn undo_state(&self) -> usize {
        self.engine.handle(H_UNDO) as usize
    }

    pub fn log_state(&self) -> usize {
        crate::log_buffer::hirari_log_buffer_default_state() as usize
    }

    pub fn engine_block_state(&self) -> usize {
        self.engine.block() as usize
    }

    pub fn audio_driver_host(&self) -> usize {
        self.driver as usize
    }

    pub fn is_driver_running(&self) -> bool {
        !self.driver.is_null()
            && {
                crate::audio_driver_host::hirari_audio_driver_host_is_running(self.driver)
            }
    }

    pub fn is_audio_device_ready(&self) -> bool {
        let _lock = self.config_guard();
        let ready = self.is_driver_running();
        if !ready && !self.engine.block().is_null() && self.transport_playing() {
            // Device loss is a transport safety boundary, not merely a UI
            // status change. Stop the graph even when no UI poll is running.
            self.engine.set_playing(false);
        }
        ready
    }

    /// Transport start/stop is Rust-owned (`engine_transport.rs`); this is the
    /// engine-side safety read the device checks use.
    fn transport_playing(&self) -> bool {
        unsafe { crate::engine_block_host::hirari_engine_block_is_playing(self.engine.block()) != 0 }
    }

    pub fn try_reconnect_audio_device(&self) {
        if crate::audio_device_mode::hirari_native_test_isolation() {
            return;
        }
        let _lock = self.config_guard();
        if !self.driver.is_null() {
            crate::audio_driver_host::hirari_audio_driver_host_try_reconnect(self.driver);
            if !self.is_driver_running() {
                self.engine.set_playing(false);
            }
        }
    }

    /// The configuration transaction is Rust-owned (`engine_config.rs`); the
    /// session lock serializes it with shutdown and reconnect.
    pub fn config_guard(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.config_lock.lock()
    }

    pub fn get_audio_config_generation(&self) -> u64 {
        unsafe {
            crate::engine_block_host::hirari_engine_block_generation(self.engine.block())
        }
    }

    /// Processes one bounded stereo block through the native graph. Rust must
    /// not maintain a second production audio graph.
    pub fn process_audio_block(&self, left: &mut [f32], right: &mut [f32]) {
        // The native graph is prepared for bounded realtime blocks. Reject
        // oversized FFI slices before narrowing to u32; otherwise a hostile or
        // malformed caller could wrap the frame count and reach the DSP with a
        // different size than the slice actually contains.
        if left.is_empty()
            || right.is_empty()
            || left.len() != right.len()
            || left.len() > MAX_AUDIO_BLOCK_SIZE as usize
        {
            return;
        }
        // A host with no device to drive its callback dispatches the block
        // through the same boundary a real driver would use; a running device
        // already drives it, so the dispatcher answers false and the graph
        // renders here.
        let dispatched = {
            crate::audio_driver_host::hirari_audio_driver_host_dispatch_process(
                self.driver,
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len() as u32,
            )
        };
        if dispatched {
            return;
        }
        let mut channels: [*mut f32; 2] = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            self.engine
                .process_block_direct(channels.as_mut_ptr(), 2, left.len() as u32);
        }
    }

    /// Compatibility API: stop only this wrapper's device and session graph.
    pub fn shutdown(&self) {
        let _lock = self.config_guard();
        if !self.driver.is_null() {
            crate::audio_driver_host::hirari_audio_driver_host_stop(self.driver);
        }
        self.engine.shutdown();
    }

    pub fn add_region(&self, tid: u32, path: &str, start_beat: f64) -> bool {
        self.engine.add_region(tid, path, start_beat)
    }

    /// Project save and load are Rust transactions (`layout_gather.rs`,
    /// `project_load_host.rs`); this only names the document path.
    pub fn save_project(&self, path: &str) -> bool {
        unsafe {
            crate::layout_gather::hirari_project_save_from_host(
                self.engine.host(),
                path.as_ptr(),
                path.len(),
            )
        }
    }

    pub fn load_project(&self, path: &str) -> bool {
        unsafe {
            crate::project_load_host::hirari_project_load_from_host(
                self.engine.host(),
                path.as_ptr(),
                path.len(),
            )
        }
    }

    pub fn bounce_project(&self, path: &str, format: u32) -> bool {
        self.engine.bounce_project(path, format)
    }

    pub fn bounce_project_async(&self, path: &str, format: u32) -> bool {
        self.engine.bounce_project_async(path, format)
    }

    pub fn queue_region_waveform(&self, tid: u32, rid: u32) -> u64 {
        if tid == 0 || rid == 0 {
            return 0;
        }
        unsafe {
            crate::async_snapshots::hirari_waveform_snapshot_request(
                self.engine.host(),
                tid,
                rid,
                Some(compute_region_waveform),
                Some(destroy_noop),
            )
        }
    }

    /// Command dispatch. Admission lives in Rust (`command_host.rs`).
    pub fn push_command(
        &self,
        command_type: u32,
        tid: u32,
        val: f32,
        ts: u64,
        expected_project_generation: u64,
        expected_audio_generation: u64,
    ) -> bool {
        unsafe {
            crate::command_host::hirari_engine_push_command(
                self.engine.host(),
                command_type,
                tid,
                val,
                ts,
                expected_project_generation,
                expected_audio_generation,
            )
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        // Cancel bridge-owned offline work before the handle disappears, then
        // stop the device before the graph it drives.
        self.engine.shutdown();
        if !self.driver.is_null() {
            unsafe { crate::audio_driver_host::hirari_audio_driver_host_destroy(self.driver) };
            self.driver = ptr::null_mut();
        }
    }
}

/// # Safety
/// `context` must be a live engine host, per the request contract.
unsafe extern "C" fn compute_region_waveform(
    context: *mut c_void,
    track_id: u32,
    region_id: u32,
    output: *mut f32,
    capacity: usize,
) -> usize {
    if context.is_null() || output.is_null() {
        return 0;
    }
    let buffer =
        unsafe { crate::region_audio::hirari_region_waveform(context, track_id, region_id, 128) };
    if buffer.is_null() {
        return 0;
    }
    let count = unsafe {
        let data = crate::region_audio::hirari_float_buffer_data(buffer);
        let count = crate::region_audio::hirari_float_buffer_len(buffer);
        if data.is_null() {
            crate::region_audio::hirari_float_buffer_destroy(buffer);
            return 0;
        }
        let copied = count.min(capacity);
        std::ptr::copy_nonoverlapping(data, output, copied);
        crate::region_audio::hirari_float_buffer_destroy(buffer);
        copied
    };
    count
}

unsafe extern "C" fn destroy_noop(_context: *mut c_void) {}

/// The device callback. Validates the caller's buffer contract, stages the
/// hardware input into the Rust input store and renders the block.
unsafe extern "C" fn process_driver_block(
    inputs: *const *const f32,
    input_channel_count: u32,
    outputs: *const *mut f32,
    output_channel_count: u32,
    frames: u32,
    context: *mut c_void,
) {
    if context.is_null() || outputs.is_null() || output_channel_count < 2
        || output_channel_count as usize > DRIVER_MAX_CHANNELS
        || frames == 0
        || frames > MAX_AUDIO_BLOCK_SIZE
    {
        return;
    }
    for channel in 0..output_channel_count {
        if unsafe { *outputs.add(channel as usize) }.is_null() {
            return;
        }
    }
    let session = unsafe { &*(context as *const AudioEngine) };
    // The caller owns these buffers; the Rust input store copies what the
    // graph needs so no driver pointer is retained beyond the callback.
    if !inputs.is_null() && input_channel_count > 0 && unsafe { *inputs } != ptr::null() {
        unsafe {
            crate::engine_input_block::hirari_input_blocks_publish_monitor(
                session.engine.handle(H_INPUT_BLOCKS),
                inputs,
                input_channel_count,
                frames,
            );
        }
    }
    unsafe {
        crate::engine_input_block::hirari_input_blocks_publish_talkback(
            session.engine.handle(H_INPUT_BLOCKS),
            inputs,
            input_channel_count,
            frames,
            session.engine.handle(H_CONTROL_ROOM),
        );
    }
    let mut mutable_outputs: [*mut f32; DRIVER_MAX_CHANNELS] = [ptr::null_mut(); DRIVER_MAX_CHANNELS];
    for (index, slot) in mutable_outputs.iter_mut().enumerate().take(output_channel_count as usize) {
        *slot = unsafe { *outputs.add(index) };
    }
    unsafe {
        session
            .engine
            .process_block_direct(mutable_outputs.as_mut_ptr(), output_channel_count, frames);
    }
}

/// The wire code of one bridge command.
fn command_code(cmd: crate::ffi::HirariCommand) -> u32 {
    match cmd {
        crate::ffi::HirariCommand::SetVolume => 0,
        crate::ffi::HirariCommand::SetPan => 1,
        crate::ffi::HirariCommand::SetMute => 2,
        crate::ffi::HirariCommand::SetSolo => 3,
        crate::ffi::HirariCommand::AddPlugin => 4,
        crate::ffi::HirariCommand::RemovePlugin => 5,
        crate::ffi::HirariCommand::GenerateDrumFill => 6,
        crate::ffi::HirariCommand::ExecuteAutoMixing => 7,
        crate::ffi::HirariCommand::ExecuteAutoArrangement => 8,
        crate::ffi::HirariCommand::SetSpatialMode => 9,
        crate::ffi::HirariCommand::SetSpatialPosition => 10,
        crate::ffi::HirariCommand::SetAscendedMode => 11,
        crate::ffi::HirariCommand::SetAutomationRecordMode => 12,
        // cxx shared enums are open to future codes; admission rejects them.
        _ => u32::MAX,
    }
}

/// Creates the session engine with the device start policy the offline mode
/// selects.
pub fn new_audio_engine() -> Box<AudioEngine> {
    AudioEngine::new(true)
}

/// Creates the session engine without claiming a device: offline renders,
/// automation and isolated tests.
pub fn new_audio_engine_offline() -> Box<AudioEngine> {
    AudioEngine::new(false)
}

/// The inner engine behind a bridge handle.
pub fn get_unified_engine(bridge: &AudioEngine) -> &HirariUnifiedEngine {
    bridge.get_core()
}

/// Command admission through the shared Rust host.
pub fn push_command(
    engine: &AudioEngine,
    cmd: crate::ffi::HirariCommand,
    tid: u32,
    val: f32,
    ts: u64,
    expected_project_generation: u64,
    expected_audio_generation: u64,
) -> bool {
    // Branchless boundary validation: a track id outside the table is refused
    // before admission.
    let mask = u32::from(tid < 1024);
    if mask == 0 {
        return false;
    }
    engine.push_command(
        command_code(cmd),
        tid,
        val,
        ts,
        expected_project_generation,
        expected_audio_generation,
    )
}
